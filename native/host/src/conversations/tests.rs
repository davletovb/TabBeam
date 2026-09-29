use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use super::*;
use crate::conversation::HistoryMessage;
use runtime_core::protocol::Capability;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "pervue-conversations-{}-{:x}",
            std::process::id(),
            RandomState::new().hash_one(SystemTime::now())
        )))
    }

    fn store(&self) -> SessionStore {
        SessionStore::new(Some(self.0.clone()))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A provider that answers from scripts and records what it was asked.
struct Recorder {
    persistent: bool,
    scripts: RefCell<VecDeque<Vec<Update>>>,
    turns: RefCell<Vec<Turn>>,
    removed_sessions: Arc<Mutex<Vec<String>>>,
    retried_groups: Arc<Mutex<Vec<String>>>,
    cleanup_fails: Arc<AtomicBool>,
}

impl Recorder {
    fn new(persistent: bool) -> Self {
        Self {
            persistent,
            scripts: RefCell::default(),
            turns: RefCell::default(),
            removed_sessions: Arc::default(),
            retried_groups: Arc::default(),
            cleanup_fails: Arc::default(),
        }
    }

    fn script(self, updates: Vec<Update>) -> Self {
        self.scripts.borrow_mut().push_back(updates);
        self
    }

    fn turns(conversations: &Conversations<Self>) -> Vec<Turn> {
        conversations.provider().turns.borrow().clone()
    }
}

impl Provider for Recorder {
    fn id(&self) -> &str {
        "test"
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts {
            start: Duration::from_secs(1),
            idle: Duration::from_secs(1),
            max_turn: Duration::MAX,
            stop_grace: Duration::ZERO,
        }
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: Capability::Supported,
            continuation: Capability::Supported,
            web_search: Capability::Supported,
            model_selection: Capability::Unsupported,
            cancellation: Capability::Supported,
            tool_isolation: Capability::Supported,
        }
    }

    fn supports_persistent_session(&self) -> bool {
        self.persistent
    }

    fn status(&self) -> Box<dyn Exchange> {
        Box::new(Scripted::new([Update::Completed]))
    }

    fn send(&self, turn: Turn) -> Box<dyn Exchange> {
        self.turns.borrow_mut().push(turn);
        let script = self
            .scripts
            .borrow_mut()
            .pop_front()
            .expect("a script for every turn");
        Box::new(Scripted::new(script))
    }

    fn cleanup_sessions(&self, sessions: &[String]) -> Cleanup {
        let (removed, fails) = (
            Arc::clone(&self.removed_sessions),
            Arc::clone(&self.cleanup_fails),
        );
        let sessions = sessions.to_vec();
        Cleanup::new(
            move || {
                if fails.load(Ordering::SeqCst) {
                    return Err(std::io::Error::other("cleanup failed"));
                }
                removed.lock().unwrap().extend(sessions);
                Ok(())
            },
            || {},
        )
    }

    fn cleanup_group(&self, group: &str) -> Cleanup {
        let retried = Arc::clone(&self.retried_groups);
        let group = group.to_owned();
        Cleanup::new(
            move || {
                retried.lock().unwrap().push(group);
                Ok(())
            },
            || {},
        )
    }
}

fn answer(session: &str) -> Vec<Update> {
    vec![
        Update::Launched,
        Update::Session(session.to_owned()),
        Update::Started {
            conversation_id: None,
        },
        Update::Delta("ok".to_owned()),
        Update::Completed,
    ]
}

fn request(text: &str) -> SendRequest {
    SendRequest {
        text: text.to_owned(),
        history: Vec::new(),
        conversation_id: None,
        context: None,
        model: None,
        native_search: false,
        session_policy: SessionPolicy::Persistent,
        fresh_session: false,
        conversation: ConversationSlot::default(),
    }
}

fn history() -> Vec<HistoryMessage> {
    vec![
        HistoryMessage {
            role: HistoryRole::User,
            text: "first question".to_owned(),
        },
        HistoryMessage {
            role: HistoryRole::Assistant,
            text: "first answer".to_owned(),
        },
    ]
}

