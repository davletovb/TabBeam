//! Provider-independent web search (SRCH-01..04).
//!
//! Search is deliberately separate from model execution. A search backend
//! returns normalized, bounded sources; the host then gives those sources to
//! whichever AI provider the conversation selected. Search text is always
//! treated as untrusted reference data.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::protocol::events::{ErrorBody, ErrorCode};
use crate::providers::environment;
use crate::providers::{BUSY_LIMIT, Exchange, Provider, SendRequest, Timeouts, Update};
use pervue_core::process::{Event as ProcessEvent, Process, ProcessSpec};
use pervue_core::protocol::Source;

/// Pseudo-backend routes used by the host before independent adapters.
pub const DEFAULT_BACKEND_ID: &str = "auto";
pub const PROVIDER_BACKEND_ID: &str = "provider";
pub const BRAVE_BACKEND_ID: &str = "brave";
pub const DEFAULT_RESULT_COUNT: usize = 8;
pub const MAX_RESULT_COUNT: usize = 10;

const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TITLE_BYTES: usize = 512;
const MAX_URL_BYTES: usize = 4096;
const MAX_SNIPPET_BYTES: usize = 4096;
const MAX_META_BYTES: usize = 256;
const HTTP_MARKER: &str = "\nPERVUE_HTTP_STATUS:";
const MAX_QUERY_CHARS: usize = 600;
const MAX_QUERY_WORDS: usize = 75;

const SEARCH_NOT_CONFIGURED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_NOT_CONFIGURED",
    message: "Brave Search is not configured. Add a Brave Search API key, then try again.",
    retryable: false,
};
const SEARCH_TOOL_NOT_FOUND: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_TRANSPORT_NOT_FOUND",
    message: "Pervue could not find the system HTTPS client used for web search.",
    retryable: false,
};
const SEARCH_START_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_BACKEND_UNAVAILABLE",
    message: "Web search could not start. Try again.",
    retryable: true,
};
const SEARCH_REQUEST_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_BACKEND_UNAVAILABLE",
    message: "Brave Search could not be reached. Try again.",
    retryable: true,
};
const SEARCH_RATE_LIMITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_RATE_LIMITED",
    message: "Brave Search is rate limited. Try again shortly.",
    retryable: true,
};
const SEARCH_AUTH_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_AUTHENTICATION_FAILED",
    message: "Brave Search rejected the configured API key.",
    retryable: false,
};
const SEARCH_RESPONSE_INVALID: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "MALFORMED_SEARCH_OUTPUT",
    message: "Brave Search returned a response Pervue could not understand.",
    retryable: false,
};
const SEARCH_RESPONSE_TOO_LARGE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_RESPONSE_TOO_LARGE",
    message: "The search response exceeded Pervue's safety limit.",
    retryable: true,
};

const SEARCH_QUERY_INVALID: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::SearchFailed,
    reason: "SEARCH_QUERY_INVALID",
    message: "The search query could not be accepted. Change the query and try again.",
    retryable: false,
};

/// Search settings carried by `conversation.send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    pub backend_id: String,
    pub count: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            backend_id: DEFAULT_BACKEND_ID.to_owned(),
            count: DEFAULT_RESULT_COUNT,
        }
    }
}

/// One provider-independent search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub count: usize,
}

pub type SharedResults = Rc<RefCell<Option<Vec<Source>>>>;

/// A running search plus the slot where it leaves normalized results.
pub struct SearchHandle {
    pub exchange: Box<dyn Exchange>,
    pub results: SharedResults,
}

/// A search backend. This contract is intentionally independent of the AI
/// provider contract: search returns sources, not model output.
pub trait SearchProvider {
    fn id(&self) -> &str;
    fn timeouts(&self) -> Timeouts;
    fn search(&self, request: SearchRequest) -> SearchHandle;
}

/// Registry of available search backends.
pub struct SearchProviders(Vec<Box<dyn SearchProvider>>);

impl SearchProviders {
    pub fn new(providers: Vec<Box<dyn SearchProvider>>) -> Self {
        Self(providers)
    }

    pub fn installed() -> Self {
        Self(vec![Box::new(Brave::installed())])
    }

    pub fn get(&self, id: &str) -> Option<&dyn SearchProvider> {
        self.0
            .iter()
            .map(AsRef::as_ref)
            .find(|provider| provider.id() == id)
    }
}

