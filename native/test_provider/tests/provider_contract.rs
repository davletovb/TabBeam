//! Provider-neutral contract cases run unchanged against Codex and Claude (TST-10).

mod support;

use std::time::{Duration, Instant};

use pervue_host::protocol::events::{Authentication, Availability, Capability};
use pervue_host::providers::{Exchange, Provider, SendRequest, Update};
use support::{FakeClaude, FakeCodex};

const DEADLINE: Duration = Duration::from_secs(20);

fn ask(text: &str) -> SendRequest {
    SendRequest {
        text: text.to_owned(),
        history: Vec::new(),
        conversation_id: None,
        context: None,
    }
}

fn run_to_end(exchange: &mut dyn Exchange) -> Vec<Update> {
    let deadline = Instant::now() + DEADLINE;
    let mut updates = Vec::new();
    loop {
        let update = exchange
            .next(deadline)
            .expect("provider contract timed out");
        let terminal = update.is_terminal();
        updates.push(update);
        if terminal {
            return updates;
        }
    }
}

fn visible(updates: &[Update]) -> Vec<Update> {
    updates
        .iter()
        .filter(|update| **update != Update::Activity)
        .cloned()
        .collect()
}

fn common_contract(provider: &dyn Provider) {
    let status = run_to_end(provider.status().as_mut());
    let Update::Status {
        provider_id,
        status: state,
    } = &status[0]
    else {
        panic!("missing provider status: {status:?}");
    };
    assert_eq!(provider_id, provider.id());
    assert_eq!(state.availability, Availability::Available);
    assert_eq!(state.authentication, Authentication::Authenticated);
    assert_eq!(state.capabilities.streaming, Capability::Supported);
    assert_eq!(state.capabilities.continuation, Capability::Supported);
    assert_eq!(state.capabilities.cancellation, Capability::Supported);

    let first = visible(&run_to_end(provider.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation_id) = first[0].clone() else {
        panic!("missing conversation: {first:?}");
    };
    assert_eq!(
        first.last(),
        Some(&Update::Completed),
        "{} did not complete",
        provider.id()
    );
    assert!(
        first
            .iter()
            .any(|update| matches!(update, Update::Delta(text) if !text.is_empty()))
    );

    let second = visible(&run_to_end(
        provider
            .send(SendRequest {
                conversation_id: Some(conversation_id.clone()),
                ..ask("second")
            })
            .as_mut(),
    ));
    assert_eq!(
        second.first(),
        Some(&Update::Started {
            conversation_id: Some(conversation_id),
        })
    );
    assert_eq!(second.last(), Some(&Update::Completed));
}

#[test]
fn codex_and_claude_run_the_same_provider_neutral_contract() {
    let codex = FakeCodex::install("answers", "signed-in");
    let claude = FakeClaude::install("answers", "signed-in");
    let codex_adapter = codex.adapter();
    let claude_adapter = claude.adapter();

    for provider in [
        &codex_adapter as &dyn Provider,
        &claude_adapter as &dyn Provider,
    ] {
        common_contract(provider);
    }
}

#[test]
fn provider_differences_are_expressed_only_as_capabilities() {
    let codex = FakeCodex::install("answers", "signed-in").adapter();
    let claude = FakeClaude::install("answers", "signed-in").adapter();

    assert_eq!(codex.capabilities().page_context, Capability::Supported);
    assert_eq!(claude.capabilities().page_context, Capability::Unsupported);
    assert_eq!(
        codex.capabilities().model_selection,
        Capability::Unsupported
    );
    assert_eq!(
        claude.capabilities().model_selection,
        Capability::Unsupported
    );
}