fn run(mut exchange: Box<dyn Exchange>) -> Vec<Update> {
    let mut updates = Vec::new();
    while let Some(update) = exchange.next(Instant::now() + Duration::from_secs(5)) {
        let terminal = update.is_terminal();
        updates.push(update);
        if terminal {
            return updates;
        }
    }
    panic!("the exchange ended without a terminal update: {updates:?}");
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn failure(updates: &[Update]) -> Failure {
    match updates.last() {
        Some(Update::Failed(failure)) => *failure,
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Starts a conversation and returns its ID.
fn first_conversation(conversations: &Conversations<Recorder>) -> String {
    let first = request("first question");
    let slot = first.conversation.clone();
    assert_eq!(
        run(conversations.send(first)).last(),
        Some(&Update::Completed)
    );
    assert!(slot.created());
    slot.id().expect("a conversation ID")
}

fn text_of(turn: &Turn) -> Vec<&str> {
    turn.messages.iter().map(|m| m.text.as_str()).collect()
}

#[test]
fn a_provider_without_sessions_gets_the_whole_dialogue_and_an_id() {
    let conversations = Conversations::new(
        Recorder::new(false)
            .script(vec![Update::Completed])
            .script(vec![Update::Completed]),
        SessionStore::new(None),
    );
    let mut first = request("first question");
    first.session_policy = SessionPolicy::Ephemeral;
    let slot = first.conversation.clone();
    run(conversations.send(first));
    let id = slot.id().expect("an ID");
    assert!(slot.created());
    assert!(private_fs::is_conversation_id(&id));

    let mut follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        history: history(),
        ..request("follow up")
    };
    follow_up.session_policy = SessionPolicy::Ephemeral;
    let slot = follow_up.conversation.clone();
    run(conversations.send(follow_up));
    assert_eq!(slot.id().as_deref(), Some(id.as_str()));
    assert!(!slot.created());

    let turns = Recorder::turns(&conversations);
    // A lone plain question goes as it is; a follow-up carries the dialogue,
    // with the question after it.
    assert_eq!(text_of(&turns[0]), ["first question"]);
    assert_eq!(
        text_of(&turns[1]),
        [
            "first question",
            "first answer",
            "Current user question:\nfollow up"
        ]
    );
    for turn in &turns {
        assert_eq!(turn.session, SessionPolicy::Ephemeral);
        assert_eq!(turn.continuation, None);
        assert_eq!(turn.cleanup_group.as_deref(), Some(id.as_str()));
        assert_eq!(turn.tools, ToolPolicy::ProviderDefault);
    }
}

#[test]
fn a_provider_without_sessions_refuses_a_malformed_conversation_id() {
    let conversations = Conversations::new(Recorder::new(false), SessionStore::new(None));
    let mut bad = request("hi");
    bad.session_policy = SessionPolicy::Ephemeral;
    bad.conversation_id = Some("../escape".to_owned());
    assert_eq!(failure(&run(conversations.send(bad))), UNKNOWN_CONVERSATION);
    assert!(Recorder::turns(&conversations).is_empty());
}

#[test]
fn tools_follow_what_the_request_carries() {
    let conversations = Conversations::new(
        Recorder::new(false)
            .script(vec![Update::Completed])
            .script(vec![Update::Completed]),
        SessionStore::new(None),
    );
    let context = crate::conversation::BrowserContext {
        mode: crate::conversation::BrowserContextMode::Selection,
        text: "page text".to_owned(),
        truncated: false,
        page: crate::conversation::BrowserPageContext {
            title: "T".to_owned(),
            url: "https://example.com/".to_owned(),
        },
    };
    for (native_search, context) in [(true, None), (false, Some(context))] {
        let mut turn = request("Explain");
        turn.session_policy = SessionPolicy::Ephemeral;
        turn.native_search = native_search;
        turn.context = context;
        run(conversations.send(turn));
    }
    let turns = Recorder::turns(&conversations);
    assert_eq!(turns[0].tools, ToolPolicy::NativeWebSearch);
    assert_eq!(turns[0].messages[0].text, "Current user question:\nExplain");
    assert_eq!(turns[1].tools, ToolPolicy::None);
    let framed = &turns[1].messages[0].text;
    assert!(framed.contains("untrusted reference data"), "{framed}");
    assert!(
        framed.ends_with("Current user question:\nExplain"),
        "{framed}"
    );
}

#[test]
fn a_new_conversation_is_announced_with_its_mapping_already_stored() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")),
        scratch.store(),
    );
    let request = request("first question");
    let slot = request.conversation.clone();
    let updates = run(conversations.send(request));
    assert_eq!(
        updates,
        [
            Update::Launched,
            Update::Session("session-1".to_owned()),
            Update::Started {
                conversation_id: None
            },
            Update::Delta("ok".to_owned()),
            Update::Completed,
        ]
    );
    let id = slot.id().expect("a conversation ID");
    assert!(slot.created());
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-1"));
    // On disk too: a restarted host finds it.
    assert_eq!(scratch.store().get(&id).as_deref(), Some("session-1"));
    let turn = &Recorder::turns(&conversations)[0];
    assert_eq!(turn.continuation, None);
    assert_eq!(turn.session, SessionPolicy::Persistent);
}

