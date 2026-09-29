//! Conversations over a runtime [`Provider`].
//!
//! The runtime runs turns; it knows no conversations. This layer is where
//! Pervue's conversations meet it. For each request it:
//!
//! - frames the request's history and browser context into the turn's
//!   messages, and picks the turn's tool policy;
//! - owns the conversation IDs the extension sees, and the map from each
//!   conversation to the native session its provider keeps for it, so a
//!   conversation resumes after a restart ([`SessionStore`]);
//! - decides when a turn continues a native session, when it starts a new one
//!   (a search turn never resumes a session that may hold page text), and when
//!   the dialogue is replayed from history instead;
//! - records the sessions a conversation leaves behind, and has the provider
//!   remove their saved transcripts, now or after a restart;
//! - recovers when a provider says a session is gone;
//! - forgets a conversation, and everything the provider saved for it.
//!
//! A provider that keeps no native session needs none of that but a
//! conversation ID.

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::conversation::{BrowserContext, HistoryMessage, Role as HistoryRole, provider_prompt};
use crate::providers::{
    Cleanup, ConversationProvider, ConversationSlot, Exchange, Provider, Scripted, SendRequest,
    Timeouts, Update, forget, private_fs,
};
use runtime_core::exchange::SessionLoss;
use runtime_core::protocol::{Capabilities, ErrorCode, Failure};
use runtime_core::turn::{Message, Role, SessionPolicy, ToolPolicy, Turn};

mod store;

pub use store::{Durability, SessionStore};

/// A conversation ID the layer doesn't know, and no history to rebuild it.
pub const UNKNOWN_CONVERSATION: Failure = Failure {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    retryable: false,
};

/// A conversation's session mapping couldn't be stored.
pub const SESSION_STORE_FAILED: Failure = Failure {
    code: ErrorCode::InternalError,
    reason: "SESSION_STORE_FAILED",
    retryable: true,
};

/// A provider, with the conversations Pervue keeps over it.
pub struct Conversations<P: Provider> {
    provider: Rc<P>,
    store: Rc<SessionStore>,
}

impl<P: Provider + 'static> Conversations<P> {
    pub fn new(provider: P, store: SessionStore) -> Self {
        Self {
            provider: Rc::new(provider),
            store: Rc::new(store),
        }
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }

    pub fn store(&self) -> &SessionStore {
        &self.store
    }
}

/// What one request says, before it becomes turns.
struct Draft {
    text: String,
    history: Vec<HistoryMessage>,
    context: Option<BrowserContext>,
    model: Option<String>,
    tools: ToolPolicy,
    session: SessionPolicy,
}

impl Draft {
    /// A turn for this request. With `with_history`, the earlier dialogue goes
    /// with it as messages, for a provider with no native session to resume.
    /// The current message is framed with the label that sets the question
    /// apart from what comes before it: when there is dialogue, page context,
    /// or a search; a lone plain question is sent as it is.
    fn turn(&self, with_history: bool, continuation: Option<String>, group: Option<&str>) -> Turn {
        let earlier: &[HistoryMessage] = if with_history { &self.history } else { &[] };
        let framed = !earlier.is_empty()
            || self.context.is_some()
            || self.tools == ToolPolicy::NativeWebSearch;
        let current = if framed {
            provider_prompt(&[], self.context.as_ref(), &self.text)
        } else {
            self.text.clone()
        };
        let mut messages: Vec<Message> = earlier
            .iter()
            .map(|message| Message {
                role: match message.role {
                    HistoryRole::User => Role::User,
                    HistoryRole::Assistant => Role::Assistant,
                },
                text: message.text.clone(),
            })
            .collect();
        messages.push(Message {
            role: Role::User,
            text: current,
        });
        Turn {
            system: None,
            messages,
            model: self.model.clone(),
            tools: self.tools,
            session: self.session,
            continuation,
            cleanup_group: group.map(str::to_owned),
            // Pervue checks the sign-in before every turn of a provider that
            // can be asked to.
            check_sign_in: true,
        }
    }
}

impl<P: Provider + 'static> ConversationProvider for Conversations<P> {
    fn id(&self) -> &str {
        self.provider.id()
    }

    fn timeouts(&self) -> Timeouts {
        self.provider.timeouts()
    }

    fn capabilities(&self) -> Capabilities {
        self.provider.capabilities()
    }

    fn supports_persistent_session(&self) -> bool {
        self.provider.supports_persistent_session()
    }

    fn status(&self) -> Box<dyn Exchange> {
        self.provider.status()
    }

    fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
        let SendRequest {
            text,
            history,
            conversation_id,
            context,
            model,
            native_search,
            session_policy,
            fresh_session,
            conversation: slot,
        } = request;
        let tools = if native_search {
            ToolPolicy::NativeWebSearch
        } else if context.is_some() {
            ToolPolicy::None
        } else {
            ToolPolicy::ProviderDefault
        };
        let draft = Draft {
            text,
            history,
            context,
            model,
            tools,
            session: session_policy,
        };
        let persistent = self.provider.supports_persistent_session()
            && session_policy == SessionPolicy::Persistent;
        if persistent {
            self.send_persistent(draft, conversation_id, fresh_session, slot)
        } else {
            self.send_stateless(&draft, conversation_id, &slot)
        }
    }

    fn forget(&self, conversation_id: &str) -> Box<dyn Exchange> {
        let sessions = self.store.sessions_of(conversation_id);
        let sessions = if sessions.is_empty() {
            Cleanup::nothing()
        } else {
            self.provider.cleanup_sessions(&sessions)
        };
        let Cleanup { work, completed } =
            sessions.then(self.provider.cleanup_group(conversation_id));
        let removal = self.store.files_to_remove(conversation_id);
        let store = Rc::clone(&self.store);
        let conversation = conversation_id.to_owned();
        // The provider's files go first, on a thread of their own, and the
        // mappings last, the one in memory only once everything else is
        // gone: if the provider's files can't all be removed, a retry can
        // still find them.
        forget::in_background(
            move || {
                work()?;
                removal.map_or(Ok(()), store::Removal::run)
            },
            move || {
                completed();
                store.forget_memory(&conversation);
            },
        )
    }
}

