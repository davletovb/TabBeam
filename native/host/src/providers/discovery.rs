//! Host policy for provider executable discovery. The reusable directory
//! search lives in `runtime-core`; adapters use this entry point so every
//! installed provider honors the same override, including hermetic tests.

use runtime_core::discovery::SearchPath;

/// Host-specific override for the generic search path.
pub const SEARCH_PATH_VARIABLE: &str = "PERVUE_PROVIDER_PATH";

/// Search locations for an installed adapter, with the host's override.
pub fn installed() -> SearchPath {
    SearchPath::from_env(SEARCH_PATH_VARIABLE)
}