#[test]
fn a_follow_up_resumes_the_conversations_session_after_a_restart() {
    let scratch = Scratch::new();
    let id = {
        let conversations = Conversations::new(
            Recorder::new(true).script(answer("session-1")),
            scratch.store(),
        );
        first_conversation(&conversations)
    };
    let restarted = Conversations::new(
        Recorder::new(true).script(answer("session-1")),
        scratch.store(),
    );
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        ..request("again")
    };
    let slot = follow_up.conversation.clone();
    assert_eq!(
        run(restarted.send(follow_up)).last(),
        Some(&Update::Completed)
    );
    assert!(!slot.created());
    assert_eq!(slot.id().as_deref(), Some(id.as_str()));
    let turn = &Recorder::turns(&restarted)[0];
    assert_eq!(turn.continuation.as_deref(), Some("session-1"));
    assert_eq!(text_of(turn), ["again"]);
}

#[test]
fn an_unknown_conversation_without_history_fails_before_the_provider_runs() {
    let conversations = Conversations::new(Recorder::new(true), SessionStore::new(None));
    let request = SendRequest {
        conversation_id: Some("conv_0123456789abcdef".to_owned()),
        ..request("again")
    };
    assert_eq!(
        failure(&run(conversations.send(request))),
        UNKNOWN_CONVERSATION
    );
    assert!(Recorder::turns(&conversations).is_empty());
}

#[test]
fn an_unknown_conversation_with_history_starts_a_new_one_from_the_dialogue() {
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-9")),
        SessionStore::new(None),
    );
    let request = SendRequest {
        conversation_id: Some("conv_0123456789abcdef".to_owned()),
        history: history(),
        ..request("follow up")
    };
    let slot = request.conversation.clone();
    run(conversations.send(request));
    assert!(slot.created());
    assert_ne!(slot.id().as_deref(), Some("conv_0123456789abcdef"));
    let turn = &Recorder::turns(&conversations)[0];
    assert_eq!(turn.continuation, None);
    assert_eq!(text_of(turn)[1], "first answer");
}

#[test]
fn a_search_turn_forks_a_session_and_removes_the_one_it_replaced() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true)
            .script(answer("session-1"))
            .script(answer("session-2")),
        scratch.store(),
    );
    let id = first_conversation(&conversations);

    let search = SendRequest {
        conversation_id: Some(id.clone()),
        history: history(),
        native_search: true,
        fresh_session: true,
        ..request("look it up")
    };
    let slot = search.conversation.clone();
    assert_eq!(
        run(conversations.send(search)).last(),
        Some(&Update::Completed)
    );

    // The fork never resumes the session that may hold page text; the
    // dialogue stands in for it, under the same conversation.
    let turn = &Recorder::turns(&conversations)[1];
    assert_eq!(turn.continuation, None);
    assert_eq!(turn.tools, ToolPolicy::NativeWebSearch);
    assert_eq!(text_of(turn)[1], "first answer");
    assert!(!slot.created());
    assert_eq!(slot.id().as_deref(), Some(id.as_str()));

    // The conversation moved to the new session; the old one's transcripts
    // are removed in the background and its marker goes with them.
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-2"));
    let removed = Arc::clone(&conversations.provider().removed_sessions);
    wait_until("the superseded session's cleanup", || {
        removed.lock().unwrap().as_slice() == ["session-1"]
    });
    wait_until("the marker to go", || {
        scratch.store().superseded(&id).is_empty()
    });
}

#[test]
fn a_failed_cleanup_leaves_its_marker_for_a_later_forget() {
    let scratch = Scratch::new();
    let recorder = Recorder::new(true)
        .script(answer("session-1"))
        .script(answer("session-2"));
    recorder.cleanup_fails.store(true, Ordering::SeqCst);
    let conversations = Conversations::new(recorder, scratch.store());
    let id = first_conversation(&conversations);
    let search = SendRequest {
        conversation_id: Some(id.clone()),
        native_search: true,
        fresh_session: true,
        ..request("look it up")
    };
    run(conversations.send(search));
    // Give the failing cleanup time to run; the marker must survive it.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(scratch.store().superseded(&id), ["session-1"]);
    assert!(
        conversations
            .provider()
            .removed_sessions
            .lock()
            .unwrap()
            .is_empty()
    );

    // Forgetting the conversation removes every session it used, including
    // the one left behind.
    conversations
        .provider()
        .cleanup_fails
        .store(false, Ordering::SeqCst);
    assert_eq!(
        run(conversations.forget(&id)).last(),
        Some(&Update::Completed)
    );
    let mut removed = conversations
        .provider()
        .removed_sessions
        .lock()
        .unwrap()
        .clone();
    removed.sort();
    assert_eq!(removed, ["session-1", "session-2"]);
    assert_eq!(scratch.store().get(&id), None);
    assert!(scratch.store().superseded(&id).is_empty());
    assert_eq!(conversations.store().get(&id), None);
}

