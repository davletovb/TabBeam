//! TabBeam's conversations over the Grok adapter, against a fake `grok`. Grok
//! keeps no session, so a conversation continues from the dialogue TabBeam
//! replays; this tests that mapping, and that a conversation the host never
//! made is refused before Grok runs. The adapter's own behaviour is tested at
//! the runtime's level, in `seatline-tests`.

mod support;

use std::time::{Duration, Instant};

use seatline_core::turn::SessionPolicy;
use tabbeam_host::conversation::{HistoryMessage, Role};
use tabbeam_host::providers::{
    ConversationProvider, ConversationSlot, Exchange, SendRequest, Update,
};

use support::FakeGrok;

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

fn request(conversation_id: Option<String>, native_search: bool, model: &str) -> SendRequest {
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
        model: Some(model.to_owned()),
        native_search,
        session_policy: SessionPolicy::Ephemeral,
        fresh_session: false,
        conversation: ConversationSlot::default(),
    }
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
fn one_shot_turns_continue_from_bounded_tabbeam_history() {
    let fake = FakeGrok::install();
    let adapter = fake.adapter();

    let first_request = request(None, false, "grok-4.6");
    let slot = first_request.conversation.clone();
    let first = collect(adapter.send(first_request));
    assert!(slot.created(), "a new conversation is announced");
    let conversation = slot.id().expect("conversation created");
    assert_eq!(answer_text(&first), "Grok answer");
    assert!(
        first
            .iter()
            .any(|update| matches!(update, Update::Activity))
    );
    assert!(matches!(first.last(), Some(Update::Completed)));

    let second_request = request(Some(conversation.clone()), false, "grok-4.6");
    let slot = second_request.conversation.clone();
    let second = collect(adapter.send(second_request));
    assert!(!slot.created(), "a continued conversation isn't announced");
    assert_eq!(slot.id().as_deref(), Some(conversation.as_str()));
    assert!(
        second
            .iter()
            .any(|update| matches!(update, Update::Started))
    );
    assert_eq!(answer_text(&second), "Grok follow-up answer");
    assert!(matches!(second.last(), Some(Update::Completed)));
}

#[test]
fn invalid_model_and_conversation_ids_are_refused_before_launch() {
    let fake = FakeGrok::install();
    let model = collect(fake.adapter().send(request(None, false, "claude-opus")));
    assert!(matches!(
        model.as_slice(),
        [Update::Failed(error)] if error.reason == "MODEL_NOT_SUPPORTED"
    ));

    let conversation = collect(fake.adapter().send(request(
        Some("--latest".to_owned()),
        false,
        "grok-4.6",
    )));
    assert!(matches!(
        conversation.as_slice(),
        [Update::Failed(error)] if error.reason == "UNKNOWN_CONVERSATION"
    ));
}

#[test]
fn tabbeam_marks_its_grok_workspaces_with_the_owner_file_installed_hosts_know() {
    // A host that was killed leaves its workspaces behind, and the next one
    // finds them by this name, so it must not change.
    let fake = FakeGrok::install();
    let adapter = fake.adapter();
    let mut running = adapter.send(request(None, false, "grok-hang"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(update) = running.next(deadline) {
        if matches!(update, Update::Started) {
            break;
        }
    }
    let live = fake.turn_dirs();
    assert_eq!(live.len(), 1);
    assert!(live[0].join(".tabbeam-owner").exists());
    running.cancel(Duration::from_millis(100));
    collect(running);
    assert!(fake.read("grok-agents").contains("name: tabbeam-text\n"));
}
