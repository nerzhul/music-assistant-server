//! S3-compatible object store implementation.
//!
//! The implementation is intentionally simple: it uses `reqwest` against the
//! AWS S3 v4 REST API (with optional path-style addressing for MinIO /
//! TrueNAS). The operations supported are the four we need for a cache
//! (`put`, `get`, `exists`, `delete`) plus a presign helper for read URLs.
//!
//! Auth is static (access key + secret) for now. STS / IRSA / OIDC can be
//! added later by injecting a [`reqwest::RequestBuilder`] wrapper that signs
//! requests.

#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::{Digest, Sha256};
use url::Url;

use super::{BytesStream, CacheError, ObjectStore};
use tokio_stream::StreamExt;

/// Configuration for an S3-compatible object store.
#[derive(Debug, Clone)]
pub struct S3Config {
    /// Endpoint URL (e.g. `https://s3.eu-west-1.amazonaws.com` or
    /// `http://minio.local:9000`).
    pub endpoint: String,
    /// Region (used for the AWS v4 signature).
    pub region: String,
    /// Bucket name.
    pub bucket: String,
    /// Key prefix (a namespace) prepended to every object key. Useful to
    /// share a bucket between multiple MA instances.
    pub key_prefix: String,
    /// Access key id. Empty means "anonymous" (rare, mainly for testing).
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Optional session token (for STS / SSO).
    pub session_token: Option<String>,
    /// `true` for path-style addresses (`<endpoint>/<bucket>/<key>`), `false`
    /// for virtual-hosted style (`<bucket>.<endpoint>/<key>`). MinIO and
    /// TrueNAS require `true`.
    pub path_style: bool,
    /// If set, returned public URLs will use this base (e.g. a CDN).
    pub publish_host: Option<String>,
    /// TTL (seconds) for presigned read URLs.
    pub presign_ttl_secs: u32,
}

