//! Antigravity CLI stream-json output reduced to Pervue's provider-neutral events.

use serde_json::Value;

use crate::protocol::events::{ErrorBody, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Init {
        conversation_id: String,
        permission_mode: String,
        agent: String,
    },
    AgentDelta(String),
    Tool(String),
    Subagent,
    Progress,
    ResultSuccess {
        conversation_id: Option<String>,
        response: String,
    },
    ResultFailed(ErrorBody<'static>),
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

const MAX_CONVERSATION_ID_LENGTH: usize = 128;

pub fn is_conversation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CONVERSATION_ID_LENGTH
        && !id.starts_with('-')
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub fn parse(line: &str) -> Result<Line, Malformed> {
    if line.trim().is_empty() {
        return Ok(Line::Ignored);
    }
    let event: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let kind = event.get("event").and_then(Value::as_str).ok_or(Malformed)?;
    Ok(match kind {
        "init" => {
            let conversation_id = event
                .get("conversation_id")
                .and_then(Value::as_str)
                .ok_or(Malformed)?;
            if !is_conversation_id(conversation_id) {
                return Err(Malformed);
            }
            let init = event.get("init").ok_or(Malformed)?;
            let permission_mode = init
                .get("permission_mode")
                .and_then(Value::as_str)
                .ok_or(Malformed)?;
            let agent = init.get("agent").and_then(Value::as_str).ok_or(Malformed)?;
            Line::Init {
                conversation_id: conversation_id.to_owned(),
                permission_mode: permission_mode.to_owned(),
                agent: agent.to_owned(),
            }
        }
        "step_update" => {
            let update = event.get("step_update").ok_or(Malformed)?;
            if update.get("subagent_info").is_some_and(|value| !value.is_null())
                || update.get("step_type").and_then(Value::as_str) == Some("subagent")
            {
                Line::Subagent
            } else if update.get("step_type").and_then(Value::as_str) == Some("tool")
                || update.get("tool_info").is_some_and(|value| !value.is_null())
            {
                Line::Tool(
                    update
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                )
            } else if update.get("step_type").and_then(Value::as_str) == Some("agent_response") {
                match update.get("text_delta").and_then(Value::as_str) {
                    Some(text) => Line::AgentDelta(text.to_owned()),
                    None => Line::Progress,
                }
            } else {
                Line::Progress
            }
        }
        "result" => {
            let result = event.get("result").ok_or(Malformed)?;
            let status = result
                .get("status")
                .and_then(Value::as_str)
                .ok_or(Malformed)?;
            if status == "SUCCESS" {
                let conversation_id = result
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if conversation_id
                    .as_deref()
                    .is_some_and(|id| !is_conversation_id(id))
                {
                    return Err(Malformed);
                }
                Line::ResultSuccess {
                    conversation_id,
                    response: result
                        .get("response")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }
            } else {
                let message = result
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or(status);
                Line::ResultFailed(provider_failure(message))
            }
        }
        _ => Line::Ignored,
    })
}

const AUTH_REJECTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "AUTH_REJECTED",
    message: "Gemini isn't signed in through Antigravity. Run \"agy\" in a terminal, sign in, then try again.",
    retryable: false,
};

const RATE_LIMITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_RATE_LIMITED",
    message: "Gemini has reached a usage or rate limit. Try again later.",
    retryable: true,
};

const UNAVAILABLE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Gemini couldn't answer through Antigravity right now. Try again.",
    retryable: true,
};

pub fn authentication_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "authentication required",
        "not authenticated",
        "not signed in",
        "sign-in required",
        "signin required",
        "login required",
        "credentials missing",
        "credentials not found",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

pub fn provider_failure(message: &str) -> ErrorBody<'static> {
    let lower = message.to_ascii_lowercase();
    if authentication_failure(message) {
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
    fn parses_antigravity_stream_json_events() {
        assert_eq!(
            parse(
                r#"{"event":"init","conversation_id":"agy-123","init":{"permission_mode":"request-review","agent":"pervue-text","tools":[]}}"#
            ),
            Ok(Line::Init {
                conversation_id: "agy-123".to_owned(),
                permission_mode: "request-review".to_owned(),
                agent: "pervue-text".to_owned(),
            })
        );
        assert_eq!(
            parse(
                r#"{"event":"step_update","step_update":{"step_type":"agent_response","state":"ACTIVE","text_delta":"hi"}}"#
            ),
            Ok(Line::AgentDelta("hi".to_owned()))
        );
        assert_eq!(
            parse(
                r#"{"event":"step_update","step_update":{"step_type":"tool","tool_name":"search_web","tool_info":{}}}"#
            ),
            Ok(Line::Tool("search_web".to_owned()))
        );
        assert_eq!(
            parse(
                r#"{"event":"result","result":{"conversation_id":"agy-123","status":"SUCCESS","response":"done"}}"#
            ),
            Ok(Line::ResultSuccess {
                conversation_id: Some("agy-123".to_owned()),
                response: "done".to_owned(),
            })
        );
    }

    #[test]
    fn invalid_conversation_ids_and_malformed_events_are_refused() {
        for id in ["", "--help", "../../x", "has space"] {
            let line = serde_json::json!({
                "event":"init",
                "conversation_id":id,
                "init":{"permission_mode":"request-review","agent":"pervue-text"}
            });
            assert_eq!(parse(&line.to_string()), Err(Malformed));
        }
        assert_eq!(parse("{not json"), Err(Malformed));
    }

    #[test]
    fn failures_are_normalized_without_forwarding_provider_text() {
        for (message, reason) in [
            ("authentication required", "AUTH_REJECTED"),
            ("429 RESOURCE_EXHAUSTED quota", "PROVIDER_RATE_LIMITED"),
            ("secret provider detail", "PROVIDER_UNAVAILABLE"),
        ] {
            let failure = provider_failure(message);
            assert_eq!(failure.reason, reason);
            assert!(!failure.message.contains("secret"));
        }
    }
}
