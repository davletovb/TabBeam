//! Claude Code stream-json output reduced to provider-neutral events.

use serde_json::Value;

use crate::protocol::events::{ErrorBody, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Init(String),
    MessageStart,
    TextDelta(String),
    WebSearch(Vec<SearchResult>),
    Progress,
    ResultSuccess {
        session_id: Option<String>,
        text: String,
    },
    ResultFailed(ErrorBody<'static>),
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source_name: Option<String>,
    pub age: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

const MAX_SESSION_ID_LENGTH: usize = 128;

pub fn is_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_SESSION_ID_LENGTH
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn parse(line: &str) -> Result<Line, Malformed> {
    if line.trim().is_empty() {
        return Ok(Line::Ignored);
    }
    let event: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let kind = event.get("type").and_then(Value::as_str).ok_or(Malformed)?;
    Ok(match kind {
        "system" if event.get("subtype").and_then(Value::as_str) == Some("init") => {
            let id = event
                .get("session_id")
                .and_then(Value::as_str)
                .ok_or(Malformed)?;
            if !is_session_id(id) {
                return Err(Malformed);
            }
            Line::Init(id.to_owned())
        }
        "stream_event" => {
            let nested = event
                .get("event")
                .filter(|value| value.is_object())
                .ok_or(Malformed)?;
            match nested
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "message_start" => Line::MessageStart,
                "content_block_delta" => {
                    let delta = nested
                        .get("delta")
                        .filter(|value| value.is_object())
                        .ok_or(Malformed)?;
                    if delta.get("type").and_then(Value::as_str) == Some("text_delta") {
                        Line::TextDelta(
                            delta
                                .get("text")
                                .and_then(Value::as_str)
                                .ok_or(Malformed)?
                                .to_owned(),
                        )
                    } else {
                        Line::Progress
                    }
                }
                "content_block_start" => nested
                    .get("content_block")
                    .and_then(search_results)
                    .map(Line::WebSearch)
                    .unwrap_or(Line::Progress),
                _ => Line::Progress,
            }
        }
        "assistant" => event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(search_results)
                    .flatten()
                    .collect::<Vec<_>>()
            })
            .filter(|results| !results.is_empty())
            .map(Line::WebSearch)
            .unwrap_or(Line::Progress),
        "user" => Line::Progress,
        "result" => {
            let failed = event
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || event.get("subtype").and_then(Value::as_str) != Some("success");
            if failed {
                let message = event
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Line::ResultFailed(result_failure(message))
            } else {
                let id = event
                    .get("session_id")
                    .and_then(Value::as_str)
                    .filter(|id| is_session_id(id))
                    .map(str::to_owned);
                Line::ResultSuccess {
                    session_id: id,
                    text: event
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }
            }
        }
        _ => Line::Ignored,
    })
}

fn search_results(block: &Value) -> Option<Vec<SearchResult>> {
    if block.get("type").and_then(Value::as_str) != Some("web_search_tool_result") {
        return None;
    }
    let content = block.get("content")?.as_array()?;
    Some(content.iter().filter_map(search_result).collect())
}

fn search_result(value: &Value) -> Option<SearchResult> {
    if value.get("type").and_then(Value::as_str) != Some("web_search_result") {
        return None;
    }
    let url = value.get("url").and_then(Value::as_str)?;
    if !safe_http_url(url) {
        return None;
    }
    let title = value.get("title").and_then(Value::as_str).unwrap_or(url).trim();
    if title.is_empty() {
        return None;
    }
    Some(SearchResult {
        title: bounded(title, 512),
        url: bounded(url, 4096),
        snippet: String::new(),
        source_name: None,
        age: value
            .get("page_age")
            .and_then(Value::as_str)
            .map(|value| bounded(value, 256))
            .filter(|value| !value.is_empty()),
    })
}

fn safe_http_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 4096
        && !url.chars().any(|character| character.is_control() || character.is_whitespace())
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

const AUTH_REJECTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "AUTH_REJECTED",
    message: "Claude's sign-in was rejected. Run \"claude auth login\" in a terminal, then try again.",
    retryable: false,
};

const RATE_LIMITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_RATE_LIMITED",
    message: "Claude has reached a usage or rate limit. Try again later.",
    retryable: true,
};

