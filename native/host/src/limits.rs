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
        assert_eq!(contract["host_name"], HOST_NAME);
    }
}
