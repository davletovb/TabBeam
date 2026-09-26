//! Compatibility path for platform executable lookup; owned by pervue-core.
pub use pervue_core::discovery::*;

/// Host-specific override for the generic search path.
pub const SEARCH_PATH_VARIABLE: &str = "PERVUE_PROVIDER_PATH";