#[test]
fn forgetting_keeps_the_mapping_for_a_retry_when_the_provider_cleanup_fails() {
    let scratch = Scratch::new();
    let recorder = Recorder::new(true).script(answer("session-1"));
    let conversations = Conversations::new(recorder, scratch.store());
    let id = first_conversation(&conversations);
    conversations
        .provider()
        .cleanup_fails
        .store(true, Ordering::SeqCst);
    assert_eq!(
        failure(&run(conversations.forget(&id))),
        forget::SESSION_FORGET_FAILED
    );
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-1"));
    assert_eq!(scratch.store().get(&id).as_deref(), Some("session-1"));
}

#[test]
fn forgetting_retries_the_conversations_per_turn_cleanups() {
    let conversations = Conversations::new(Recorder::new(false), SessionStore::new(None));
    let id = "conv_0123456789abcdef";
    assert_eq!(
        run(conversations.forget(id)).last(),
        Some(&Update::Completed)
    );
    assert_eq!(
        *conversations.provider().retried_groups.lock().unwrap(),
        [id]
    );
}

#[test]
fn a_session_the_provider_says_is_gone_is_rebuilt_under_the_same_conversation() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true)
            .script(answer("session-1"))
            .script(vec![
                Update::Launched,
                Update::Session("session-1".to_owned()),
                Update::Started {
                    conversation_id: None,
                },
                Update::SessionLost(SessionLoss::Confirmed),
                Update::Failed(Failure {
                    code: ErrorCode::InvalidRequest,
                    reason: "UNKNOWN_SESSION",
                    retryable: false,
                }),
            ])
            .script(answer("session-2")),
        scratch.store(),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        history: history(),
        ..request("follow up")
    };
    let slot = follow_up.conversation.clone();
    let updates = run(conversations.send(follow_up));
    assert_eq!(updates.last(), Some(&Update::Completed));
    // One Started, and no new conversation.
    assert_eq!(
        updates
            .iter()
            .filter(|update| matches!(update, Update::Started { .. }))
            .count(),
        1
    );
    assert!(!slot.created());
    assert_eq!(slot.id().as_deref(), Some(id.as_str()));
    assert!(!updates.iter().any(|u| matches!(u, Update::SessionLost(_))));

    let turns = Recorder::turns(&conversations);
    assert_eq!(turns[1].continuation.as_deref(), Some("session-1"));
    assert_eq!(turns[2].continuation, None);
    assert_eq!(text_of(&turns[2])[1], "first answer");
    // The request's sign-in was checked once, by the run that failed.
    assert!(turns[1].check_sign_in);
    assert!(!turns[2].check_sign_in);
    // The rebuilt session is the conversation's now, even after a restart.
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-2"));
    assert_eq!(scratch.store().get(&id).as_deref(), Some("session-2"));
}

#[test]
fn a_gone_session_without_history_drops_the_mapping_and_fails_as_unknown() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")).script(vec![
            Update::Launched,
            Update::SessionLost(SessionLoss::Confirmed),
            Update::Failed(Failure {
                code: ErrorCode::ProviderFailed,
                reason: "PROCESS_EXITED",
                retryable: true,
            }),
        ]),
        scratch.store(),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        ..request("again")
    };
    assert_eq!(
        failure(&run(conversations.send(follow_up))),
        UNKNOWN_CONVERSATION
    );
    assert_eq!(conversations.store().get(&id), None);
    assert_eq!(scratch.store().get(&id), None);
}

#[test]
fn a_resume_that_ends_before_the_turn_starts_begins_a_new_conversation() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true)
            .script(answer("session-1"))
            .script(vec![
                Update::Launched,
                Update::SessionLost(SessionLoss::Suspected),
                Update::Failed(Failure {
                    code: ErrorCode::ProviderFailed,
                    reason: "PROCESS_EXITED",
                    retryable: true,
                }),
            ])
            .script(answer("session-2")),
        scratch.store(),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        history: history(),
        ..request("follow up")
    };
    let slot = follow_up.conversation.clone();
    let updates = run(conversations.send(follow_up));
    assert_eq!(updates.last(), Some(&Update::Completed));
    assert!(slot.created());
    let new_id = slot.id().unwrap();
    assert_ne!(new_id, id);
    // Why the resume failed isn't known, so the old mapping stays.
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-1"));
    assert_eq!(
        conversations.store().get(&new_id).as_deref(),
        Some("session-2")
    );
}

