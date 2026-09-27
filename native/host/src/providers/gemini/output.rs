//! Gemini CLI stream-json output reduced to Pervue's provider-neutral events.

use serde_json::Value;

use crate::protocol::events::{ErrorBody, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Init(String),
    AssistantDelta(String),
    WebSearch,
    Progress,
    ResultSuccess,
    ResultFailed(ErrorBody<'static>),
    Ignored,
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
        "init" => {
            let id = event.get("session_id").and_then(Value::as_str).ok_or(Malformed)?;
            if !is_session_id(id) {
                return Err(Malformed);
            }
            Line::Init(id.to_owned())
        }
        "message" => {
            let role = event.get("role").and_then(Value::as_str).ok_or(Malformed)?;
            let content = event.get("content").and_then(Value::as_str).ok_or(Malformed)?;
            if role == "assistant" {
                Line::AssistantDelta(content.to_owned())
            } else {
                Line::Progress
            }
        }
        "tool_use" => {
            let tool = event.get("tool_name").and_then(Value::as_str).ok_or(Malformed)?;
            if tool == "google_web_search" {
                Line::WebSearch
            } else {
                Line::Progress
            }
        }
        "tool_result" => Line::Progress,
        "error" => {
            let severity = event.get("severity").and_then(Value::as_str).ok_or(Malformed)?;
            let message = event.get("message").and_then(Value::as_str).unwrap_or_default();
            if severity == "error" {
                Line::ResultFailed(provider_failure(message))
            } else {
                Line::Progress
            }
        }
        "result" => match event.get("status").and_then(Value::as_str) {
            Some("success") => Line::ResultSuccess,
            Some("error") => {
                let message = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Line::ResultFailed(provider_failure(message))
            }
            _ => return Err(Malformed),
        },
        _ => Line::Ignored,
    })
}

const AUTH_REJECTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "AUTH_REJECTED",
    message: "Gemini isn't signed in. Open Gemini CLI, sign in, then try again.",
    retryable: false,
};

const RATE_LIMITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_RATE_LIMITED",
    message: "Gemini has reached a usage or rate limit. Try again later.",
    retryable: true,
};

const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "Gemini's saved session no longer exists. Pervue will rebuild it from conversation history when possible.",
    retryable: false,
};

const UNAVAILABLE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Gemini couldn't answer right now. Try again.",
    retryable: true,
};

pub fn names_unknown_session(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    ["invalid session identifier", "session not found", "no session", "unknown session"]
        .iter()
        .any(|phrase| lower.contains(phrase))
}

pub fn provider_failure(message: &str) -> ErrorBody<'static> {
    let lower = message.to_ascii_lowercase();
    if names_unknown_session(message) {
        UNKNOWN_CONVERSATION
    } else if [
        "not authenticated",
        "authentication required",
        "login required",
        "sign in",
        "401 unauthorized",
        "invalid api key",
        "api key not valid",
        "oauth",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        AUTH_REJECTED
    } else if [
        "rate limit",
        "quota",
        "resource_exhausted",
        "too many requests",
        "429",
        "usage limit",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
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
    fn parses_documented_stream_json_events() {
        assert_eq!(
            parse(r#"{"type":"init","timestamp":"x","session_id":"abc-123","model":"gemini"}"#),
            Ok(Line::Init("abc-123".to_owned()))
        );
        assert_eq!(
            parse(r#"{"type":"message","timestamp":"x","role":"assistant","content":"hi","delta":true}"#),
            Ok(Line::AssistantDelta("hi".to_owned()))
        );
        assert_eq!(
            parse(r#"{"type":"tool_use","timestamp":"x","tool_name":"google_web_search","tool_id":"1","parameters":{}}"#),
            Ok(Line::WebSearch)
        );
        assert_eq!(
            parse(r#"{"type":"result","timestamp":"x","status":"success"}"#),
            Ok(Line::ResultSuccess)
        );
    }

    #[test]
    fn invalid_session_ids_and_malformed_events_are_refused() {
        for id in ["", "--help", "../../x", "has space"] {
            let line = serde_json::json!({"type":"init","session_id":id,"model":"x","timestamp":"x"});
            assert_eq!(parse(&line.to_string()), Err(Malformed));
        }
        assert_eq!(parse("{not json"), Err(Malformed));
    }

    #[test]
    fn failures_are_normalized_without_forwarding_provider_text() {
        for (message, reason) in [
            ("Please sign in to continue", "AUTH_REJECTED"),
            ("429 RESOURCE_EXHAUSTED quota", "PROVIDER_RATE_LIMITED"),
            ("session not found", "UNKNOWN_CONVERSATION"),
            ("secret provider detail", "PROVIDER_UNAVAILABLE"),
        ] {
            let failure = provider_failure(message);
            assert_eq!(failure.reason, reason);
            assert!(!failure.message.contains("secret"));
        }
    }
}