impl<P: Provider + 'static> Conversations<P> {
    /// A provider that keeps no native session: every turn carries the whole
    /// dialogue, and the conversation is only an ID.
    fn send_stateless(
        &self,
        draft: &Draft,
        conversation_id: Option<String>,
        slot: &ConversationSlot,
    ) -> Box<dyn Exchange> {
        if conversation_id
            .as_deref()
            .is_some_and(|id| !private_fs::is_conversation_id(id))
        {
            return Box::new(Scripted::failed(UNKNOWN_CONVERSATION));
        }
        let created = conversation_id.is_none();
        let id = conversation_id.unwrap_or_else(private_fs::new_conversation_id);
        slot.set(&id, created);
        self.provider.send(draft.turn(true, None, Some(&id)))
    }

    /// A provider whose sessions the conversation keeps and resumes.
    fn send_persistent(
        &self,
        draft: Draft,
        conversation_id: Option<String>,
        fresh_session: bool,
        slot: ConversationSlot,
    ) -> Box<dyn Exchange> {
        let prior = conversation_id
            .as_deref()
            .and_then(|conversation| self.store.get(conversation));
        if conversation_id.is_some() && prior.is_none() && draft.history.is_empty() {
            return Box::new(Scripted::failed(UNKNOWN_CONVERSATION));
        }
        // A fresh session (a search turn) never resumes: the session may hold
        // page text from an earlier turn, and search must not see it.
        let resume = if fresh_session { None } else { prior.clone() };
        let group = conversation_id.as_deref();
        let replay = (!draft.history.is_empty()).then(|| draft.turn(true, None, group));
        let (first, fallback, conversation) = match replay {
            // A fresh session for a conversation that has one: the dialogue
            // stands in for it, under the same conversation.
            Some(replay) if fresh_session && prior.is_some() => (replay, None, conversation_id),
            // No session to resume, but the dialogue: a new conversation.
            Some(replay) if resume.is_none() => (replay, None, None),
            replay => (
                draft.turn(false, resume.clone(), group),
                replay,
                conversation_id,
            ),
        };
        if let Some(conversation) = &conversation {
            slot.set(conversation, false);
        }
        let superseded = if fresh_session { prior } else { None };
        Box::new(ConversationExchange {
            inner: self.provider.send(first),
            provider: Rc::clone(&self.provider),
            store: Rc::clone(&self.store),
            slot,
            conversation,
            resumed: resume,
            superseded,
            fallback,
            recovering: false,
            lost: None,
            saw_delta: false,
            started: false,
            cancelled: false,
            queue: VecDeque::new(),
            finished: false,
        })
    }
}

/// One turn of a conversation over a provider with native sessions, as an
/// exchange: the provider's, with the session bookkeeping done as its updates
/// pass.
struct ConversationExchange<P: Provider> {
    inner: Box<dyn Exchange>,
    provider: Rc<P>,
    store: Rc<SessionStore>,
    slot: ConversationSlot,
    /// The conversation being served, or `None` until a new one gets its
    /// first session.
    conversation: Option<String>,
    /// The session this run resumes.
    resumed: Option<String>,
    /// The session this run replaces, when it forks a new one.
    superseded: Option<String>,
    /// What to run instead, once, if the session turns out to be lost.
    fallback: Option<Turn>,
    /// The run is a rebuild under the same conversation, and its session
    /// replaces the lost one.
    recovering: bool,
    lost: Option<SessionLoss>,
    saw_delta: bool,
    /// `Started` went to the host; a rebuild after that doesn't send it again.
    started: bool,
    cancelled: bool,
    queue: VecDeque<Update>,
    finished: bool,
}