const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "Claude's saved session no longer exists. Pervue will rebuild it from conversation history when possible.",
    retryable: false,
};

const UNAVAILABLE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Claude couldn't answer right now. Try again.",
    retryable: true,
};

/// Whether Claude's message says the session it was asked to resume doesn't
/// exist. Claude reports this in a `result`, or on stderr before `init` as
/// "No conversation found with session ID: …".
pub fn names_unknown_session(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "session no longer exists",
        "session not found",
        "no conversation found",
        "unknown session",
        "invalid session id",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

/// Classifies a failed `result` by specific phrases, never bare words such as
/// "rate" or "auth". Besides Claude's own wording, this covers the raw API
/// errors it passes on, such as `API Error: 429 {"type":"error","error":{"type":
/// "rate_limit_error",…}}`.
pub fn result_failure(message: &str) -> ErrorBody<'static> {
    let lower = message.to_ascii_lowercase();
    if names_unknown_session(message) {
        UNKNOWN_CONVERSATION
    } else if [
        "authentication failed",
        "authentication_error",
        "not authenticated",
        "login required",
        "not logged in",
        "oauth token",
        "401 unauthorized",
        "api error: 401",
        "invalid api key",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        AUTH_REJECTED
    } else if [
        "rate limit",
        "rate_limit_error",
        "usage limit",
        "too many requests",
        "api error: 429",
        "billing limit",
        "credit balance",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
        // "5-hour limit reached", "Weekly limit reached"; a context limit is
        // not something waiting fixes.
        || (lower.contains("limit reached") && !lower.contains("context"))
    {
        RATE_LIMITED
    } else {
        UNAVAILABLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_message_start_delta_and_result() {
        assert_eq!(
            parse(r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#),
            Ok(Line::Init("abc-123".to_owned()))
        );
        assert_eq!(
            parse(
                r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}"#
            ),
            Ok(Line::MessageStart)
        );
        assert_eq!(
            parse(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#
            ),
            Ok(Line::TextDelta("hi".to_owned()))
        );
        assert_eq!(
            parse(
                r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"abc-123"}"#
            ),
            Ok(Line::ResultSuccess {
                session_id: Some("abc-123".to_owned()),
                text: "done".to_owned()
            })
        );
    }

    #[test]
    fn parses_web_search_tool_results() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","title":"Example","url":"https://example.com/","page_age":"1 day ago"},{"type":"web_search_result","title":"Unsafe","url":"javascript:alert(1)"}]}}}"#;
        assert_eq!(
            parse(line),
            Ok(Line::WebSearch(vec![SearchResult {
                title: "Example".to_owned(),
                url: "https://example.com/".to_owned(),
                snippet: String::new(),
                source_name: None,
                age: Some("1 day ago".to_owned()),
            }]))
        );
    }

    #[test]
    fn invalid_session_ids_are_refused() {
        for id in ["", "--help", "../../x", "has space"] {
            assert!(!is_session_id(id), "{id}");
        }
    }

    #[test]
    fn error_classification_uses_specific_phrases() {
        assert_eq!(
            result_failure("authentication failed").reason,
            "AUTH_REJECTED"
        );
        assert_eq!(
            result_failure("rate limit exceeded (429 Too Many Requests)").reason,
            "PROVIDER_RATE_LIMITED"
        );
        assert_eq!(
            result_failure("session no longer exists").reason,
            "UNKNOWN_CONVERSATION"
        );
        assert_eq!(
            result_failure("No conversation found with session ID: abc").reason,
            "UNKNOWN_CONVERSATION"
        );
        assert_eq!(
            result_failure(
                r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#
            )
            .reason,
            "AUTH_REJECTED"
        );
        for message in [
            r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
            "5-hour limit reached \u{2219} resets 3pm",
            "Claude AI usage limit reached|1760000000",
        ] {
            assert_eq!(
                result_failure(message).reason,
                "PROVIDER_RATE_LIMITED",
                "{message}"
            );
        }

        for message in [
            "Failed to generate a response",
            "The author is unavailable",
            "Prompt exceeds context limit",
            "Context limit reached",
        ] {
            assert_eq!(
                result_failure(message).reason,
                "PROVIDER_UNAVAILABLE",
                "{message}"
            );
        }
    }
}
