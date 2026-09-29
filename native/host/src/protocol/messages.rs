//! Pervue-owned wording for neutral runtime failures.

use seatline_core::protocol::Failure;

use super::events::ErrorBody;

pub fn provider_failure(provider: Option<&str>, failure: Failure) -> ErrorBody<'static> {
    let reason = wire_reason(failure.reason);
    ErrorBody {
        code: failure.code.into(),
        reason,
        message: message(provider, reason, failure.retryable),
        retryable: failure.retryable,
    }
}

/// The name protocol v1 gives a failure the runtime names in its own terms.
/// Pervue only asks for a tool-free turn to carry page context, and only
/// resumes a session on behalf of a conversation.
fn wire_reason(reason: &'static str) -> &'static str {
    match reason {
        "TOOL_ISOLATION_UNAVAILABLE" => "PAGE_CONTEXT_TOOLS_ENABLED",
        "UNKNOWN_SESSION" => "UNKNOWN_CONVERSATION",
        reason => reason,
    }
}

fn message(provider: Option<&str>, reason: &str, retryable: bool) -> &'static str {
    match (provider, reason, retryable) {
        (Some("codex"), "EXECUTABLE_NOT_FOUND", _) => {
            "Codex isn't installed. Install the Codex CLI, then try again."
        }
        (Some("claude"), "EXECUTABLE_NOT_FOUND", _) => {
            "Claude isn't installed. Install Claude Code, then try again."
        }
        (Some("gemini"), "EXECUTABLE_NOT_FOUND", _) => {
            "Antigravity CLI isn't installed. Install Antigravity CLI, then try again."
        }
        (Some("grok"), "EXECUTABLE_NOT_FOUND", _) => {
            "Grok Build isn't installed. Install the Grok CLI, then try again."
        }

        (Some("codex"), "LOGIN_REQUIRED" | "AUTH_REJECTED", _) => {
            "Codex isn't signed in. Run \"codex login\" in a terminal, then try again."
        }
        (Some("claude"), "LOGIN_REQUIRED" | "AUTH_REJECTED", _) => {
            "Claude isn't signed in. Run \"claude auth login\" in a terminal, then try again."
        }
        (Some("gemini"), "AUTH_REJECTED", _) => {
            "Gemini isn't signed in through Antigravity. Run \"agy\" in a terminal, sign in, then try again."
        }
        (Some("grok"), "AUTH_REJECTED", _) => {
            "Grok isn't signed in with a Grok/X account. Run grok login, then try again."
        }

        (Some("codex"), "PROVIDER_UNAVAILABLE", false) => {
            "Codex couldn't start. Reinstall the Codex CLI, then try again."
        }
        (Some("claude"), "PROVIDER_UNAVAILABLE", false) => {
            "Claude couldn't start. Reinstall Claude Code, then try again."
        }
        (Some("gemini"), "PROVIDER_UNAVAILABLE", false) => {
            "Antigravity CLI couldn't start. Reinstall it, then try again."
        }
        (Some("grok"), "PROVIDER_UNAVAILABLE", false) => {
            "Grok Build couldn't start. Reinstall it, then try again."
        }
        (Some("codex"), "PROVIDER_UNAVAILABLE", true) => {
            "Codex couldn't answer right now. Try again."
        }
        (Some("claude"), "PROVIDER_UNAVAILABLE", true) => {
            "Claude couldn't answer right now. Try again."
        }
        (Some("gemini"), "PROVIDER_UNAVAILABLE", true) => {
            "Gemini couldn't answer through Antigravity right now. Try again."
        }
        (Some("grok"), "PROVIDER_UNAVAILABLE", true) => {
            "Grok couldn't answer right now. Try again."
        }

        (Some("codex"), "WORKSPACE_UNAVAILABLE", _) => {
            "Pervue couldn't prepare a private folder for Codex. Make sure your cache folder exists and only you can change it, then try again."
        }
        (Some("claude"), "WORKSPACE_UNAVAILABLE", _) => {
            "Pervue couldn't prepare a private folder for Claude. Check your cache folder and try again."
        }
        (Some("gemini"), "WORKSPACE_UNAVAILABLE", _) => {
            "Pervue couldn't prepare a private folder for Antigravity. Check your cache folder, then try again."
        }
        (Some("grok"), "WORKSPACE_UNAVAILABLE", _) => {
            "Pervue couldn't prepare a private folder for Grok. Check your cache folder, then try again."
        }

        (Some("codex"), "PROCESS_EXITED", _) => "Codex stopped unexpectedly. Try again.",
        (Some("claude"), "PROCESS_EXITED", _) => "Claude stopped unexpectedly. Try again.",
        (Some("gemini"), "PROCESS_EXITED", _) => "Antigravity stopped unexpectedly. Try again.",
        (Some("grok"), "PROCESS_EXITED", _) => "Grok stopped unexpectedly. Try again.",

        (Some("codex"), "MALFORMED_PROVIDER_OUTPUT", _) => {
            "Codex answered in a way Pervue doesn't understand. Update Codex and Pervue, then try again."
        }
        (Some("claude"), "MALFORMED_PROVIDER_OUTPUT", _) => {
            "Claude answered in a way Pervue doesn't understand. Update Claude Code and Pervue, then try again."
        }
        (Some("gemini"), "MALFORMED_PROVIDER_OUTPUT", _) => {
            "Antigravity answered in a way Pervue doesn't understand. Update Antigravity CLI and Pervue, then try again."
        }
        (Some("grok"), "MALFORMED_PROVIDER_OUTPUT", _) => {
            "Grok answered in a way Pervue doesn't understand. Update Grok Build and Pervue, then try again."
        }

        (Some("codex"), "PROVIDER_RATE_LIMITED", _) => {
            "Codex has reached a usage or rate limit. Try again later."
        }
        (Some("claude"), "PROVIDER_RATE_LIMITED", _) => {
            "Claude has reached a usage or rate limit. Try again later."
        }
        (Some("gemini"), "PROVIDER_RATE_LIMITED", _) => {
            "Gemini has reached a usage or rate limit. Try again later."
        }
        (Some("grok"), "PROVIDER_RATE_LIMITED", _) => {
            "Grok has reached a usage or rate limit. Try again later."
        }

        (Some("codex"), "PAGE_CONTEXT_TOOLS_ENABLED", _) => {
            "Pervue won't send browser context to Codex while user-configured MCP servers are enabled. Disable them or choose No context."
        }
        (Some("codex"), "NATIVE_SEARCH_CONFIGURATION_UNSAFE", _) => {
            "Pervue won't enable Codex web search while user-configured MCP servers are enabled. Disable them or use a plain Ask turn."
        }
        (Some("gemini"), "PROVIDER_AGENT_NOT_USED", _) => {
            "Antigravity didn't use Pervue's restricted agent, so the turn was stopped. Update Antigravity CLI, then try again."
        }
        (Some("gemini"), "PROVIDER_PERMISSIONS_TOO_OPEN", _) => {
            "Antigravity is set to run tools without asking, so Pervue stopped the turn. Set Antigravity's tool permission to review requests, then try again."
        }
        (Some("grok"), "SEARCH_UNSUPPORTED", _) => {
            "Grok Build's shipped headless CLI doesn't expose a Pervue-safe native web-search surface yet. Turn Web off and try again."
        }
        (Some("claude"), "UNKNOWN_CONVERSATION", _) => {
            "Claude's saved session no longer exists or can't be continued. Start a new conversation."
        }
        (Some("gemini"), "UNKNOWN_CONVERSATION", _) => {
            "This Gemini conversation can't be continued. Start a new conversation."
        }
        (Some("grok"), "UNKNOWN_CONVERSATION", _) => {
            "This Grok conversation can't be continued. Start a new conversation."
        }

        (_, "PROVIDER_NOT_INSTALLED", _) => {
            "Pervue's companion app doesn't support this AI provider yet. Update it, then try again."
        }
        (_, "NATIVE_SEARCH_UNSUPPORTED", _) => {
            "The selected AI provider does not support native web search."
        }
        (_, "SEARCH_WITH_CONTEXT_UNSUPPORTED", _) => {
            "Pervue won't combine web search with browser context yet. Choose No context or turn off Search."
        }
        (_, "PAGE_CONTEXT_UNSUPPORTED", _) => {
            "This AI provider can't use browser context. Choose No context, then ask again."
        }
        (_, "MODEL_SELECTION_UNSUPPORTED", _) => {
            "This AI provider can't switch models. Choose its default model, then ask again."
        }
        (_, "PROVIDER_RATE_LIMITED", _) => {
            "The provider has reached a usage or rate limit. Try again later."
        }
        (_, "UNKNOWN_CONVERSATION", _) => "This conversation can't be continued. Start a new one.",
        (_, "WORKSPACE_UNAVAILABLE", _) => {
            "Pervue couldn't prepare a private provider workspace. Check your cache folder, then try again."
        }
        (_, "PROCESS_EXITED", _) => "The provider stopped unexpectedly. Try again.",
        (_, "MALFORMED_PROVIDER_OUTPUT", _) => {
            "The provider answered in an unsupported format. Update the provider CLI and Pervue, then try again."
        }
        (_, "PROVIDER_BOUNDARY_VIOLATION", _) => {
            "The provider exposed or used a tool Pervue doesn't allow, so the turn was stopped."
        }
        (_, "PROVIDER_AGENT_NOT_USED", _) => {
            "The provider did not use Pervue's restricted agent, so the turn was stopped."
        }
        (_, "PROVIDER_PERMISSIONS_TOO_OPEN", _) => {
            "The provider's tool permissions are too open for this Pervue turn."
        }
        (_, "SESSION_STORE_FAILED", _) => {
            "The provider conversation could not be saved. Check available disk space and try again."
        }
        (_, "SESSION_FORGET_FAILED", _) => {
            "Pervue couldn't remove everything this conversation left behind. Delete it again to retry."
        }
        (_, "NATIVE_SEARCH_NO_SOURCES", _) => {
            "The provider finished web search without returning usable sources. Try again or update the provider."
        }
        (_, "PAGE_CONTEXT_TOOLS_ENABLED", _) => {
            "Pervue won't send browser context while user-configured provider tools are enabled."
        }
        (_, "NATIVE_SEARCH_CONFIGURATION_UNSAFE", _) => {
            "Pervue won't enable native search while user-configured provider tools are enabled."
        }
        (_, "MODEL_NOT_SUPPORTED" | "MODEL_MISMATCH", _) => {
            "The selected model is not supported by this provider."
        }
        (_, "SEARCH_UNSUPPORTED", _) => {
            "This provider execution mode does not expose a Pervue-safe native web-search surface."
        }
        (_, "PERSISTENT_SESSION_UNSUPPORTED", _) => {
            "This provider execution mode does not support persistent native sessions."
        }
        (_, "WORKSPACE_MISMATCH" | "TOOLSET_MISMATCH" | "SKILLS_MISMATCH" | "MCP_MISMATCH", _) => {
            "The provider crossed Pervue's isolated execution boundary, so the turn was stopped."
        }
        (_, "PROVIDER_UNAVAILABLE", _) => "The provider couldn't answer right now. Try again.",
        _ => "The provider couldn't complete this request. Try again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seatline_core::protocol::ErrorCode;

    fn failure(reason: &'static str, retryable: bool) -> Failure {
        Failure {
            code: ErrorCode::ProviderFailed,
            reason,
            retryable,
        }
    }

    #[test]
    fn provider_wording_preserves_specific_remedies() {
        let cases = [
            (
                "codex",
                failure("PROVIDER_UNAVAILABLE", false),
                "Reinstall the Codex CLI",
            ),
            (
                "claude",
                failure("PROVIDER_UNAVAILABLE", false),
                "Reinstall Claude Code",
            ),
            (
                "gemini",
                failure("PROVIDER_PERMISSIONS_TOO_OPEN", false),
                "review requests",
            ),
            (
                "gemini",
                failure("MALFORMED_PROVIDER_OUTPUT", false),
                "Update Antigravity CLI",
            ),
            ("grok", failure("SEARCH_UNSUPPORTED", false), "Turn Web off"),
            (
                "codex",
                failure("WORKSPACE_UNAVAILABLE", false),
                "only you can change it",
            ),
        ];
        for (provider, failure, expected) in cases {
            let rendered = provider_failure(Some(provider), failure);
            assert!(
                rendered.message.contains(expected),
                "{provider}: {}",
                rendered.message
            );
            assert!(!rendered.message.contains("sk-"));
        }
    }

    #[test]
    fn retryable_unavailable_is_not_described_as_an_install_failure() {
        let rendered = provider_failure(Some("claude"), failure("PROVIDER_UNAVAILABLE", true));
        assert!(rendered.message.contains("answer right now"));
        assert!(!rendered.message.contains("Reinstall"));
    }

    #[test]
    fn runtime_reasons_keep_the_names_protocol_v1_gives_them() {
        // A tool-free turn is only asked for to carry page context.
        let refused = provider_failure(
            Some("codex"),
            Failure {
                code: ErrorCode::InvalidRequest,
                reason: "TOOL_ISOLATION_UNAVAILABLE",
                retryable: false,
            },
        );
        assert_eq!(refused.reason, "PAGE_CONTEXT_TOOLS_ENABLED");
        assert_eq!(
            refused.message,
            provider_failure(
                Some("codex"),
                Failure {
                    code: ErrorCode::InvalidRequest,
                    reason: "PAGE_CONTEXT_TOOLS_ENABLED",
                    retryable: false,
                }
            )
            .message
        );

        // A session is only resumed on behalf of a conversation.
        let lost = provider_failure(
            Some("claude"),
            Failure {
                code: ErrorCode::InvalidRequest,
                reason: "UNKNOWN_SESSION",
                retryable: false,
            },
        );
        assert_eq!(lost.reason, "UNKNOWN_CONVERSATION");
        assert_eq!(
            lost.message,
            provider_failure(
                Some("claude"),
                Failure {
                    code: ErrorCode::InvalidRequest,
                    reason: "UNKNOWN_CONVERSATION",
                    retryable: false,
                }
            )
            .message
        );
    }
}