impl<P: Provider + 'static> ConversationExchange<P> {
    fn handle(&mut self, update: Update) {
        match update {
            Update::Session(handle) => match self.on_session(&handle) {
                Ok(()) => self.queue.push_back(Update::Session(handle)),
                Err(failure) => self.fail(failure),
            },
            Update::SessionLost(loss) => self.lost = Some(loss),
            Update::Started { .. } => self.on_started(),
            Update::Delta(text) => {
                self.saw_delta = true;
                self.queue.push_back(Update::Delta(text));
            }
            Update::Failed(failure) => self.on_failed(failure),
            update @ (Update::Completed | Update::Stopped) => self.end(update),
            update => self.queue.push_back(update),
        }
    }

    /// Keeps the mapping in step with the session a turn reports.
    fn on_session(&mut self, handle: &str) -> Result<(), Failure> {
        let stored = |result: std::io::Result<()>| result.map_err(|_| SESSION_STORE_FAILED);
        let Some(conversation) = self.conversation.clone() else {
            // The first session of a new conversation, stored before the
            // conversation is announced: a conversation that couldn't be
            // resumed never starts.
            let conversation = self.store.new_id();
            stored(self.store.remember_new(&conversation, handle))?;
            self.slot.set(&conversation, true);
            self.conversation = Some(conversation);
            self.resumed = Some(handle.to_owned());
            return Ok(());
        };
        if self.recovering {
            // A rebuild replaces the lost session behind the same
            // conversation, so the extension sees no new one.
            stored(self.store.remember(&conversation, handle))?;
            self.recovering = false;
        } else if let Some(old) = self.superseded.take() {
            let marker = self
                .store
                .record_superseded(&conversation, &old)
                .map_err(|_| SESSION_STORE_FAILED)?;
            stored(self.store.remember(&conversation, handle))?;
            self.remove_transcripts(marker, old);
        } else {
            // A session that differs from the mapped one: a resumed session
            // can be forked. The answer is already coming, so a failed write
            // doesn't fail the turn.
            let current = self.store.get(&conversation).or(self.resumed.clone());
            if current.as_deref() != Some(handle) {
                self.store.follow(&conversation, handle);
                if let Some(old) = current {
                    // Without a marker that can be retried, a removal that
                    // fails would be forgotten: don't start one.
                    if let Ok(marker) = self.store.record_superseded(&conversation, &old) {
                        self.remove_transcripts(marker, old);
                    }
                }
            }
        }
        self.resumed = Some(handle.to_owned());
        Ok(())
    }

    /// Removes a session's saved transcripts away from the request, keeping
    /// the marker until that worked.
    fn remove_transcripts(&self, marker: Option<std::path::PathBuf>, session: String) {
        let cleanup = self.provider.cleanup_sessions(&[session]);
        forget::tracked_cleanup(marker, cleanup.work);
    }

    fn on_started(&mut self) {
        if self.started {
            return;
        }
        if self.conversation.is_none() {
            // A persistent turn that started without saying which session it
            // runs in can't be resumed.
            return self.fail(SESSION_STORE_FAILED);
        }
        self.started = true;
        self.queue.push_back(Update::Started {
            conversation_id: None,
        });
    }

    fn on_failed(&mut self, failure: Failure) {
        if self.cancelled || self.saw_delta {
            return self.end(Update::Failed(failure));
        }
        match (self.lost.take(), self.fallback.take()) {
            // The provider said the session is gone: forget it, then rebuild
            // from the dialogue under the same conversation, or say so.
            (Some(SessionLoss::Confirmed), fallback) => {
                if let Some(conversation) = &self.conversation {
                    self.store.drop_mapping(conversation);
                }
                match fallback {
                    Some(turn) => self.rebuild(turn, true),
                    None => self.end(Update::Failed(UNKNOWN_CONVERSATION)),
                }
            }
            // A resumed run that ended before the turn began, for a reason
            // the provider didn't give: the mapping may be fine, so it stays,
            // and the dialogue starts a new conversation.
            (Some(SessionLoss::Suspected), Some(turn)) => self.rebuild(turn, false),
            _ => self.end(Update::Failed(failure)),
        }
    }

    /// Runs `turn` in place of the run that couldn't resume.
    fn rebuild(&mut self, turn: Turn, same_conversation: bool) {
        self.resumed = None;
        self.superseded = None;
        self.saw_delta = false;
        if same_conversation {
            self.recovering = true;
        } else {
            self.conversation = None;
        }
        self.inner = self.provider.send(turn);
    }

    fn fail(&mut self, failure: Failure) {
        // Dropping the run kills and reaps its process.
        self.inner = Box::new(Scripted::new([]));
        self.end(Update::Failed(failure));
    }

    fn end(&mut self, terminal: Update) {
        self.queue.push_back(terminal);
        self.finished = true;
    }
}

impl<P: Provider + 'static> Exchange for ConversationExchange<P> {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        loop {
            if let Some(update) = self.queue.pop_front() {
                return Some(update);
            }
            if self.finished {
                return None;
            }
            let update = self.inner.next(deadline)?;
            self.handle(update);
        }
    }

    fn cancel(&mut self, grace: Duration) {
        if self.queue.iter().any(Update::is_terminal) {
            // The turn already ended; the caller hasn't seen it yet.
            self.queue = VecDeque::from([Update::Stopped]);
            return;
        }
        if self.finished || self.cancelled {
            return;
        }
        self.cancelled = true;
        self.inner.cancel(grace);
    }
}

#[cfg(test)]
mod tests;
