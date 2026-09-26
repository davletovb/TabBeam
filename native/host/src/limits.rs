//! Bounds on everything the host accepts across the browser/native trust
//! boundary (SEC-01). `docs/protocol/native-messaging-v1.json` records the same
//! values for the extension, and tests on both sides keep them equal.

/// Largest Native Messaging payload the host reads or writes (1 MiB). Chrome
/// caps host-to-browser messages at the same size.
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// Deepest container nesting accepted in a request (protocol v1 §1 rule 11).
pub const MAX_JSON_DEPTH: usize = 128;

/// Longest request ID accepted, in characters (protocol v1 §2).
pub const MAX_REQUEST_ID_LENGTH: usize = 128;

/// Maximum normalized prior dialogue supplied with a follow-up.
pub const MAX_HISTORY_MESSAGES: usize = 32;
pub const MAX_HISTORY_BYTES: usize = 128 * 1024;

/// Browser-context limits mirrored from the extension's explicit capture
/// policy. The host enforces them again because browser messages are untrusted.
pub const MAX_SELECTION_BYTES: usize = 16 * 1024;
pub const MAX_PAGE_BYTES: usize = 64 * 1024;
pub const MAX_CONTEXT_TITLE_BYTES: usize = 1024;
pub const MAX_CONTEXT_URL_BYTES: usize = 2048;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::HOST_NAME;

    #[test]
    fn limits_match_the_shared_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/protocol/native-messaging-v1.json"
        ))
        .expect("the shared contract is JSON");

        assert_eq!(contract["max_frame_bytes"], MAX_FRAME_SIZE);
        assert_eq!(contract["max_json_depth"], MAX_JSON_DEPTH);
        assert_eq!(contract["max_request_id_length"], MAX_REQUEST_ID_LENGTH);
        assert_eq!(contract["max_history_messages"], MAX_HISTORY_MESSAGES);
        assert_eq!(contract["max_history_bytes"], MAX_HISTORY_BYTES);
        assert_eq!(contract["host_name"], HOST_NAME);
    }
}
