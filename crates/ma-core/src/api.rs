//! Generic command-handler registry for the JSON-RPC-like API.
//!
//! Mirrors the role of `music_assistant.helpers.api.APICommandHandler`:
//! every command has a name, a target closure, a `required_auth` flag
//! and a `required_role`. The registry serialises both directions
//! (callers dispatch `CommandMessage`s, the registry returns
//! `SuccessResultMessage` / `ErrorResultMessage`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::auth::UserRole;
use crate::errors::{ErrorCode, MusicAssistantError};
use crate::messages::{error_message, CommandMessage, ErrorResultMessage, SuccessResultMessage};

/// User role required to execute a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRole {
    /// Any caller, authenticated or not.
    Anonymous,
    /// Any authenticated user.
    Authenticated,
    /// Admin role only.
    Admin,
}

/// Wire result type returned by command handlers.
pub type HandlerResult = std::result::Result<Value, MusicAssistantError>;

/// Boxed future returned by a command handler.
pub type HandlerFuture = Pin<Box<dyn Future<Output = HandlerResult> + Send>>;

/// Command handler trait. The trait is object-safe: a `CommandHandler`
/// can be stored as a `Box<dyn CommandHandler>` or wrapped in an
/// `Arc<dyn CommandHandler>`.
pub trait CommandHandler: Send + Sync + 'static {
    fn handle(&self, ctx: CommandContext, args: Value) -> HandlerFuture;
}

impl<F, Fut> CommandHandler for F
where
    F: Fn(CommandContext, Value) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResult> + Send + 'static,
{
    fn handle(&self, ctx: CommandContext, args: Value) -> HandlerFuture {
        Box::pin((self)(ctx, args))
    }
}

/// Per-request context passed to command handlers.
#[derive(Debug, Clone, Default)]
pub struct CommandContext {
    /// `Some(user_id)` when the caller is authenticated via the
    /// long-lived token / OAuth pipeline.
    pub user_id: Option<String>,
    /// `Some(role)` for the caller. Drives the `RequiredRole` check.
    pub role: Option<UserRole>,
    /// `Some(player_id)` for Sendspin web players, so the handler can
    /// auto-whitelist a specific player for that session.
    pub sendspin_player_id: Option<String>,
}

/// Description of a registered command (used by the `/api-docs` endpoints).
#[derive(Debug, Clone)]
pub struct CommandDescription {
    pub name: String,
    pub required_role: RequiredRole,
    pub required_args: Vec<String>,
    pub description: Option<String>,
}

