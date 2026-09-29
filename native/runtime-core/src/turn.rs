//! Neutral turn contract shared by applications and provider adapters.

use std::fmt;

use serde::Serialize;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    None,
    NativeWebSearch,
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
    /// Whether the caller requires a fresh sign-in classification before the
    /// provider process starts.
    pub check_sign_in: bool,
}

impl Turn {
    pub fn validate(&self) -> Result<(), TurnError> {
        if self.messages.is_empty() {
            return Err(TurnError::NoMessages);
        }
        if self
            .messages
            .iter()
            .any(|message| message.text.as_bytes().contains(&0))
        {
            return Err(TurnError::InvalidText);
        }
        if self.continuation.is_some() && self.session != SessionPolicy::Persistent {
            return Err(TurnError::ContinuationRequiresPersistentSession);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnError {
    NoMessages,
    InvalidText,
    ContinuationRequiresPersistentSession,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Namespace(String);

impl Namespace {
    pub fn fixed(value: impl Into<String>) -> Result<Self, NamespaceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 64
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_')
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
        assert!(Usage {
            input_tokens: Some(10),
            output_tokens: Some(4),
        }
        .is_monotonic_after(first));
        assert!(!Usage {
            input_tokens: Some(9),
            output_tokens: Some(4),
        }
        .is_monotonic_after(first));
    }

    #[test]
    fn namespaces_are_fixed_safe_path_components() {
        assert_eq!(Namespace::fixed("pervue").unwrap().as_str(), "pervue");
        for value in ["", "Pervue", "../pervue", "per vue"] {
            assert!(Namespace::fixed(value).is_err(), "{value}");
        }
    }
}