enum SynthesisStage {
    Searching {
        exchange: Box<dyn Exchange>,
        results: SharedResults,
    },
    Answering(Box<dyn Exchange>),
    Done,
}

/// One provider-neutral search -> synthesis request. Retrieval completes
/// before model execution starts. The model receives the same normalized
/// sources that are emitted to the browser.
pub struct SynthesisExchange {
    stage: SynthesisStage,
    provider: Option<Rc<dyn Provider>>,
    provider_timeouts: Timeouts,
    request: Option<SendRequest>,
    sources: VecDeque<Source>,
    announce_sources: bool,
}

impl SynthesisExchange {
    pub fn new(
        handle: SearchHandle,
        provider: Rc<dyn Provider>,
        provider_timeouts: Timeouts,
        request: SendRequest,
    ) -> Self {
        Self {
            stage: SynthesisStage::Searching {
                exchange: handle.exchange,
                results: handle.results,
            },
            provider: Some(provider),
            provider_timeouts,
            request: Some(request),
            sources: VecDeque::new(),
            announce_sources: false,
        }
    }
}

impl Exchange for SynthesisExchange {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        if self.announce_sources {
            if let Some(source) = self.sources.pop_front() {
                return Some(Update::Source(source));
            }
            self.announce_sources = false;
        }

        match &mut self.stage {
            SynthesisStage::Searching { exchange, results } => {
                let update = exchange.next(deadline)?;
                match update {
                    Update::Completed => {
                        let found = results.borrow_mut().take().unwrap_or_default();
                        self.sources = found.iter().cloned().collect();
                        let mut request = self.request.take().expect("search request exists");
                        request.search_results = Some(found);
                        let provider = self.provider.take().expect("provider exists");
                        self.stage = SynthesisStage::Answering(provider.send(request));
                        Some(Update::ResetTimeouts(self.provider_timeouts))
                    }
                    Update::Failed(error) => {
                        self.stage = SynthesisStage::Done;
                        Some(Update::Failed(error))
                    }
                    Update::Stopped => {
                        self.stage = SynthesisStage::Done;
                        Some(Update::Stopped)
                    }
                    Update::Activity => Some(Update::Activity),
                    // A search backend produces sources in its result slot,
                    // not provider/conversation events.
                    _ => {
                        self.stage = SynthesisStage::Done;
                        Some(Update::Failed(SEARCH_RESPONSE_INVALID))
                    }
                }
            }
            SynthesisStage::Answering(exchange) => {
                let update = exchange.next(deadline)?;
                if matches!(update, Update::Started { .. }) {
                    self.announce_sources = true;
                }
                if update.is_terminal() {
                    self.stage = SynthesisStage::Done;
                }
                Some(update)
            }
            SynthesisStage::Done => None,
        }
    }

    fn cancel(&mut self, grace: Duration) {
        match &mut self.stage {
            SynthesisStage::Searching { exchange, .. } | SynthesisStage::Answering(exchange) => {
                exchange.cancel(grace);
            }
            SynthesisStage::Done => {}
        }
    }
}

/// Brave Search API backend. The API key remains in the native process and is
/// sent to curl through stdin, never argv, logs, extension storage, or model
/// provider environments.
pub struct Brave {
    curl: Option<PathBuf>,
    api_key: Option<String>,
    inherited: Vec<(OsString, OsString)>,
    timeouts: Timeouts,
}

impl Brave {
    pub fn installed() -> Self {
        let curl = configured_curl();
        let api_key = std::env::var("BRAVE_SEARCH_API_KEY")
            .ok()
            .filter(|key| !key.is_empty() && !key.contains(['\r', '\n']));
        Self {
            curl,
            api_key,
            inherited: environment::inherit(std::env::vars_os(), &[]),
            timeouts: Timeouts {
                start: Duration::from_secs(20),
                idle: Duration::from_secs(20),
                stop_grace: Duration::from_secs(1),
            },
        }
    }

    #[cfg(test)]
    fn with_transport(curl: Option<PathBuf>, api_key: Option<String>) -> Self {
        Self {
            curl,
            api_key,
            inherited: Vec::new(),
            timeouts: Timeouts {
                start: Duration::from_secs(2),
                idle: Duration::from_secs(2),
                stop_grace: Duration::from_millis(100),
            },
        }
    }
}

