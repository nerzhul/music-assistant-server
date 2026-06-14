# `crates/` — Music Assistant Rust port

Workspace members:

| Crate | Status | Description |
|---|---|---|
| `ma-core` | Phase 0 | Core types, enums, errors, WebSocket/JSON-RPC messages |
| `ma-config` | Phase 0 | TOML config loading + `MA_*` envvar overrides |
| `ma-runtime` | Phase 0 | Tokio runtime, signal handling, tracing setup |
| `ma-server` | Phase 0 | `ma-server` binary: `/info`, `/logo.png`, `/`, `/health` |

Planned (see top-level plan):

* `ma-protocol-sendspin` — serde models for the Sendspin spec
* `ma-player-sendspin` — WebSocket server + mDNS + codecs + time filter
* `ma-streams` — multi-client HTTP audio server (axum on 8097)
* `ma-provider-filesystem`, `ma-provider-radio`, `ma-provider-spotify`,
  `ma-provider-cover`, `ma-provider-ytmusic`, `ma-provider-podcasts`
* `ma-player-sync-group`, `ma-player-universal-group`, `ma-player-bridge`
* `ma-cache-s3`, `ma-ha`, `ma-discovery`

## Build & test

```sh
cargo build                  # debug
cargo build --release        # release
cargo test                   # unit + integration
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Run

```sh
# defaults: bind 0.0.0.0:8095, data dir ~/.musicassistant
cargo run -p ma-server

# override
MA_BIND_PORT=18095 MA_LOG_LEVEL=debug cargo run -p ma-server
```

## Verifying phase 0

```sh
curl http://localhost:8095/info     # ServerInfoMessage JSON
curl -I http://localhost:8095/logo.png
curl http://localhost:8095/health
```

The JSON shape of `/info` matches the `ServerInfoMessage` from
`music_assistant_models.api` (same field names, same types) so the
Music Assistant frontend can talk to this binary from day one.
