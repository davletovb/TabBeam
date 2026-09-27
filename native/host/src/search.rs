//! Provider-native web-search request options (SRCH-01..04).
//!
//! Pervue does not run a separate search service. A search turn asks the
//! selected authenticated AI provider to use its native web-search capability;
//! provider-specific result formats are normalized back into protocol sources.

pub const DEFAULT_BACKEND_ID: &str = "auto";
pub const PROVIDER_BACKEND_ID: &str = "provider";

/// Search settings carried by `conversation.send`.
///
/// `auto` and `provider` currently mean the same thing: use the selected
/// provider's native authenticated web search. Keeping the explicit route name
/// lets the wire contract grow later without committing the app to an external
/// API-key search provider today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub backend_id: String,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            backend_id: DEFAULT_BACKEND_ID.to_owned(),
        }
    }
}