impl SearchProvider for Brave {
    fn id(&self) -> &str {
        BRAVE_BACKEND_ID
    }

    fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    fn search(&self, request: SearchRequest) -> SearchHandle {
        let results = Rc::new(RefCell::new(None));
        let Some(curl) = self.curl.as_ref() else {
            return failed_handle(results, SEARCH_TOOL_NOT_FOUND);
        };
        let Some(api_key) = self.api_key.as_ref() else {
            return failed_handle(results, SEARCH_NOT_CONFIGURED);
        };

        let query = bounded_brave_query(&request.query);
        if query.is_empty() {
            return failed_handle(results, SEARCH_QUERY_INVALID);
        }
        let count = request.count.clamp(1, MAX_RESULT_COUNT);
        // Neither the API key nor the user's query is placed in argv. They go
        // through curl's stdin config, so local process listings do not expose
        // either value.
        let spec = ProcessSpec::new(curl)
            .args([
                "--disable",
                "--silent",
                "--show-error",
                "--get",
                "--proto",
                "=https",
                "--connect-timeout",
                "5",
                "--max-time",
                "15",
                "--write-out",
            ])
            .arg(format!("{HTTP_MARKER}%{{http_code}}"))
            .arg("--config")
            .arg("-")
            .envs(self.inherited.iter().cloned());

        let Ok(mut process) = Process::spawn(&spec) else {
            return failed_handle(results, SEARCH_START_FAILED);
        };
        let url = brave_url(&query, count);
        let config = format!(
            "url = \"{}\"\nheader = \"X-Subscription-Token: {}\"\nheader = \"Accept: application/json\"\n",
            curl_config_escape(&url),
            curl_config_escape(api_key)
        );
        if process.write(config.as_bytes()).is_err() {
            process.kill();
            return failed_handle(results, SEARCH_START_FAILED);
        }
        process.close_stdin();

        SearchHandle {
            exchange: Box::new(BraveExchange {
                process: Some(process),
                stdout: Vec::new(),
                results: Rc::clone(&results),
                stopping_at: None,
            }),
            results,
        }
    }
}

fn failed_handle(results: SharedResults, error: ErrorBody<'static>) -> SearchHandle {
    SearchHandle {
        exchange: Box::new(crate::providers::Scripted::failed(error)),
        results,
    }
}

fn configured_curl() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("PERVUE_CURL_PATH") {
        let path = PathBuf::from(configured);
        return (path.is_absolute() && path.is_file()).then_some(path);
    }
    system_curl()
}

#[cfg(unix)]
fn system_curl() -> Option<PathBuf> {
    let path = PathBuf::from("/usr/bin/curl");
    path.is_file().then_some(path)
}

#[cfg(not(unix))]
fn system_curl() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot")?;
    let path = PathBuf::from(root).join("System32").join("curl.exe");
    path.is_file().then_some(path)
}

fn curl_config_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn brave_url(query: &str, count: usize) -> String {
    format!(
        "{BRAVE_URL}?q={}&count={}&text_decorations=false",
        percent_encode(query.as_bytes()),
        count.clamp(1, MAX_RESULT_COUNT)
    )
}

fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

fn bounded_brave_query(query: &str) -> String {
    let mut out = String::new();
    for word in query.split_whitespace().take(MAX_QUERY_WORDS) {
        let used = out.chars().count();
        let separator = usize::from(!out.is_empty());
        let remaining = MAX_QUERY_CHARS.saturating_sub(used + separator);
        if remaining == 0 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        let mut chars = word.chars();
        out.extend(chars.by_ref().take(remaining));
        if chars.next().is_some() {
            break;
        }
    }
    out
}

struct BraveExchange {
    process: Option<Process>,
    stdout: Vec<u8>,
    results: SharedResults,
    stopping_at: Option<Instant>,
}

