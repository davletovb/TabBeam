//! Grok Build headless stream-json output reduced to Pervue events.

use serde_json::Value;

use crate::protocol::events::{ErrorBody, ErrorCode};
use crate::search::SearchResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Init {
        api_key_source: String,
        model: String,
        cwd: String,
        tools: Vec<String>,
        skills: Vec<String>,
        active_mcp_servers: usize,
    },
    Assistant {
        text: String,
        searched: bool,
        sources: Vec<SearchResult>,
        forbidden_tool: bool,
    },
    ResultSuccess {
        text: String,
    },
    ResultFailed(ErrorBody<'static>),
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

pub fn parse(line: &str) -> Result<Line, Malformed> {
    if line.trim().is_empty() {
        return Ok(Line::Ignored);
    }
    let event: Value = serde_json::from_str(line).map_err(|_| Malformed)?;
    let kind = event.get("type").and_then(Value::as_str).ok_or(Malformed)?;
    Ok(match kind {
        "system" if event.get("subtype").and_then(Value::as_str) == Some("init") => {
            let api_key_source = event
                .get("apiKeySource")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let model = event
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let cwd = event
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let tools = strings(event.get("tools"))?;
            let skills = strings(event.get("skills"))?;
            let active_mcp_servers = event
                .get("mcp_servers")
                .and_then(Value::as_array)
                .map(|servers| {
                    servers
                        .iter()
                        .filter(|server| {
                            server.get("status").and_then(Value::as_str) != Some("disabled")
                        })
                        .count()
                })
                .unwrap_or(0);
            Line::Init {
                api_key_source,
                model,
                cwd,
                tools,
                skills,
                active_mcp_servers,
            }
        }
        "system" => Line::Ignored,
        "assistant" => parse_assistant(&event)?,
        "user" => {
            let blocks = event
                .pointer("/message/content")
                .and_then(Value::as_array)
                .ok_or(Malformed)?;
            if blocks.iter().any(|block| {
                matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("tool_result")
                )
            }) {
                Line::Assistant {
                    text: String::new(),
                    searched: false,
                    sources: Vec::new(),
                    forbidden_tool: true,
                }
            } else {
                Line::Ignored
            }
        }
        "result" => {
            let subtype = event
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let is_error = event
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if subtype == "success" && !is_error {
                Line::ResultSuccess {
                    text: event
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                }
            } else {
                let message = event
                    .get("errors")
                    .and_then(Value::as_array)
                    .and_then(|errors| errors.first())
                    .and_then(|error| {
                        error
                            .get("message")
                            .and_then(Value::as_str)
                            .or_else(|| error.as_str())
                    })
                    .unwrap_or_default();
                Line::ResultFailed(provider_failure(message))
            }
        }
        // Raw partial-message framing is not requested by Pervue. If a future
        // Grok starts emitting it anyway, fail closed rather than accidentally
        // forwarding a new execution-bearing block.
        "stream_event" => return Err(Malformed),
        _ => Line::Ignored,
    })
}

fn strings(value: Option<&Value>) -> Result<Vec<String>, Malformed> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or(Malformed))
            .collect(),
        Some(_) => Err(Malformed),
    }
}

fn parse_assistant(event: &Value) -> Result<Line, Malformed> {
    let blocks = event
        .pointer("/message/content")
        .and_then(Value::as_array)
        .ok_or(Malformed)?;
    let mut text = String::new();
    let mut searched = false;
    let mut sources = Vec::new();
    let mut forbidden_tool = false;

    for block in blocks {
        match block.get("type").and_then(Value::as_str).ok_or(Malformed)? {
            "text" => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    text.push_str(value);
                }
            }
            "thinking" => {}
            "server_tool_use" => {
                if block.get("name").and_then(Value::as_str) == Some("web_search") {
                    searched = true;
                } else {
                    forbidden_tool = true;
                }
            }
            "web_search_tool_result" => {
                let Some(results) = block.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for result in results {
                    if result.get("type").and_then(Value::as_str) != Some("web_search_result") {
                        continue;
                    }
                    let Some(url) = result.get("url").and_then(Value::as_str) else {
                        continue;
                    };
                    let title = result.get("title").and_then(Value::as_str).unwrap_or(url);
                    if let Some(source) = SearchResult::new(title, url, "", None, None) {
                        sources.push(source);
                    }
                }
            }
            "tool_use" => forbidden_tool = true,
            _ => return Err(Malformed),
        }
    }

    Ok(Line::Assistant {
        text,
        searched,
        sources,
        forbidden_tool,
    })
}

const AUTH_REJECTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "AUTH_REJECTED",
    message: "Grok isn't signed in with a Grok/X account. Run grok login, then try again.",
    retryable: false,
};

const RATE_LIMITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_RATE_LIMITED",
    message: "Grok has reached a usage or rate limit. Try again later.",
    retryable: true,
};

const UNAVAILABLE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Grok couldn't answer right now. Try again.",
    retryable: true,
};

pub fn authentication_failure(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "not authenticated",
        "authentication failed",
        "run grok login",
        "sign in",
        "login required",
        "cached credential",
        "token expired",
        "unauthorized",
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
        "quota exceeded",
        "resource_exhausted",
        "too many requests",
        "http 429",
        "status 429",
        "code 429",
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
    fn parses_current_messages_stream_shapes() {
        let init = parse(r#"{"type":"system","subtype":"init","session_id":"s","apiKeySource":"oauth","model":"grok-4.6","cwd":"/tmp/x","tools":["web_search"],"skills":[],"mcp_servers":[]}"#).unwrap();
        assert!(matches!(
            init,
            Line::Init {
                api_key_source,
                model,
                tools,
                active_mcp_servers: 0,
                ..
            } if api_key_source == "oauth" && model == "grok-4.6" && tools == ["web_search"]
        ));

        let assistant = parse(r#"{"type":"assistant","message":{"content":[{"type":"server_tool_use","id":"x","name":"web_search","input":{"query":"q"}},{"type":"web_search_tool_result","tool_use_id":"x","content":[{"type":"web_search_result","url":"https://example.com/a","title":"Example"}]},{"type":"text","text":"answer"}]}}"#).unwrap();
        match assistant {
            Line::Assistant {
                text,
                searched,
                sources,
                forbidden_tool,
            } => {
                assert_eq!(text, "answer");
                assert!(searched);
                assert!(!forbidden_tool);
                assert_eq!(sources.len(), 1);
                assert_eq!(sources[0].url, "https://example.com/a");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn client_tools_fail_closed() {
        let line = parse(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x","name":"run_terminal_cmd","input":{}}]}}"#).unwrap();
        assert!(matches!(
            line,
            Line::Assistant {
                forbidden_tool: true,
                ..
            }
        ));
    }

    #[test]
    fn provider_errors_are_normalized() {
        assert_eq!(
            provider_failure("Authentication failed").reason,
            "AUTH_REJECTED"
        );
        assert_eq!(
            provider_failure("HTTP 429 rate limit").reason,
            "PROVIDER_RATE_LIMITED"
        );
        assert_eq!(
            provider_failure("internal detail").reason,
            "PROVIDER_UNAVAILABLE"
        );
    }
}
