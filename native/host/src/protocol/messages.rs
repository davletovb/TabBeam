//! Pervue-owned wording for neutral runtime failures.

use runtime_core::protocol::Failure;

use super::events::ErrorBody;

pub fn provider_failure(provider: Option<&str>, failure: Failure) -> ErrorBody<'static> {
    ErrorBody {
        code: failure.code,
        reason: failure.reason,
        message: message(provider, failure.reason),
        retryable: failure.retryable,
    }
}

fn message(provider: Option<&str>, reason: &str) -> &'static str {
    match (provider, reason) {
        (Some("codex"), "EXECUTABLE_NOT_FOUND") => {
            "Codex isn't installed. Install the Codex CLI, then try again."
        }
        (Some("claude"), "EXECUTABLE_NOT_FOUND") => {
            "Claude isn't installed. Install Claude Code, then try again."
        }
        (Some("gemini"), "EXECUTABLE_NOT_FOUND") => {
            "Antigravity CLI isn't installed. Install Antigravity CLI, then try again."
        }
        (Some("grok"), "EXECUTABLE_NOT_FOUND") => {
            "Grok Build isn't installed. Install the Grok CLI, then try again."
        }
        (Some("codex"), "LOGIN_REQUIRED" | "AUTH_REJECTED") => {
            "Codex isn't signed in. Run \"codex login\" in a terminal, then try again."
        }
        (Some("claude"), "LOGIN_REQUIRED" | "AUTH_REJECTED") => {
            "Claude isn't signed in. Run \"claude auth login\" in a terminal, then try again."
        }
        (Some("gemini"), "AUTH_REJECTED") => {
            "Gemini isn't signed in through Antigravity. Run \"agy\" in a terminal, sign in, then try again."
        }
        (Some("grok"), "AUTH_REJECTED") => {
            "Grok isn't signed in with a Grok/X account. Run grok login, then try again."
        }
        (_, "PROVIDER_NOT_INSTALLED") => {
            "Pervue's companion app doesn't support this AI provider yet. Update it, then try again."
        }
        (_, "NATIVE_SEARCH_UNSUPPORTED") => {
            "The selected AI provider does not support native web search."
        }
        (_, "SEARCH_WITH_CONTEXT_UNSUPPORTED") => {
            "Pervue won't combine web search with browser context yet. Choose No context or turn off Search."
        }
        (_, "PAGE_CONTEXT_UNSUPPORTED") => {
            "This AI provider can't use browser context. Choose No context, then ask again."
        }
        (_, "MODEL_SELECTION_UNSUPPORTED") => {
            "This AI provider can't switch models. Choose its default model, then ask again."
        }
        (_, "PROVIDER_RATE_LIMITED") => {
            "The provider has reached a usage or rate limit. Try again later."
        }
        (_, "UNKNOWN_CONVERSATION") => "This conversation can't be continued. Start a new one.",
        (_, "WORKSPACE_UNAVAILABLE") => {
            "Pervue couldn't prepare a private provider workspace. Check your cache folder, then try again."
        }
        (_, "PROCESS_EXITED") => "The provider stopped unexpectedly. Try again.",
        (_, "MALFORMED_PROVIDER_OUTPUT") => {
            "The provider answered in an unsupported format. Update the provider CLI and Pervue, then try again."
        }
        (_, "PROVIDER_BOUNDARY_VIOLATION") => {
            "The provider exposed or used a tool Pervue doesn't allow, so the turn was stopped."
        }
        (_, "PROVIDER_AGENT_NOT_USED") => {
            "The provider did not use Pervue's restricted agent, so the turn was stopped."
        }
        (_, "PROVIDER_PERMISSIONS_TOO_OPEN") => {
            "The provider's tool permissions are too open for this Pervue turn."
        }
        (_, "SESSION_STORE_FAILED") => {
            "The provider conversation could not be saved. Check available disk space and try again."
        }
        (_, "SESSION_FORGET_FAILED") => {
            "Pervue couldn't remove everything this conversation left behind. Delete it again to retry."
        }
        (_, "NATIVE_SEARCH_NO_SOURCES") => {
            "The provider finished web search without returning usable sources. Try again or update the provider."
        }
        (_, "PAGE_CONTEXT_TOOLS_ENABLED") => {
            "Pervue won't send browser context while user-configured provider tools are enabled."
        }
        (_, "NATIVE_SEARCH_CONFIGURATION_UNSAFE") => {
            "Pervue won't enable native search while user-configured provider tools are enabled."
        }
        (_, "MODEL_NOT_SUPPORTED" | "MODEL_MISMATCH") => {
            "The selected model is not supported by this provider."
        }
        (_, "SEARCH_UNSUPPORTED") => {
            "This provider execution mode does not expose a Pervue-safe native web-search surface."
        }
        (_, "PERSISTENT_SESSION_UNSUPPORTED") => {
            "This provider execution mode does not support persistent native sessions."
        }
        (_, "WORKSPACE_MISMATCH" | "TOOLSET_MISMATCH" | "SKILLS_MISMATCH" | "MCP_MISMATCH") => {
            "The provider crossed Pervue's isolated execution boundary, so the turn was stopped."
        }
        (_, "PROVIDER_UNAVAILABLE") => "The provider couldn't answer right now. Try again.",
        _ => "The provider couldn't complete this request. Try again.",
    }
}