impl Exchange for BraveExchange {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        let busy_until = deadline.max(
            Instant::now()
                .checked_add(BUSY_LIMIT)
                .unwrap_or_else(Instant::now),
        );
        loop {
            let process = self.process.as_mut()?;
            if let Some(kill_at) = self.stopping_at {
                if Instant::now() >= kill_at {
                    process.kill();
                    self.process = None;
                    self.stopping_at = None;
                    return Some(Update::Stopped);
                }
                match process.next_event(deadline.min(kill_at)) {
                    Some(ProcessEvent::Exited(_)) => {
                        self.process = None;
                        self.stopping_at = None;
                        return Some(Update::Stopped);
                    }
                    Some(_) => {}
                    None if Instant::now() >= kill_at => continue,
                    None => return None,
                }
                if Instant::now() >= busy_until {
                    return None;
                }
                continue;
            }

            match process.next_event(deadline)? {
                ProcessEvent::Stdout(bytes) => {
                    if self.stdout.len().saturating_add(bytes.len()) > MAX_RESPONSE_BYTES {
                        process.kill();
                        self.process = None;
                        return Some(Update::Failed(SEARCH_RESPONSE_TOO_LARGE));
                    }
                    self.stdout.extend_from_slice(&bytes);
                }
                ProcessEvent::Stderr(_) => {}
                ProcessEvent::Exited(exit) => {
                    self.process = None;
                    if !exit.status.is_some_and(|status| status.success()) {
                        return Some(Update::Failed(SEARCH_REQUEST_FAILED));
                    }
                    let (body, status) = match split_http_status(&self.stdout) {
                        Some(value) => value,
                        None => return Some(Update::Failed(SEARCH_RESPONSE_INVALID)),
                    };
                    if status == 429 {
                        return Some(Update::Failed(SEARCH_RATE_LIMITED));
                    }
                    if matches!(status, 401 | 403) {
                        return Some(Update::Failed(SEARCH_AUTH_FAILED));
                    }
                    if status == 422 {
                        return Some(Update::Failed(SEARCH_QUERY_INVALID));
                    }
                    if !(200..300).contains(&status) {
                        return Some(Update::Failed(SEARCH_REQUEST_FAILED));
                    }
                    let normalized = match normalize_brave(body) {
                        Ok(results) => results,
                        Err(()) => return Some(Update::Failed(SEARCH_RESPONSE_INVALID)),
                    };
                    *self.results.borrow_mut() = Some(normalized);
                    return Some(Update::Completed);
                }
            }
            if Instant::now() >= busy_until {
                return None;
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        if self.stopping_at.is_some() {
            return;
        }
        let Some(process) = self.process.as_mut() else {
            return;
        };
        process.request_stop();
        let now = Instant::now();
        self.stopping_at = Some(now.checked_add(grace).unwrap_or(now));
    }
}

fn split_http_status(stdout: &[u8]) -> Option<(&[u8], u16)> {
    let marker = HTTP_MARKER.as_bytes();
    let position = stdout
        .windows(marker.len())
        .rposition(|window| window == marker)?;
    let status = std::str::from_utf8(&stdout[position + marker.len()..])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((&stdout[..position], status))
}

#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveItem>,
}

#[derive(Deserialize)]
struct BraveItem {
    title: Option<String>,
    url: Option<String>,
    description: Option<String>,
    page_age: Option<String>,
    profile: Option<BraveProfile>,
}

#[derive(Deserialize)]
struct BraveProfile {
    name: Option<String>,
    long_name: Option<String>,
}

fn normalize_brave(body: &[u8]) -> Result<Vec<Source>, ()> {
    let response: BraveResponse = serde_json::from_slice(body).map_err(|_| ())?;
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item in response.web.map_or_else(Vec::new, |web| web.results) {
        if normalized.len() >= MAX_RESULT_COUNT {
            break;
        }
        let (Some(title), Some(url)) = (item.title.as_deref(), item.url.as_deref()) else {
            continue;
        };
        if title.trim().is_empty() || !safe_http_url(url) {
            continue;
        }
        let url = bounded(url, MAX_URL_BYTES);
        if !seen.insert(url.clone()) {
            continue;
        }
        let source_name = item
            .profile
            .and_then(|profile| profile.long_name.or(profile.name))
            .map(|value| bounded(&plain_text(&value), MAX_META_BYTES))
            .filter(|value| !value.is_empty());
        let age = item
            .page_age
            .map(|value| bounded(&plain_text(&value), MAX_META_BYTES))
            .filter(|value| !value.is_empty());
        normalized.push(Source {
            id: format!("src_search_{}", normalized.len() + 1),
            backend_id: BRAVE_BACKEND_ID.to_owned(),
            title: bounded(&plain_text(title), MAX_TITLE_BYTES),
            url,
            snippet: bounded(
                &plain_text(item.description.as_deref().unwrap_or_default()),
                MAX_SNIPPET_BYTES,
            ),
            source_name,
            age,
        });
    }
    Ok(normalized)
}

