//! Neutral turn contract shared by applications and provider adapters.

use std::fmt;

use serde::Serialize;

pub const MAX_MODEL_ID_BYTES: usize = 128;
pub const MAX_MODEL_LABEL_BYTES: usize = 64;
pub const MAX_MODEL_OPTIONS: usize = 32;
pub const MAX_CONTINUATION_BYTES: usize = 256;
pub const MAX_CLEANUP_GROUP_BYTES: usize = 64;

pub fn is_model_id(model: &str) -> bool {
    let bytes = model.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_MODEL_ID_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || b"._-:/@".contains(&byte))
}

/// Whether `value` can be a native-session handle: opaque to the runtime, and
/// safe to keep in a file name and to pass as one command-line argument.
pub fn is_session_handle(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_CONTINUATION_BYTES
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// What the provider may do besides answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    /// No tools at all. For turns whose text the application doesn't control:
    /// it can only inform the answer, never make the provider act. An adapter
    /// that can't guarantee this (see `Capabilities::tool_isolation`) refuses
    /// the turn rather than running it with tools.
    None,
    /// No tools except the provider's own web search.
    NativeWebSearch,
    /// The provider's own configuration decides. For turns whose text the
    /// application wrote itself, so the user's usual provider setup applies.
    ProviderDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPolicy {
    Ephemeral,
    Persistent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub model: Option<String>,
    pub tools: ToolPolicy,
    pub session: SessionPolicy,
    pub continuation: Option<String>,
    /// Groups this turn's per-turn cleanup records with the others of the same
    /// group, so the application can retry a group's failed deletions
    /// together (Pervue: one conversation). Opaque to the runtime.
    pub cleanup_group: Option<String>,
    /// Whether the caller requires a fresh sign-in classification before the
    /// provider process starts.
    pub check_sign_in: bool,
}

/// Whether `value` can name a cleanup group: a file-name-safe token.
pub fn is_cleanup_group(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_CLEANUP_GROUP_BYTES
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl Turn {
    pub fn validate(&self) -> Result<(), TurnError> {
        if self.messages.is_empty() {
            return Err(TurnError::NoMessages);
        }
        if self
            .system
            .as_ref()
            .is_some_and(|text| text.as_bytes().contains(&0))
            || self
                .messages
                .iter()
                .any(|message| message.text.as_bytes().contains(&0))
        {
            return Err(TurnError::InvalidText);
        }
        if self
            .model
            .as_deref()
            .is_some_and(|model| !is_model_id(model))
        {
            return Err(TurnError::InvalidModel);
        }
        if self
            .continuation
            .as_deref()
            .is_some_and(|continuation| !is_session_handle(continuation))
        {
            return Err(TurnError::InvalidContinuation);
        }
        if self.continuation.is_some() && self.session != SessionPolicy::Persistent {
            return Err(TurnError::ContinuationRequiresPersistentSession);
        }
        if self
            .cleanup_group
            .as_deref()
            .is_some_and(|group| !is_cleanup_group(group))
        {
            return Err(TurnError::InvalidCleanupGroup);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnError {
    NoMessages,
    InvalidText,
    InvalidModel,
    InvalidContinuation,
    ContinuationRequiresPersistentSession,
    InvalidCleanupGroup,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl Usage {
    pub fn is_monotonic_after(self, previous: Self) -> bool {
        non_decreasing(previous.input_tokens, self.input_tokens)
            && non_decreasing(previous.output_tokens, self.output_tokens)
    }
}

fn non_decreasing(previous: Option<u64>, next: Option<u64>) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => next >= previous,
        (Some(_), None) => false,
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInClassification {
    Subscription,
    ApiKey,
    Cloud,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Namespace(String);

impl Namespace {
    pub fn fixed(value: impl Into<String>) -> Result<Self, NamespaceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(NamespaceError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceError;

impl fmt::Display for NamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("namespace must be 1-64 lowercase ASCII letters, digits, '-' or '_'")
    }
}

impl std::error::Error for NamespaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_continuation_requires_persistence() {
        let turn = Turn {
            system: None,
            messages: vec![Message {
                role: Role::User,
                text: "hello".to_owned(),
            }],
            model: None,
            tools: ToolPolicy::None,
            session: SessionPolicy::Ephemeral,
            continuation: Some("opaque".to_owned()),
            cleanup_group: None,
            check_sign_in: true,
        };
        assert_eq!(
            turn.validate(),
            Err(TurnError::ContinuationRequiresPersistentSession)
        );
    }

    #[test]
    fn usage_snapshots_never_go_backwards() {
        let first = Usage {
            input_tokens: Some(10),
            output_tokens: Some(3),
        };
        assert!(
            Usage {
                input_tokens: Some(10),
                output_tokens: Some(4),
            }
            .is_monotonic_after(first)
        );
        assert!(
            !Usage {
                input_tokens: Some(9),
                output_tokens: Some(4),
            }
            .is_monotonic_after(first)
        );
    }

    #[test]
    fn namespaces_are_fixed_safe_path_components() {
        assert_eq!(Namespace::fixed("pervue").unwrap().as_str(), "pervue");
        for value in ["", "Pervue", "../pervue", "per vue"] {
            assert!(Namespace::fixed(value).is_err(), "{value}");
        }
    }
    #[test]
    fn argv_bound_fields_are_validated() {
        let base = Turn {
            system: None,
            messages: vec![Message {
                role: Role::User,
                text: "hello".to_owned(),
            }],
            model: None,
            tools: ToolPolicy::None,
            session: SessionPolicy::Persistent,
            continuation: None,
            cleanup_group: None,
            check_sign_in: false,
        };
        assert!(
            Turn {
                model: Some("--help".to_owned()),
                ..base.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Turn {
                continuation: Some("--resume".to_owned()),
                ..base.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Turn {
                system: Some("bad\0system".to_owned()),
                ..base.clone()
            }
            .validate()
            .is_err()
        );
        for group in ["", "../up", "has space", &"x".repeat(65)] {
            assert_eq!(
                Turn {
                    cleanup_group: Some(group.to_owned()),
                    ..base.clone()
                }
                .validate(),
                Err(TurnError::InvalidCleanupGroup),
                "{group:?}"
            );
        }
        assert!(
            Turn {
                cleanup_group: Some("conv_0123456789abcdef".to_owned()),
                ..base
            }
            .validate()
            .is_ok()
        );
    }
}
