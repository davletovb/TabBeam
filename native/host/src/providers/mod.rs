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

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::conversation::{BrowserContext, HistoryMessage};
use crate::conversations::{Conversations, Durability, SessionStore};
use runtime_core::protocol::{Capabilities, ErrorCode, Failure};
pub use runtime_core::stream::BUSY_LIMIT;
use runtime_core::turn::{SessionPolicy, Turn};

pub mod claude;
pub mod codex;
pub mod fake;
pub mod gemini;
pub mod grok;

pub use runtime_platform::layout::Layout;

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
    /// Where the conversation layer tells the host which conversation this
    /// request serves, and whether it just created it.
    pub conversation: ConversationSlot,
}

/// The conversation a request serves. The host creates one per request and
/// reads it when the response starts; the layer that owns conversation IDs
/// fills it in. It replaces the conversation events providers used to send,
/// which are Pervue's protocol vocabulary, not the runtime's.
#[derive(Debug, Clone, Default)]
pub struct ConversationSlot(Rc<RefCell<SlotState>>);

#[derive(Debug, Default)]
struct SlotState {
    id: Option<String>,
    created: bool,
}

impl ConversationSlot {
    /// The conversation the request serves, once known.
    pub fn id(&self) -> Option<String> {
        self.0.borrow().id.clone()
    }

    /// Whether the request created that conversation, so the host announces
    /// it before the response starts.
    pub fn created(&self) -> bool {
        self.0.borrow().created
    }

    pub fn set(&self, id: impl Into<String>, created: bool) {
        *self.0.borrow_mut() = SlotState {
            id: Some(id.into()),
            created,
        };
    }
}

impl PartialEq for ConversationSlot {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ConversationSlot {}

pub use runtime_core::exchange::{Exchange, Scripted, Timeouts, Update};

pub(crate) fn keep_bounded_output(output: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let remaining = limit.saturating_sub(output.len());
    output.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

/// File-system work that removes what a provider saved. It owns everything it
/// needs, so it runs on a thread of its own and never holds up the caller.
pub struct Cleanup {
    /// The removal itself, on that thread.
    pub work: Box<dyn FnOnce() -> io::Result<()> + Send>,
    /// Runs on the caller's thread once `work` succeeded, and never
    /// otherwise: what it drops, such as an in-memory record, stays for a
    /// retry after a failure.
    pub completed: Box<dyn FnOnce()>,
}

impl Cleanup {
    /// Nothing to remove.
    pub fn nothing() -> Self {
        Self::new(|| Ok(()), || {})
    }

    pub fn new(
        work: impl FnOnce() -> io::Result<()> + Send + 'static,
        completed: impl FnOnce() + 'static,
    ) -> Self {
        Self {
            work: Box::new(work),
            completed: Box::new(completed),
        }
    }

    /// This cleanup, then `next`: the second runs only if the first
    /// succeeded, and both `completed` steps run once both worked.
    #[must_use]
    pub fn then(self, next: Self) -> Self {
        let (first, second) = (self.work, next.work);
        let (first_done, second_done) = (self.completed, next.completed);
        Self::new(
            move || {
                first()?;
                second()
            },
            move || {
                first_done();
                second_done();
            },
        )
    }
}

/// A turn a caller built wrongly: an adapter refuses it instead of passing
/// what it holds to a command line.
pub const INVALID_TURN: Failure = Failure {
    code: ErrorCode::InvalidRequest,
    reason: "INVALID_TURN",
    retryable: false,
};

/// One execution mode of a provider CLI, in terms that belong to no
/// application: it runs a [`Turn`], and knows nothing of conversations, of
/// browsers, or of which sessions an application keeps.
pub trait Provider {
    /// The provider ID requests name, such as `codex`.
    fn id(&self) -> &str;

    fn timeouts(&self) -> Timeouts;

    /// What this mode can do. It can change while the runtime runs, for
    /// example when the user's own provider configuration changes.
    fn capabilities(&self) -> Capabilities;

    /// Whether this mode can keep a native session and resume it by an opaque
    /// handle. Only such modes accept a persistent [`Turn`].
    fn supports_persistent_session(&self) -> bool {
        false
    }

    /// Starts checking availability, authentication, and capabilities. The
    /// exchange reports one `Status` and then `Completed`.
    fn status(&self) -> Box<dyn Exchange>;

    /// Starts serving `turn`. A persistent turn reports the native session it
    /// runs in, before `Started`, as an opaque handle.
    fn send(&self, turn: Turn) -> Box<dyn Exchange>;

    /// Removes what the provider saved for these native sessions, where the
    /// adapter can prove the provider wrote it for this application.
    fn cleanup_sessions(&self, sessions: &[String]) -> Cleanup {
        let _ = sessions;
        Cleanup::nothing()
    }

    /// Retries the per-turn cleanups that failed earlier for the turns of
    /// `group` (`Turn::cleanup_group`).
    fn cleanup_group(&self, group: &str) -> Cleanup {
        let _ = group;
        Cleanup::nothing()
    }
}

/// A provider as the host serves it: a conversation the extension names, with
/// its history and browser context, over a runtime [`Provider`].
pub trait ConversationProvider {
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
pub struct Providers(Vec<Box<dyn ConversationProvider>>);

impl Providers {
    pub fn new(providers: Vec<Box<dyn ConversationProvider>>) -> Self {
        Self(providers)
    }

    /// The providers of an installed host. The fake scaffold stays registered
    /// for deterministic protocol diagnostics; real adapters use the same
    /// platform discovery rules.
    pub fn installed(layout: &Layout) -> Self {
        let data = layout.data_dir();
        let sessions = |name: &str| SessionStore::new(data.as_ref().map(|dir| dir.join(name)));
        Self(vec![
            Box::new(fake::Fake),
            // Codex refuses to start a conversation it couldn't resume after a
            // restart; Claude keeps one in memory when there is no directory.
            Box::new(Conversations::new(
                codex::Codex::installed(layout),
                sessions("codex-sessions").with_durability(Durability::Required),
            )),
            Box::new(Conversations::new(
                claude::Claude::installed(layout),
                sessions("claude-sessions"),
            )),
            Box::new(Conversations::new(
                gemini::Gemini::installed(layout),
                SessionStore::new(None),
            )),
            Box::new(Conversations::new(
                grok::Grok::installed(layout),
                SessionStore::new(None),
            )),
        ])
    }

    /// Only the deterministic fake scaffold, which starts no processes: for
    /// fuzzing and protocol tests.
    pub fn scaffold() -> Self {
        Self(vec![Box::new(fake::Fake)])
    }

    pub fn get(&self, id: &str) -> Option<&dyn ConversationProvider> {
        self.0
            .iter()
            .map(Box::as_ref)
            .find(|provider| provider.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn ConversationProvider> {
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
            pending: providers.iter().map(ConversationProvider::status).collect(),
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
