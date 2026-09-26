//! Claude Code stream-json output reduced to provider-neutral events.

use serde_json::Value;

use crate::protocol::events::{ErrorBody, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Init(String),
    MessageStart,
    TextDelta(String),
    Progress,
    ResultSuccess {
        session_id: Option<String>,
        text: String,
    },
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
                _ => Line::Progress,
            }
        }
        "assistant" | "user" => Line::Progress,
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

pub fn result_failure(message: &str) -> ErrorBody<'static> {
    let lower = message.to_ascii_lowercase();
    if [
        "session no longer exists",
        "session not found",
        "no conversation found",
        "unknown session",
        "invalid session id",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        UNKNOWN_CONVERSATION
    } else if [
        "authentication failed",
        "not authenticated",
        "login required",
        "not logged in",
        "oauth token",
        "401 unauthorized",
        "invalid api key",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        AUTH_REJECTED
    } else if [
        "rate limit",
        "usage limit",
        "too many requests",
        "429 too many requests",
        "billing limit",
        "credit balance",
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
    fn parses_init_message_start_delta_and_result() {
        assert_eq!(
            parse(r#"{"type":"system","subtype":"init","session_id":"abc-123"}"#),
            Ok(Line::Init("abc-123".to_owned()))
        );
        assert_eq!(
            parse(r#"{"type":"stream_event","event":{"type":"message_start","message":{"role":"assistant"}}}"#),
            Ok(Line::MessageStart)
        );
        assert_eq!(
            parse(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#),
            Ok(Line::TextDelta("hi".to_owned()))
        );
        assert_eq!(
            parse(r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"abc-123"}"#),
            Ok(Line::ResultSuccess {
                session_id: Some("abc-123".to_owned()),
                text: "done".to_owned()
            })
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

        for message in [
            "Failed to generate a response",
            "The author is unavailable",
            "Prompt exceeds context limit",
        ] {
            assert_eq!(
                result_failure(message).reason,
                "PROVIDER_UNAVAILABLE",
                "{message}"
            );
        }
    }
}