#[test]
fn a_resume_that_ends_before_the_turn_starts_reports_its_own_failure_without_history() {
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")).script(vec![
            Update::Launched,
            Update::SessionLost(SessionLoss::Suspected),
            Update::Failed(Failure {
                code: ErrorCode::ProviderFailed,
                reason: "PROCESS_EXITED",
                retryable: true,
            }),
        ]),
        SessionStore::new(None),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        ..request("again")
    };
    assert_eq!(
        failure(&run(conversations.send(follow_up))).reason,
        "PROCESS_EXITED"
    );
    // The mapping is kept: a crash never discards the native session.
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-1"));
}

#[test]
fn a_loss_after_answer_text_is_not_rebuilt() {
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")).script(vec![
            Update::Session("session-1".to_owned()),
            Update::Started {
                conversation_id: None,
            },
            Update::Delta("partial".to_owned()),
            Update::SessionLost(SessionLoss::Confirmed),
            Update::Failed(Failure {
                code: ErrorCode::ProviderFailed,
                reason: "PROCESS_EXITED",
                retryable: true,
            }),
        ]),
        SessionStore::new(None),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        history: history(),
        ..request("again")
    };
    assert_eq!(
        failure(&run(conversations.send(follow_up))).reason,
        "PROCESS_EXITED"
    );
    assert_eq!(Recorder::turns(&conversations).len(), 2);
    assert_eq!(conversations.store().get(&id).as_deref(), Some("session-1"));
}

#[test]
fn a_session_forked_by_the_provider_replaces_the_mapping_and_cleans_up_the_old_one() {
    let scratch = Scratch::new();
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")).script(vec![
            Update::Launched,
            Update::Session("session-1".to_owned()),
            Update::Started {
                conversation_id: None,
            },
            Update::Delta("ok".to_owned()),
            // The final result names a different session.
            Update::Session("session-2".to_owned()),
            Update::Completed,
        ]),
        scratch.store(),
    );
    let id = first_conversation(&conversations);
    let follow_up = SendRequest {
        conversation_id: Some(id.clone()),
        ..request("again")
    };
    assert_eq!(
        run(conversations.send(follow_up)).last(),
        Some(&Update::Completed)
    );
    assert_eq!(scratch.store().get(&id).as_deref(), Some("session-2"));
    let removed = Arc::clone(&conversations.provider().removed_sessions);
    wait_until("the old session's cleanup", || {
        removed.lock().unwrap().as_slice() == ["session-1"]
    });
}

#[test]
fn a_mapping_that_cannot_be_stored_ends_the_turn_before_it_is_announced() {
    let scratch = Scratch::new();
    std::fs::create_dir_all(&scratch.0).unwrap();
    let blocked = scratch.0.join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")),
        SessionStore::new(Some(blocked)),
    );
    let request = request("first question");
    let updates = run(conversations.send(request));
    assert_eq!(failure(&updates), SESSION_STORE_FAILED);
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::Started { .. } | Update::Delta(_))),
        "{updates:?}"
    );
}

#[test]
fn a_conversation_that_cannot_be_kept_across_restarts_is_refused_when_durability_is_required() {
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")),
        SessionStore::new(None).with_durability(Durability::Required),
    );
    assert_eq!(
        failure(&run(conversations.send(request("first question")))),
        SESSION_STORE_FAILED
    );
}

#[test]
fn cancelling_stops_the_run_and_ends_once() {
    let conversations = Conversations::new(
        Recorder::new(true).script(answer("session-1")),
        SessionStore::new(None),
    );
    let mut exchange = conversations.send(request("first question"));
    assert_eq!(
        exchange.next(Instant::now() + Duration::from_secs(1)),
        Some(Update::Launched)
    );
    exchange.cancel(Duration::ZERO);
    exchange.cancel(Duration::ZERO);
    let mut ends = 0;
    while let Some(update) = exchange.next(Instant::now() + Duration::from_secs(1)) {
        if update.is_terminal() {
            ends += 1;
            assert_eq!(update, Update::Stopped);
        }
    }
    assert_eq!(ends, 1);
}