fn safe_http_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        && url.len() <= MAX_URL_BYTES
}

fn plain_text(value: &str) -> String {
    // Brave is requested with text_decorations=false, so angle brackets are
    // content, not highlighting markup. Decode entities once and preserve
    // code-like text such as Vec<String> verbatim.
    decode_entities(value)
}

fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut offset = 0;
    while offset < value.len() {
        let rest = &value[offset..];
        let entity = [
            ("&quot;", "\""),
            ("&#39;", "'"),
            ("&#x27;", "'"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&amp;", "&"),
        ]
        .into_iter()
        .find(|(encoded, _)| rest.starts_with(encoded));
        if let Some((encoded, decoded)) = entity {
            output.push_str(decoded);
            offset += encoded.len();
            continue;
        }
        let character = rest.chars().next().expect("offset is in bounds");
        output.push(character);
        offset += character.len_utf8();
    }
    output
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
    fn brave_results_are_normalized_bounded_and_deduplicated() {
        let body = br#"{
          "web":{"results":[
            {"title":"One result","url":"https://example.com/a","description":"A &amp; B","page_age":"2 days ago","profile":{"long_name":"example.com"}},
            {"title":"Duplicate","url":"https://example.com/a","description":"ignored"},
            {"title":"Unsafe","url":"javascript:alert(1)","description":"ignored"},
            {"title":"Two","url":"http://example.org/b","description":null}
          ]}
        }"#;
        let results = normalize_brave(body).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "src_search_1");
        assert_eq!(results[0].title, "One result");
        assert_eq!(results[0].snippet, "A & B");
        assert_eq!(results[0].source_name.as_deref(), Some("example.com"));
        assert_eq!(results[1].id, "src_search_2");
    }

    #[test]
    fn brave_url_percent_encodes_query_for_stdin_config() {
        let url = brave_url("rust & café?", 8);
        assert_eq!(
            url,
            "https://api.search.brave.com/res/v1/web/search?q=rust%20%26%20caf%C3%A9%3F&count=8&text_decorations=false"
        );
        assert!(!url.contains(' '));
    }

    #[test]
    fn brave_query_obeys_api_word_and_character_bounds() {
        let long = std::iter::repeat_n("abcdefghij", 100)
            .collect::<Vec<_>>()
            .join(" ");
        let query = bounded_brave_query(&long);
        assert!(query.split_whitespace().count() <= 75);
        assert!(query.chars().count() <= 600);
    }

    #[test]
    fn long_first_token_is_truncated_instead_of_dropped() {
        let query = bounded_brave_query(&"界".repeat(700));
        assert_eq!(query.chars().count(), MAX_QUERY_CHARS);
        assert!(!query.is_empty());
    }

    #[test]
    fn plain_text_keeps_comparisons_and_decodes_entities_once() {
        assert_eq!(
            plain_text("Rust 1.80 < 1.81 adds X"),
            "Rust 1.80 < 1.81 adds X"
        );
        assert_eq!(
            plain_text("Use Vec<String> &amp; HashMap<K, V>"),
            "Use Vec<String> & HashMap<K, V>"
        );
        assert_eq!(plain_text("&amp;lt;script&amp;gt;"), "&lt;script&gt;");
    }

    #[test]
    fn incomplete_brave_items_are_skipped_not_fatal() {
        let body = br#"{"web":{"results":[
            {"title":null,"url":"https://bad.example/","description":"skip"},
            {"title":"No URL","description":"skip"},
            {"title":"Good","url":"https://example.com/","description":"ok"}
        ]}}"#;
        let results = normalize_brave(body).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Good");
    }

    #[test]
    fn missing_transport_or_key_fails_as_search_not_provider() {
        let brave = Brave::with_transport(None, Some("key".to_owned()));
        let mut handle = brave.search(SearchRequest {
            query: "hello".to_owned(),
            count: 5,
        });
        assert!(matches!(
            handle.exchange.next(Instant::now()),
            Some(Update::Failed(error)) if error.code == ErrorCode::SearchFailed
        ));
    }
}
