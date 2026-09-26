//! Normalized errors, capabilities and provider status shared by adapters.

use serde::{Serialize, Serializer};

/// Normalized error categories (DOC-02 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    HostNotInstalled,
    HostUnavailable,
    ProviderNotFound,
    ProviderNotAuthenticated,
    ProviderFailed,
    RequestCancelled,
    RequestTimeout,
    ContextUnavailable,
    InvalidRequest,
    InternalError,
}

/// The `error` object of a `response.failed` event (DOC-02 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ErrorBody<'a> {
    pub code: ErrorCode,
    pub reason: &'a str,
    pub message: &'a str,
    pub retryable: bool,
}

/// Provider availability (DOC-02 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    NotFound,
    Unknown,
}

/// Provider authentication state (DOC-02 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    Authenticated,
    Unauthenticated,
    Unknown,
}

/// A capability value, serialized as `true`, `false`, or `"unknown"` (DOC-02 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Supported,
    Unsupported,
    Unknown,
}

impl Serialize for Capability {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Supported => serializer.serialize_bool(true),
            Self::Unsupported => serializer.serialize_bool(false),
            Self::Unknown => serializer.serialize_str("unknown"),
        }
    }
}

/// The v1 capability set (DOC-02 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub streaming: Capability,
    pub continuation: Capability,
    pub web_search: Capability,
    pub page_context: Capability,
    pub attachments: Capability,
    pub model_selection: Capability,
    pub cancellation: Capability,
}

/// A model an adapter suggests (`status.models`). Suggestions, not the
/// complete set: a provider may accept other valid model IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelOption {
    /// What `conversation.send` passes as `model`.
    pub id: &'static str,
    /// How the extension names it.
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderState {
    pub availability: Availability,
    pub authentication: Authentication,
    pub capabilities: Capabilities,
    /// Suggested models, when `model_selection` is supported. Omitted when
    /// empty; an adapter that can take any model ID may suggest none.
    #[serde(skip_serializing_if = "<[ModelOption]>::is_empty")]
    pub models: &'static [ModelOption],
}
