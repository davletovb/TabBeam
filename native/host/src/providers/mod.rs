//! Provider adapters (PRO-01).
//!
//! The adapter contract is provisional until a second real provider works
//! (framework §10.1). A [`Provider`] reports its status and serves requests
//! through [`Exchange`]s: state machines the host loop drives, which never
//! block past the deadline they are given. That lets one host serve several
//! requests, and read cancellations, while providers work.
//!
//! Everything provider-specific stays inside the adapter: command lines,
//! output formats, and provider session IDs. The host sees only [`Update`]s in
//! protocol terms, and the popup sees only protocol events.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::conversation::HistoryMessage;
use crate::protocol::events::{ErrorBody, ProviderState};

pub mod codex;
pub mod discovery;
pub mod fake;

/// One `conversation.send`, in provider-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub text: String,
    /// Previous user and assistant messages, in order. Adapters without a
    /// native continuation can use these to reconstruct the dialogue.
    pub history: Vec<HistoryMessage>,
    /// The conversation to continue, or `None` to start one.
    pub conversation_id: Option<String>,
    /// Whether the request attaches browser context (`payload.context`). An
    /// adapter whose provider can't use it fails the request rather than
    /// answer without it (DOC-02 §6).
    pub has_context: bool,
}

/// What an exchange reports, in protocol order. After a terminal update
/// (`Completed`, `Failed`, or `Stopped`), the exchange is finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// A new provider-neutral conversation (`conversation.created`). Comes
    /// before `Started`.
    ConversationCreated(String),
    /// Response production started (`response.started`).
    Started { conversation_id: Option<String> },
    /// The next piece of the answer (`response.delta`).
    Delta(String),
    /// One provider's status (`provider.status`).
    Status {
        provider_id: String,
        status: ProviderState,
    },
    /// The provider is working without anything to show, for example while
    /// a tool runs. Resets the idle timeout.
    Activity,
    /// Terminal: the request succeeded.
    Completed,
    /// Terminal: the request failed.
    Failed(ErrorBody<'static>),
    /// Terminal: the exchange stopped after [`Exchange::cancel`].
    Stopped,
}

impl Update {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed(_) | Self::Stopped)
    }
}

/// One request being served.
pub trait Exchange {
    /// Returns the next update, waiting until `deadline` at most, or `None` if
    /// the deadline passes first.
    fn next(&mut self, deadline: Instant) -> Option<Update>;

    /// Stops the work. Updates already produced may still arrive, then
    /// `Stopped`, or another terminal update if the work ended first. A
    /// process that outlives `grace` after being asked to stop is killed.
    fn cancel(&mut self, grace: Duration);
}

/// How long the host lets a provider's requests take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// From the request to `response.started`.
    pub start: Duration,
    /// Between updates once the response has started.
    pub idle: Duration,
    /// How long a cancelled or timed-out request may take to stop before its
    /// process is killed.
    pub stop_grace: Duration,
}

/// A provider adapter.
pub trait Provider {
    /// The provider ID requests name, such as `codex`.
    fn id(&self) -> &str;

    fn timeouts(&self) -> Timeouts;

    /// Starts checking availability, authentication, and capabilities. The
    /// exchange reports one `Status` and then `Completed`.
    fn status(&self) -> Box<dyn Exchange>;

    /// Starts serving `request`.
    fn send(&self, request: SendRequest) -> Box<dyn Exchange>;
}

/// The providers a host serves, in the order `provider.status` reports them.
pub struct Providers(Vec<Box<dyn Provider>>);

impl Providers {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self(providers)
    }

    /// The providers of an installed host: the fake scaffold and Codex, found
    /// by the platform lookup rules in [`codex::Codex::installed`].
    pub fn installed() -> Self {
        Self(vec![
            Box::new(fake::Fake),
            Box::new(codex::Codex::installed()),
        ])
    }

    /// Only the deterministic fake scaffold, which starts no processes: for
    /// fuzzing and protocol tests.
    pub fn scaffold() -> Self {
        Self(vec![Box::new(fake::Fake)])
    }

    pub fn get(&self, id: &str) -> Option<&dyn Provider> {
        self.iter().find(|provider| provider.id() == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn Provider> {
        self.0.iter().map(AsRef::as_ref)
    }
}

/// An exchange whose updates are all known when it starts.
pub struct Scripted(VecDeque<Update>);

impl Scripted {
    pub fn new(updates: impl IntoIterator<Item = Update>) -> Self {
        Self(updates.into_iter().collect())
    }

    /// An exchange that fails at once with `error`.
    pub fn failed(error: ErrorBody<'static>) -> Self {
        Self::new([Update::Failed(error)])
    }
}

impl Exchange for Scripted {
    fn next(&mut self, _deadline: Instant) -> Option<Update> {
        self.0.pop_front()
    }

    fn cancel(&mut self, _grace: Duration) {
        if !self.0.is_empty() {
            self.0 = VecDeque::from([Update::Stopped]);
        }
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
