mod support;

use std::time::{Duration, Instant};

use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::providers::{
    ConversationProvider, ConversationSlot, Exchange, SendRequest, Update,
};
use runtime_core::turn::SessionPolicy;

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
fn status_uses_grok_models_and_cached_oauth() {
    let fake = FakeGrok::install();
    let updates = collect(fake.adapter().status());
    match &updates[0] {
        Update::Status {
            provider_id,
            status,
        } => {
            assert_eq!(provider_id, "grok");
            assert_eq!(
                status.availability,
                pervue_host::protocol::events::Availability::Available
            );
            assert_eq!(
                status.authentication,
                pervue_host::protocol::events::Authentication::Authenticated
            );
            assert_eq!(
                status.capabilities,
                pervue_host::providers::grok::CAPABILITIES
            );
        }
        other => panic!("unexpected update: {other:?}"),
    }
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn signed_out_status_is_reported_as_available_but_unauthenticated() {
    let fake = FakeGrok::install();
    std::fs::remove_file(fake.home.join(".grok/auth.json")).unwrap();
    let updates = collect(fake.adapter().status());
    assert!(matches!(
        &updates[0],
        Update::Status { status, .. }
            if status.availability == pervue_host::protocol::events::Availability::Available
                && status.authentication == pervue_host::protocol::events::Authentication::Unauthenticated
    ));
}

#[test]
fn relocated_grok_home_is_used_only_to_find_the_cached_auth_file() {
    let fake = FakeGrok::install();
    std::fs::remove_file(fake.home.join(".grok/auth.json")).unwrap();
    let custom = fake.home.join("custom-grok");
    std::fs::create_dir_all(&custom).unwrap();
    std::fs::write(custom.join("auth.json"), "{}").unwrap();

    let updates = collect(
        fake.adapter_with_environment(&[("GROK_HOME", custom)])
            .status(),
    );
    assert!(matches!(
        &updates[0],
        Update::Status { status, .. }
            if status.authentication == pervue_host::protocol::events::Authentication::Authenticated
    ));
}

#[test]
fn one_shot_turns_continue_from_bounded_pervue_history() {
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
fn resolved_model_alias_is_accepted() {
    let fake = FakeGrok::install();
    let updates = collect(fake.adapter().send(request(None, false, "grok-4")));
    assert_eq!(answer_text(&updates), "Grok answer");
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn web_search_is_not_advertised_or_launched_on_shipped_grok() {
    let fake = FakeGrok::install();
    assert_eq!(
        pervue_host::providers::grok::CAPABILITIES.web_search,
        pervue_host::protocol::events::Capability::Unsupported
    );
    let updates = collect(fake.adapter().send(request(None, true, "grok-4.6")));
    assert!(matches!(
        updates.as_slice(),
        [Update::Failed(error)] if error.reason == "SEARCH_UNSUPPORTED"
    ));
    assert!(fake.turn_dirs().is_empty());
}

#[test]
fn every_init_boundary_fails_closed_with_a_specific_reason() {
    let fake = FakeGrok::install();
    let adapter = fake.adapter();
    for (model, reason) in [
        ("grok-init-auth", "AUTH_REJECTED"),
        ("grok-init-model", "MODEL_MISMATCH"),
        ("grok-init-cwd", "WORKSPACE_MISMATCH"),
        ("grok-init-tools", "TOOLSET_MISMATCH"),
        ("grok-init-skills", "SKILLS_MISMATCH"),
        ("grok-init-mcp", "MCP_MISMATCH"),
    ] {
        let updates = collect(adapter.send(request(None, false, model)));
        assert!(
            matches!(updates.last(), Some(Update::Failed(error)) if error.reason == reason),
            "{model}: {updates:?}"
        );
    }
}

#[test]
fn unexpected_client_tool_activity_fails_closed() {
    let fake = FakeGrok::install();
    let updates = collect(
        fake.adapter()
            .send(request(None, false, "grok-tool-violation")),
    );
    assert!(matches!(
        updates.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
    ));
}

#[test]
fn failed_results_are_normalized() {
    let fake = FakeGrok::install();
    let updates = collect(
        fake.adapter()
            .send(request(None, false, "grok-result-auth")),
    );
    assert!(matches!(
        updates.last(),
        Some(Update::Failed(error)) if error.reason == "AUTH_REJECTED"
    ));
}

#[test]
fn a_second_host_does_not_remove_a_live_turn_workspace() {
    let fake = FakeGrok::install();
    let first_adapter = fake.adapter();
    let mut running = first_adapter.send(request(None, false, "grok-hang"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(update) = running.next(deadline) {
        if matches!(update, Update::Started) {
            break;
        }
    }
    let live = fake.turn_dirs();
    assert_eq!(live.len(), 1, "expected one live turn workspace");

    let _second_host = fake.adapter();
    for path in live {
        assert!(path.exists(), "a second host removed {path:?}");
    }

    running.cancel(Duration::from_millis(100));
    let updates = collect(running);
    assert!(matches!(updates.last(), Some(Update::Stopped)));
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