#[derive(Clone)]
struct Registration {
    description: CommandDescription,
    handler: Arc<dyn CommandHandler>,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Process-wide registry of API commands.
#[derive(Default, Clone)]
pub struct CommandRegistry {
    by_name: Arc<RwLock<HashMap<String, Registration>>>,
}

impl std::fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self.by_name.read().keys().cloned().collect();
        f.debug_struct("CommandRegistry")
            .field("commands", &names)
            .finish()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new command. Returns the previous handler (if any) for
    /// the same name so callers can decide whether to overwrite.
    pub fn register<H: CommandHandler>(
        &self,
        name: impl Into<String>,
        required_role: RequiredRole,
        required_args: Vec<String>,
        description: Option<String>,
        handler: H,
    ) -> Option<Arc<dyn CommandHandler>> {
        let name = name.into();
        let desc = CommandDescription {
            name: name.clone(),
            required_role,
            required_args,
            description,
        };
        let prev = self.by_name.write().insert(
            name,
            Registration {
                description: desc,
                handler: Arc::new(handler),
            },
        );
        prev.map(|r| r.handler)
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn CommandHandler>> {
        self.by_name
            .read()
            .get(name)
            .map(|r| Arc::clone(&r.handler))
    }

    /// Snapshot of every registered command.
    pub fn list(&self) -> Vec<CommandDescription> {
        self.by_name
            .read()
            .values()
            .map(|r| r.description.clone())
            .collect()
    }

    /// Dispatch a `CommandMessage` to its handler and serialise the
    /// result. Handles authentication / role checks based on the
    /// handler's `RequiredRole` and the supplied `ctx`.
    pub async fn dispatch(
        &self,
        msg: &CommandMessage,
        ctx: &CommandContext,
    ) -> Result<SuccessResultMessage, ErrorResultMessage> {
        let handler = match self.get(&msg.command) {
            Some(h) => h,
            None => {
                return Err(error_message(
                    msg.message_id.clone(),
                    ErrorCode::InvalidCommand.as_i32(),
                    format!("invalid command: {}", msg.command),
                ));
            }
        };
        let desc = self
            .by_name
            .read()
            .get(&msg.command)
            .map(|r| r.description.clone())
            .unwrap();
        if let Err(e) = check_role(desc.required_role, ctx) {
            return Err(error_message(
                msg.message_id.clone(),
                e.code().as_i32(),
                e.to_string(),
            ));
        }
        let args = msg.args.clone().unwrap_or(Value::Null);
        match handler.handle(ctx.clone(), args).await {
            Ok(value) => Ok(SuccessResultMessage {
                message_id: msg.message_id.clone(),
                result: Some(value),
                partial: false,
            }),
            Err(e) => Err(error_message(
                msg.message_id.clone(),
                e.code().as_i32(),
                e.to_string(),
            )),
        }
    }
}

fn check_role(
    required: RequiredRole,
    ctx: &CommandContext,
) -> std::result::Result<(), MusicAssistantError> {
    match required {
        RequiredRole::Anonymous => Ok(()),
        RequiredRole::Authenticated => {
            if ctx.user_id.is_none() {
                Err(MusicAssistantError::AuthenticationFailed)
            } else {
                Ok(())
            }
        }
        RequiredRole::Admin => match ctx.role {
            Some(UserRole::Admin) => Ok(()),
            _ => Err(MusicAssistantError::PermissionDenied),
        },
    }
}

/// Parse `args` into a typed struct, returning a clear error on failure.
pub fn parse_args<T: DeserializeOwned>(
    args: &Value,
) -> std::result::Result<T, MusicAssistantError> {
    serde_json::from_value::<T>(args.clone())
        .map_err(|e| MusicAssistantError::InvalidInput(format!("invalid arguments: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct EchoArgs {
        msg: String,
    }

    fn echo_handler() -> impl CommandHandler {
        |_ctx, args| async move {
            let a: EchoArgs = parse_args(&args)?;
            Ok(Value::String(a.msg))
        }
    }

    #[tokio::test]
    async fn register_and_dispatch() {
        let reg = CommandRegistry::new();
        reg.register(
            "echo",
            RequiredRole::Anonymous,
            vec!["msg".to_string()],
            Some("echo back a string".to_string()),
            echo_handler(),
        );
        let msg = CommandMessage {
            message_id: "1".to_string(),
            command: "echo".to_string(),
            args: Some(serde_json::json!({"msg": "hi"})),
        };
        let resp = reg
            .dispatch(&msg, &CommandContext::default())
            .await
            .unwrap();
        assert_eq!(resp.result.unwrap(), serde_json::json!("hi"));
    }

    #[tokio::test]
    async fn unknown_command_returns_invalid() {
        let reg = CommandRegistry::new();
        let msg = CommandMessage {
            message_id: "1".to_string(),
            command: "nope".to_string(),
            args: None,
        };
        let err = reg
            .dispatch(&msg, &CommandContext::default())
            .await
            .unwrap_err();
        assert_eq!(err.error_code, ErrorCode::InvalidCommand.as_i32());
    }

    #[tokio::test]
    async fn admin_role_required_blocks_user() {
        let reg = CommandRegistry::new();
        reg.register(
            "admin_only",
            RequiredRole::Admin,
            vec![],
            None,
            |_ctx, _args| async { Ok(Value::Null) },
        );
        let msg = CommandMessage {
            message_id: "1".to_string(),
            command: "admin_only".to_string(),
            args: None,
        };
        let err = reg
            .dispatch(
                &msg,
                &CommandContext {
                    role: Some(UserRole::User),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.error_code, ErrorCode::InsufficientPermissions.as_i32());
    }

    #[tokio::test]
    async fn authenticated_required_blocks_anon() {
        let reg = CommandRegistry::new();
        reg.register(
            "auth_only",
            RequiredRole::Authenticated,
            vec![],
            None,
            |_ctx, _args| async { Ok(Value::Null) },
        );
        let msg = CommandMessage {
            message_id: "1".to_string(),
            command: "auth_only".to_string(),
            args: None,
        };
        let err = reg
            .dispatch(&msg, &CommandContext::default())
            .await
            .unwrap_err();
        assert_eq!(err.error_code, ErrorCode::AuthenticationFailed.as_i32());
    }

    #[test]
    fn list_returns_all_commands() {
        let reg = CommandRegistry::new();
        reg.register("a", RequiredRole::Anonymous, vec![], None, |_, _| async {
            Ok(Value::Null)
        });
        reg.register("b", RequiredRole::Admin, vec![], None, |_, _| async {
            Ok(Value::Null)
        });
        let names: Vec<String> = reg.list().into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"b".to_string()));
    }
}
