//! Provider-native web-search request and source normalization (SRCH-01..04).
//!
//! Pervue does not run a separate search service. The selected authenticated
//! AI provider performs search, then its provider-specific result format is
//! normalized here before sources reach the browser.

use std::collections::HashSet;

use pervue_core::protocol::{ErrorBody, ErrorCode, Source};
use serde::Deserialize;

pub const DEFAULT_BACKEND_ID: &str = "auto";
pub const PROVIDER_BACKEND_ID: &str = "provider";
pub const MAX_SOURCES_PER_TURN: usize = 20;

pub const NATIVE_SEARCH_NO_SOURCES: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "NATIVE_SEARCH_NO_SOURCES",
    message: "The provider finished web search without returning usable sources. Try again or update the provider.",
    retryable: true,
};

const MAX_TITLE_BYTES: usize = 512;
const MAX_URL_BYTES: usize = 4096;
const MAX_SNIPPET_BYTES: usize = 4096;
const MAX_META_BYTES: usize = 256;

/// Validated search intent carried by `conversation.send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions;

/// One provider-native search result before it receives Pervue source identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source_name: Option<String>,
    pub age: Option<String>,
}

impl SearchResult {
    pub fn new(
        title: &str,
        url: &str,
        snippet: &str,
        source_name: Option<&str>,
        age: Option<&str>,
    ) -> Option<Self> {
        if !valid_source_url(url) {
            return None;
        }
        let title = title.trim();
        if title.is_empty() {
            return None;
        }
        Some(Self {
            title: bounded(title, MAX_TITLE_BYTES),
            url: bounded(url, MAX_URL_BYTES),
            snippet: bounded(snippet, MAX_SNIPPET_BYTES),
            source_name: source_name
                .map(|value| bounded(value.trim(), MAX_META_BYTES))
                .filter(|value| !value.is_empty()),
            age: age
                .map(|value| bounded(value.trim(), MAX_META_BYTES))
                .filter(|value| !value.is_empty()),
        })
    }
}

/// Per-turn source identity, deduplication and storage bound shared by every
/// provider adapter.
pub struct SourceCollector {
    backend_id: &'static str,
    seen: HashSet<String>,
    count: usize,
}

impl SourceCollector {
    pub fn new(backend_id: &'static str) -> Self {
        Self {
            backend_id,
            seen: HashSet::new(),
            count: 0,
        }
    }

    pub fn push(&mut self, result: SearchResult) -> Option<Source> {
        if self.count >= MAX_SOURCES_PER_TURN || !self.seen.insert(result.url.clone()) {
            return None;
        }
        self.count += 1;
        Some(Source {
            id: format!("src_{}_{}", self.backend_id, self.count),
            backend_id: self.backend_id.to_owned(),
            title: result.title,
            url: result.url,
            snippet: result.snippet,
            source_name: result.source_name,
            age: result.age,
        })
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn reset(&mut self) {
        self.seen.clear();
        self.count = 0;
    }
}

/// Claude Code 2.1.x returns WebSearch results as a text `tool_result` ending
/// in `Links: [{...}]`. Parse only that JSON array; the prose before it stays
/// provider output and is never treated as data.
pub fn claude_tool_result_sources(content: &str) -> Vec<SearchResult> {
    #[derive(Deserialize)]
    struct Link {
        title: String,
        url: String,
        #[serde(default)]
        snippet: String,
    }

    let Some(marker) = content.rfind("Links:") else {
        return Vec::new();
    };
    let json = content[marker + "Links:".len()..].trim();
    let Ok(links) = serde_json::from_str::<Vec<Link>>(json) else {
        return Vec::new();
    };
    links
        .into_iter()
        .filter_map(|link| SearchResult::new(&link.title, &link.url, &link.snippet, None, None))
        .take(MAX_SOURCES_PER_TURN)
        .collect()
}

/// Codex exec currently reports only the search query in its `web_search`
/// item. Grounding URLs live in the answer text, so search turns normalize
/// Markdown links and bare HTTP(S) URLs from completed agent messages.
pub fn codex_message_sources(text: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    let bytes = text.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() && results.len() < MAX_SOURCES_PER_TURN {
        let Some(open_rel) = text[offset..].find('[') else {
            break;
        };
        let open = offset + open_rel;
        let Some(close_rel) = text[open + 1..].find(']') else {
            break;
        };
        let close = open + 1 + close_rel;
        if text.as_bytes().get(close + 1) != Some(&b'(') {
            offset = close + 1;
            continue;
        }
        let Some(end_rel) = text[close + 2..].find(')') else {
            break;
        };
        let end = close + 2 + end_rel;
        let title = &text[open + 1..close];
        let url = text[close + 2..end].trim().trim_matches(['<', '>']);
        if seen.insert(url.to_owned()) {
            if let Some(result) = SearchResult::new(title, url, "", None, None) {
                results.push(result);
            }
        }
        offset = end + 1;
    }

    // Some Codex answers cite a URL without Markdown. Keep those too.
    for token in text.split_whitespace() {
        if results.len() >= MAX_SOURCES_PER_TURN {
            break;
        }
        let url = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '\'' | ',' | ';' | '.'
            )
        });
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        if seen.insert(url.to_owned()) {
            if let Some(result) = SearchResult::new(url, url, "", None, None) {
                results.push(result);
            }
        }
    }
    results
}

fn valid_source_url(url: &str) -> bool {
    if url.len() > MAX_URL_BYTES
        || url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !matches!(scheme, "http" | "https") || rest.is_empty() {
        return false;
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    !authority.is_empty() && !authority.contains('@')
}

fn bounded(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_urls_require_http_authority_and_no_credentials() {
        for valid in [
            "https://example.com/",
            "http://example.test/path?q=1#x",
        ] {
            assert!(valid_source_url(valid), "{valid}");
        }
        for invalid in [
            "https://",
            "file:///tmp/x",
            "javascript:alert(1)",
            "https://user@example.com/",
            "https://example.com/has space",
        ] {
            assert!(!valid_source_url(invalid), "{invalid}");
        }
    }

    #[test]
    fn claude_links_payload_is_normalized() {
        let results = claude_tool_result_sources(
            "Web search results for query: \"rust\"\n\nLinks: [{\"title\":\"Rust\",\"url\":\"https://www.rust-lang.org/\"},{\"title\":\"Bad\",\"url\":\"https://\"}]",
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
    }

    #[test]
    fn codex_extracts_markdown_and_bare_urls() {
        let results = codex_message_sources(
            "See [Rust blog](https://blog.rust-lang.org/2026/09/01/release.html) and https://example.com/more.",
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust blog");
        assert_eq!(results[1].url, "https://example.com/more");
    }

    #[test]
    fn collector_deduplicates_and_caps_sources() {
        let mut collector = SourceCollector::new("test");
        for index in 0..(MAX_SOURCES_PER_TURN + 5) {
            let result = SearchResult::new(
                &format!("Result {index}"),
                &format!("https://example.com/{index}"),
                "",
                None,
                None,
            )
            .unwrap();
            let _ = collector.push(result);
        }
        assert_eq!(collector.count(), MAX_SOURCES_PER_TURN);
        assert!(collector
            .push(SearchResult::new("Again", "https://example.com/1", "", None, None).unwrap())
            .is_none());
        collector.reset();
        assert_eq!(collector.count(), 0);
    }
}
