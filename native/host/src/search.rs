//! Pervue's side of web search: the request carries search intent, and the
//! provider's own search does the rest. Normalizing what the provider returns
//! belongs to the runtime ([`seatline_core::search`]).

/// Validated search intent carried by `conversation.send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions;

pub const DEFAULT_BACKEND_ID: &str = "auto";
pub const PROVIDER_BACKEND_ID: &str = "provider";

#[cfg(test)]
mod tests {
    use seatline_core::search::valid_source_url;

    /// The host accepts only what the browser also accepts, so a source it
    /// counts toward grounding is one the extension keeps.
    #[test]
    fn source_urls_match_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/protocol/fixtures/v1-source-urls.json"
        ))
        .unwrap();
        for url in fixture["accepted"].as_array().unwrap() {
            let url = url.as_str().unwrap();
            assert!(valid_source_url(url), "{url}");
        }
        for url in fixture["rejected"].as_array().unwrap() {
            let url = url.as_str().unwrap();
            assert!(!valid_source_url(url), "{url}");
        }
    }
}
