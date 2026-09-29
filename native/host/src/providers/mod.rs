//! Provider adapters (PRO-01).
//!
//! The adapter contract is the shared surface proven by the Codex and Claude
//! adapters (PRO-07). A [`Provider`] reports its status and serves requests
//! through [`Exchange`]s: state machines the host loop drives, which never
//! block past the deadline they are given, and never keep working past it for
//! longer than [`BUSY_LIMIT`], however fast a provider writes. That lets one
//! host serve several requests, and read cancellations, while providers work.
//!
//! Everything provider-specific stays inside the adapter: command lines,
//! output formats, and provider session IDs. The host sees only [`Update`]s in
//! protocol terms, and the popup sees only protocol events.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::conversation::{BrowserContext, HistoryMessage};
use runtime_core::protocol::Capabilities;
pub use runtime_core::stream::BUSY_LIMIT;
use runtime_core::turn::SessionPolicy;

pub mod claude;
pub mod codex;
pub mod discovery;
pub mod environment;
pub mod fake;
pub mod forget;
pub mod gemini;
pub mod grok;
pub mod layout;
pub(crate) mod private_fs;

pub use layout::Layout;

/// One `conversation.send`, in provider-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub text: String,
    /// Previous user and assistant messages, in order. Adapters without a
    /// native continuation can use these to reconstruct the dialogue.
    pub history: Vec<HistoryMessage>,
    /// The conversation to continue, or `None` to start one.
    pub conversation_id: Option<String>,
    /// Browser context explicitly attached to this turn, after validation
    /// at the native trust boundary.
    pub context: Option<BrowserContext>,
    /// The model to answer with, already a valid model ID; `None` for the
    /// provider's default. Only sent to adapters with `model_selection`.
    pub model: Option<String>,
    /// Whether the selected AI provider should perform its own authenticated
    /// native web search during this same turn.
    pub native_search: bool,
    /// Whether provider-native state may outlive this turn.
    pub session_policy: SessionPolicy,
    /// Start a new native session even when this Pervue conversation already
    /// has one. The host uses this for search isolation.
    pub fresh_session: bool,
}

pub use runtime_core::exchange::{Exchange, Scripted, Timeouts, Update};

pub(crate) fn keep_bounded_output(output: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(output.len());
    output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

/// A provider adapter.
pub trait Provider {
    /// The provider ID requests name, such as `codex`.
    fn id(&self) -> &str;

    fn timeouts(&self) -> Timeouts;

    /// Capabilities that are stable for this adapter implementation. The host
    /// uses these to reject requests that would otherwise be silently degraded.
    fn capabilities(&self) -> Capabilities;

    /// Whether this execution mode supports a provider-native persistent
    /// session that can be resumed by an opaque handle.
    fn supports_persistent_session(&self) -> bool {
        false
    }

    /// Starts checking availability, authentication, and capabilities. The
    /// exchange reports one `Status` and then `Completed`.
    fn status(&self) -> Box<dyn Exchange>;

    /// Starts serving `request`.
    fn send(&self, request: SendRequest) -> Box<dyn Exchange>;

    /// Removes what this adapter keeps for a deleted conversation
    /// (`conversation.forget`): its native-session mapping and, where the
    /// adapter can prove the provider wrote it for Pervue, the provider's own
    /// transcript. A conversation it doesn't know completes: there is nothing
    /// to remove. An adapter that keeps nothing uses this default.
    fn forget(&self, conversation_id: &str) -> Box<dyn Exchange> {
        let _ = conversation_id;
        Box::new(Scripted::new([Update::Completed]))
    }
}

/// The providers a host serves, in the order `provider.status` reports them.
pub struct Providers(Vec<Box<dyn Provider>>);

impl Providers {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self(providers)
    }

    /// The providers of an installed host. The fake scaffold stays registered
    /// for deterministic protocol diagnostics; real adapters use the same
    /// platform discovery rules.
    pub fn installed(layout: &Layout) -> Self {
        Self(vec![
            Box::new(fake::Fake),
            Box::new(codex::Codex::installed(layout)),
            Box::new(claude::Claude::installed(layout)),
            Box::new(gemini::Gemini::installed(layout)),
            Box::new(grok::Grok::installed(layout)),
        ])
    }

    /// Only the deterministic fake scaffold, which starts no processes: for
    /// fuzzing and protocol tests.
    pub fn scaffold() -> Self {
        Self(vec![Box::new(fake::Fake)])
    }

    pub fn get(&self, id: &str) -> Option<&dyn Provider> {
        self.0
            .iter()
            .map(Box::as_ref)
            .find(|provider| provider.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Provider> {
        self.0.iter().map(Box::as_ref)
    }
}

/// Reports every provider's status in turn, then completes: the exchange
/// behind `provider.status` without a provider ID.
pub struct StatusOfAll {
    pending: VecDeque<Box<dyn Exchange>>,
    current: Option<Box<dyn Exchange>>,
}

impl StatusOfAll {
    pub fn new(providers: &Providers) -> Self {
        Self {
            pending: providers.iter().map(Provider::status).collect(),
            current: None,
        }
    }
}

impl Exchange for StatusOfAll {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        loop {
            let current = match &mut self.current {
                Some(current) => current,
                None => match self.pending.pop_front() {
                    Some(next) => self.current.insert(next),
                    None => return Some(Update::Completed),
                },
            };
            match current.next(deadline)? {
                Update::Completed => self.current = None,
                update => return Some(update),
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        self.pending.clear();
        match &mut self.current {
            Some(current) => current.cancel(grace),
            None => self.current = Some(Box::new(Scripted::new([Update::Stopped]))),
        }
    }
}