impl S3Config {
    /// Build an [`S3Config`] from the standard `MA_S3_*` envvars. Returns
    /// `None` if `MA_S3_BUCKET` is unset (S3 is opt-in).
    pub fn from_env() -> Option<Self> {
        let bucket = std::env::var("MA_S3_BUCKET").ok()?;
        let endpoint =
            std::env::var("MA_S3_ENDPOINT").unwrap_or_else(|_| "https://s3.amazonaws.com".into());
        let region = std::env::var("MA_S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let key_prefix = std::env::var("MA_S3_KEY_PREFIX").unwrap_or_default();
        let access_key_id = std::env::var("MA_S3_ACCESS_KEY_ID").unwrap_or_default();
        let secret_access_key = std::env::var("MA_S3_SECRET_ACCESS_KEY").unwrap_or_default();
        let session_token = std::env::var("MA_S3_SESSION_TOKEN").ok();
        let path_style = std::env::var("MA_S3_PATH_STYLE")
            .ok()
            .and_then(|s| match s.as_str() {
                "1" | "true" | "yes" => Some(true),
                "0" | "false" | "no" => Some(false),
                _ => None,
            })
            .unwrap_or(false);
        let publish_host = std::env::var("MA_S3_PUBLISH_HOST").ok();
        let presign_ttl_secs = std::env::var("MA_S3_PRESIGN_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(86_400);
        Some(Self {
            endpoint,
            region,
            bucket,
            key_prefix,
            access_key_id,
            secret_access_key,
            session_token,
            path_style,
            publish_host,
            presign_ttl_secs,
        })
    }

    fn full_key(&self, key: &str) -> String {
        if self.key_prefix.is_empty() {
            key.to_string()
        } else {
            format!(
                "{}/{}",
                self.key_prefix.trim_end_matches('/'),
                key.trim_start_matches('/')
            )
        }
    }

    fn url_for_key(&self, key: &str) -> Result<Url, CacheError> {
        let endpoint = self.endpoint.trim_end_matches('/');
        let full_key = self.full_key(key);
        let url = if self.path_style {
            format!("{}/{}/{}", endpoint, self.bucket, full_key)
        } else {
            // virtual-hosted style: rewrite host
            let parsed = Url::parse(endpoint).map_err(|e| CacheError::Backend(e.to_string()))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| CacheError::Backend("invalid endpoint".into()))?;
            let scheme = parsed.scheme();
            let port = parsed.port();
            let port_str = port.map(|p| format!(":{}", p)).unwrap_or_default();
            let path = parsed.path();
            format!(
                "{}://{}.{}{}{}/{}",
                scheme,
                self.bucket,
                host,
                port_str,
                path.trim_end_matches('/'),
                full_key
            )
        };
        Url::parse(&url).map_err(|e| CacheError::Backend(e.to_string()))
    }
}

/// S3-compatible object store.
#[derive(Debug, Clone)]
pub struct S3Store {
    cfg: S3Config,
    client: Client,
}

impl S3Store {
    /// Build a new [`S3Store`] from a config. Returns an error if the HTTP
    /// client cannot be created.
    pub fn new(cfg: S3Config) -> Result<Arc<Self>, CacheError> {
        let client = Client::builder()
            .user_agent(concat!("MusicAssistantRust/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        Ok(Arc::new(Self { cfg, client }))
    }

    /// Compute a public URL for the given key. Uses `publish_host` if set,
    /// otherwise returns a presigned read URL.
    pub fn public_url(&self, key: &str) -> Option<String> {
        let full_key = self.cfg.full_key(key);
        if let Some(base) = self.cfg.publish_host.as_ref() {
            return Some(format!("{}/{}", base.trim_end_matches('/'), full_key));
        }
        Some(self.presign_get(&full_key, self.cfg.presign_ttl_secs))
    }

    /// Build a presigned `GET` URL valid for `ttl_secs` seconds.
    pub fn presign_get(&self, full_key: &str, ttl_secs: u32) -> String {
        let host = self
            .cfg
            .url_for_key(full_key)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = Utc::now().format("%Y%m%d").to_string();
        let scope = format!("{}/{}/s3/aws4_request", date_stamp, self.cfg.region);

        let mut canonical_headers = format!("host:{}\n", host);
        if let Some(token) = &self.cfg.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{}\n", token));
        }
        let signed_headers = if self.cfg.session_token.is_some() {
            "host;x-amz-security-token"
        } else {
            "host"
        };

        let canonical_request = format!(
            "GET\n/{}\nX-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={}%2F{}&X-Amz-Date={}&X-Amz-Expires={}&X-Amz-SignedHeaders={}\n{}\nUNSIGNED-PAYLOAD",
            full_key,
            percent_encode(&format!("{}/{}", self.cfg.access_key_id, scope)),
            scope,
            amz_date,
            ttl_secs,
            signed_headers,
            canonical_headers,
        );

        let mut hasher = Sha256::new();
        hasher.update(canonical_request.as_bytes());
        let canonical_request_hash = hex_lower(&hasher.finalize());

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, scope, canonical_request_hash
        );

        let k_date = hmac_sha256(
            format!("AWS4{}", self.cfg.secret_access_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.cfg.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex_lower(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        let mut url = self.cfg.url_for_key(full_key).expect("validated earlier");
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("X-Amz-Algorithm", "AWS4-HMAC-SHA256");
            q.append_pair(
                "X-Amz-Credential",
                &format!("{}/{}", self.cfg.access_key_id, scope),
            );
            q.append_pair("X-Amz-Date", &amz_date);
            q.append_pair("X-Amz-Expires", &ttl_secs.to_string());
            q.append_pair("X-Amz-SignedHeaders", signed_headers);
            q.append_pair("X-Amz-Signature", &signature);
        }
        url.to_string()
    }

    async fn sign_and_send(
        &self,
        method: &str,
        key: &str,
        body: Option<Bytes>,
    ) -> Result<reqwest::Response, CacheError> {
        let url = self
            .cfg
            .url_for_key(key)
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| CacheError::Backend("invalid url".into()))?
            .to_string();
        let path = url.path().to_string();
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let mut canonical_headers = format!("host:{}\n", host);
        let mut signed_headers = vec!["host"];
        if let Some(token) = &self.cfg.session_token {
            canonical_headers.push_str(&format!("x-amz-security-token:{}\n", token));
            signed_headers.push("x-amz-security-token");
        }
        if body.is_some() {
            canonical_headers.push_str("x-amz-content-sha256:UNSIGNED-PAYLOAD\n");
            signed_headers.push("x-amz-content-sha256");
        }
        let signed_headers_str = signed_headers.join(";");

        let payload_hash = "UNSIGNED-PAYLOAD";

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            path,
            "", // canonical query string
            canonical_headers,
            signed_headers_str,
            payload_hash
        );

        let scope = format!("{}/{}/s3/aws4_request", date_stamp, self.cfg.region);

        let mut hasher = Sha256::new();
        hasher.update(canonical_request.as_bytes());
        let hashed_canonical = hex_lower(&hasher.finalize());

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, scope, hashed_canonical
        );

        let k_date = hmac_sha256(
            format!("AWS4{}", self.cfg.secret_access_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.cfg.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex_lower(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.cfg.access_key_id, scope, signed_headers_str, signature
        );

        let mut req = self
            .client
            .request(method.parse().unwrap(), url)
            .header("x-amz-date", amz_date)
            .header("Authorization", auth_header);
        if let Some(token) = &self.cfg.session_token {
            req = req.header("x-amz-security-token", token);
        }
        if body.is_some() {
            req = req.header("x-amz-content-sha256", "UNSIGNED-PAYLOAD");
        }
        if let Some(data) = body {
            req = req.body(data);
        }
        req.send()
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))
    }
}

