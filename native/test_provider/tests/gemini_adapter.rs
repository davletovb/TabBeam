//! Pervue's conversations over the Gemini adapter, against a fake `agy`. Gemini
//! keeps no session, so a conversation continues from the dialogue Pervue
//! replays; this tests that mapping, that a failed first turn's conversation
//! can be retried, and what deleting a conversation removes. The adapter's own
//! behaviour is tested at the runtime's level, in `runtime-tests`.

mod support;

use std::time::{Duration, Instant};

use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::conversations::{Conversations, SessionStore};
use pervue_host::providers::{
    ConversationProvider, ConversationSlot, Exchange, SendRequest, Update,
};
use runtime_core::turn::SessionPolicy;

use support::FakeGemini;

fn collect(mut exchange: Box<dyn Exchange>) -> Vec<Update> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut updates = Vec::new();
    while let Some(update) = exchange.next(deadline) {
        let terminal = matches!(
            update,
            Update::Completed | Update::Failed(_) | Update::Stopped
        );
        updates.push(update);
        if terminal {
            break;
        }
    }
    updates
}

fn request(conversation_id: Option<String>, native_search: bool) -> SendRequest {
    SendRequest {
        text: if native_search {
            "Find the example source".to_owned()
        } else {
            "Say hello".to_owned()
        },
        history: if conversation_id.is_some() {
            vec![
                HistoryMessage {
                    role: Role::User,
                    text: "Earlier question".to_owned(),
                },
                HistoryMessage {
                    role: Role::Assistant,
                    text: "Earlier assistant answer".to_owned(),
                },
            ]
        } else {
            Vec::new()
        },
        conversation_id,
        context: None,
        model: Some("gemini-test".to_owned()),
        native_search,
        session_policy: SessionPolicy::Ephemeral,
        fresh_session: false,
        conversation: ConversationSlot::default(),
    }
}

#[test]
fn one_shot_turns_continue_from_bounded_pervue_history() {
    let fake = FakeGemini::install();
    let adapter = fake.adapter();

    let request_one = request(None, false);
    let slot = request_one.conversation.clone();
    let first = collect(adapter.send(request_one));
    assert!(slot.created(), "a new conversation is announced");
    let conversation = slot.id().expect("conversation created");
    assert_eq!(answer_text(&first), "Gemini answer");
    assert!(matches!(first.last(), Some(Update::Completed)));

    let request_two = request(Some(conversation.clone()), false);
    let slot = request_two.conversation.clone();
    let second = collect(adapter.send(request_two));
    assert!(!slot.created(), "a continued conversation isn't announced");
    assert_eq!(slot.id().as_deref(), Some(conversation.as_str()));
    assert!(
        second
            .iter()
            .any(|update| matches!(update, Update::Started))
    );
    assert_eq!(answer_text(&second), "Gemini continued answer");
    assert!(matches!(second.last(), Some(Update::Completed)));
}

#[test]
fn a_failed_first_turn_can_retry_with_empty_completed_history() {
    let fake = FakeGemini::install();
    let adapter = fake.adapter();

    let mut first_request = request(None, false);
    first_request.model = Some("gemini-tool-violation".to_owned());
    let slot = first_request.conversation.clone();
    let first = collect(adapter.send(first_request));
    assert!(
        slot.created(),
        "a failed first turn still created its conversation"
    );
    let conversation = slot.id().expect("the conversation the failed turn created");
    assert!(matches!(
        first.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
    ));

    let mut retry = request(Some(conversation.clone()), false);
    retry.history.clear();
    retry.model = Some("gemini-test".to_owned());
    let slot = retry.conversation.clone();
    let updates = collect(adapter.send(retry));
    assert_eq!(slot.id().as_deref(), Some(conversation.as_str()));
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, Update::Started))
    );
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn invalid_pervue_conversation_ids_are_refused_without_running_agy() {
    let fake = FakeGemini::install();
    let mut request = request(Some("latest".to_owned()), false);
    request.history.clear();
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.as_slice(),
        [Update::Failed(error)] if error.reason == "UNKNOWN_CONVERSATION"
    ));
}

#[test]
fn persisted_cleanup_records_survive_adapter_restart_and_forget_retries_them() {
    let fake = FakeGemini::install();
    let cleanup_dir = fake.dir.join("durable-cleanups");
    let conversation = "conv_0000000000000001";
    let agy_id = "agy-restart-1";

    let transcript = fake.brain().join(agy_id).join(".system_generated/logs");
    std::fs::create_dir_all(&transcript).unwrap();
    std::fs::write(transcript.join("transcript.jsonl"), "left behind").unwrap();

    let record = cleanup_dir.join(conversation);
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(record.join(agy_id), "pending\n").unwrap();

    // A fresh adapter has an empty in-memory map and must recover from disk.
    let adapter = Conversations::new(
        fake.gemini().with_cleanup_dir(cleanup_dir.clone()),
        SessionStore::new(None),
    );
    let updates = collect(adapter.forget(conversation));
    assert!(matches!(updates.as_slice(), [Update::Completed]));
    assert!(!fake.brain().join(agy_id).exists());
    assert!(!record.exists());
}

fn answer_text(updates: &[Update]) -> String {
    updates
        .iter()
        .filter_map(|update| match update {
            Update::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn pervue_runs_its_turns_as_agents_named_pervue() {
    // The agent definitions live in per-turn workspaces, so nothing installed
    // depends on these names, but they are what Pervue has always used.
    let fake = FakeGemini::install();
    collect(fake.adapter().send(request(None, false)));
    collect(fake.adapter().send(request(None, true)));
    let invocations = fake.invocations().concat();
    assert!(invocations.contains("--agent pervue-text"), "{invocations}");
    assert!(
        invocations.contains("--agent pervue-search"),
        "{invocations}"
    );
}
