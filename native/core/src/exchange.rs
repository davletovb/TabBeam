//! Bounded-deadline provider exchange events, shared by Codex and Claude.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::protocol::{ErrorBody, ProviderState};

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
    /// the deadline passes first. Output that keeps arriving without an update
    /// may hold the call at most [`crate::stream::BUSY_LIMIT`] past the deadline.
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