#[async_trait]
impl ObjectStore for S3Store {
    async fn put(&self, key: &str, data: Bytes) -> Result<(), CacheError> {
        let resp = self.sign_and_send("PUT", key, Some(data)).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CacheError::S3(format!("PUT failed: {} {}", status, body)));
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<Bytes, CacheError> {
        let resp = self.sign_and_send("GET", key, None).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(CacheError::NotFound(self.cfg.full_key(key)));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CacheError::S3(format!("GET failed: {} {}", status, body)));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| CacheError::Backend(e.to_string()))?;
        Ok(bytes)
    }

    async fn get_stream(&self, key: &str) -> Result<BytesStream, CacheError> {
        let resp = self.sign_and_send("GET", key, None).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(CacheError::NotFound(self.cfg.full_key(key)));
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CacheError::S3(format!("GET failed: {} {}", status, body)));
        }
        let stream = resp
            .bytes_stream()
            .map(|item| item.map_err(|e| io_other(e.to_string())));
        Ok(Box::pin(stream))
    }

    async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let resp = self.sign_and_send("HEAD", key, None).await?;
        Ok(resp.status().is_success())
    }

    async fn delete(&self, key: &str) -> Result<(), CacheError> {
        let resp = self.sign_and_send("DELETE", key, None).await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CacheError::S3(format!(
                "DELETE failed: {} {}",
                status, body
            )));
        }
        Ok(())
    }

    fn public_url(&self, key: &str) -> Option<String> {
        S3Store::public_url(self, key)
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn io_other(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(msg);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_full_key_respects_prefix() {
        let cfg = S3Config {
            endpoint: "http://minio:9000".into(),
            region: "us-east-1".into(),
            bucket: "ma".into(),
            key_prefix: "covers/".into(),
            access_key_id: "x".into(),
            secret_access_key: "y".into(),
            session_token: None,
            path_style: true,
            publish_host: None,
            presign_ttl_secs: 60,
        };
        assert_eq!(cfg.full_key("abc.jpg"), "covers/abc.jpg");
        assert_eq!(cfg.full_key("/abc.jpg"), "covers/abc.jpg");

        let cfg2 = S3Config {
            key_prefix: String::new(),
            ..cfg.clone()
        };
        assert_eq!(cfg2.full_key("abc.jpg"), "abc.jpg");
    }

    #[test]
    fn config_url_for_key_path_style() {
        let cfg = S3Config {
            endpoint: "http://minio:9000".into(),
            region: "us-east-1".into(),
            bucket: "ma".into(),
            key_prefix: String::new(),
            access_key_id: "x".into(),
            secret_access_key: "y".into(),
            session_token: None,
            path_style: true,
            publish_host: None,
            presign_ttl_secs: 60,
        };
        let url = cfg.url_for_key("covers/abc.jpg").unwrap();
        assert_eq!(url.as_str(), "http://minio:9000/ma/covers/abc.jpg");
    }

    #[test]
    fn config_url_for_key_virtual_hosted() {
        let cfg = S3Config {
            endpoint: "https://s3.amazonaws.com".into(),
            region: "us-east-1".into(),
            bucket: "ma".into(),
            key_prefix: String::new(),
            access_key_id: "x".into(),
            secret_access_key: "y".into(),
            session_token: None,
            path_style: false,
            publish_host: None,
            presign_ttl_secs: 60,
        };
        let url = cfg.url_for_key("covers/abc.jpg").unwrap();
        assert_eq!(url.as_str(), "https://ma.s3.amazonaws.com/covers/abc.jpg");
    }

    #[test]
    fn presign_produces_url_with_query_params() {
        let cfg = S3Config {
            endpoint: "http://minio:9000".into(),
            region: "us-east-1".into(),
            bucket: "ma".into(),
            key_prefix: String::new(),
            access_key_id: "AKID".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            path_style: true,
            publish_host: None,
            presign_ttl_secs: 60,
        };
        let store = S3Store::new(cfg).unwrap();
        let url = store.presign_get("covers/abc.jpg", 60);
        assert!(url.contains("X-Amz-Signature="));
        assert!(url.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256"));
        assert!(url.contains("X-Amz-Credential=AKID%2F"));
    }

    #[test]
    fn public_url_with_publish_host() {
        let cfg = S3Config {
            endpoint: "http://minio:9000".into(),
            region: "us-east-1".into(),
            bucket: "ma".into(),
            key_prefix: "covers/".into(),
            access_key_id: "x".into(),
            secret_access_key: "y".into(),
            session_token: None,
            path_style: true,
            publish_host: Some("https://cdn.example.com".into()),
            presign_ttl_secs: 60,
        };
        let store = S3Store::new(cfg).unwrap();
        let url = store.public_url("abc.jpg").unwrap();
        assert_eq!(url, "https://cdn.example.com/covers/abc.jpg");
    }
}
