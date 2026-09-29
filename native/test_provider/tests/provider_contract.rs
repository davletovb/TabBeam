//! Provider-neutral contract cases run unchanged against all four providers as
//! Pervue serves them (TST-10): a conversation the extension names, over an
//! adapter. The adapters themselves meet the runtime's own contract in
//! `runtime-tests`; this is what Pervue's conversation layer adds to it.

mod support;

use runtime_core::turn::SessionPolicy;
use std::time::{Duration, Instant};

use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::protocol::events::{Authentication, Availability, Capability};
use pervue_host::providers::{ConversationProvider, Exchange, SendRequest, Update};
use support::{FakeClaude, FakeCodex, FakeGemini, FakeGrok};

const DEADLINE: Duration = Duration::from_secs(20);

/// One question, in a persistent turn where the provider keeps sessions and an
/// ephemeral one where it does not; and for Gemini and Grok, a model their
/// fakes serve.
fn ask(provider: &dyn ConversationProvider, text: &str) -> SendRequest {
    SendRequest {
        text: text.to_owned(),
        history: Vec::new(),
        conversation_id: None,
        context: None,
        model: match provider.id() {
            "gemini" => Some("gemini-test".to_owned()),
            "grok" => Some("grok-4.6".to_owned()),
            _ => None,
        },
        native_search: false,
        session_policy: if provider.supports_persistent_session() {
            SessionPolicy::Persistent
        } else {
            SessionPolicy::Ephemeral
        },
        fresh_session: false,
        conversation: pervue_host::providers::ConversationSlot::default(),
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
        .filter(|update| {
            !matches!(
                update,
                Update::Activity | Update::Launched | Update::Session(_) | Update::Usage(_)
            )
        })
        .cloned()
        .collect()
}

fn common_contract(provider: &dyn ConversationProvider) {
    let status = run_to_end(provider.status().as_mut());
    let Update::Status {
        provider_id,
        status: state,
    } = &status[0]
    else {
        panic!("missing provider status: {status:?}");
    };
    // Every message names the provider and what it reported, so a failure
    // (a probe that ended unexpectedly, say) says which one, and how.
    let id = provider.id();
    assert_eq!(provider_id, id);
    assert_eq!(
        state.availability,
        Availability::Available,
        "{id}: {status:?}"
    );
    assert_eq!(
        state.authentication,
        Authentication::Authenticated,
        "{id}: {status:?}"
    );
    assert_eq!(state.capabilities, provider.capabilities(), "{id}");
    assert_eq!(
        state.capabilities.continuation,
        Capability::Supported,
        "{id}"
    );
    assert_eq!(
        state.capabilities.cancellation,
        Capability::Supported,
        "{id}"
    );

    let first_request = ask(provider, "first");
    let slot = first_request.conversation.clone();
    let first_raw = run_to_end(provider.send(first_request).as_mut());
    let launched = first_raw
        .iter()
        .position(|update| matches!(update, Update::Launched))
        .expect("missing Launched");
    let started = first_raw
        .iter()
        .position(|update| matches!(update, Update::Started))
        .expect("missing Started");
    assert!(launched < started);
    // Whether a provider keeps a native session is the one thing that
    // differs, and it shows only in that the session is named before the turn
    // starts.
    let session = first_raw
        .iter()
        .position(|update| matches!(update, Update::Session(_)));
    match (provider.supports_persistent_session(), session) {
        (true, Some(session)) => assert!(session < started),
        (false, None) => {}
        (persistent, session) => panic!(
            "{}: persistent sessions {persistent}, but the session shows as {session:?}",
            provider.id()
        ),
    }
    let first = visible(&first_raw);
    assert!(slot.created(), "the first turn creates a conversation");
    let conversation_id = slot.id().expect("a conversation ID");
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

    // The follow-up continues that conversation, whichever way the provider
    // does it: natively, or from the dialogue Pervue replays.
    let second_request = SendRequest {
        conversation_id: Some(conversation_id.clone()),
        history: vec![
            HistoryMessage {
                role: Role::User,
                text: "first".to_owned(),
            },
            HistoryMessage {
                role: Role::Assistant,
                text: "Earlier assistant answer".to_owned(),
            },
        ],
        ..ask(provider, "second")
    };
    let slot = second_request.conversation.clone();
    let second = visible(&run_to_end(provider.send(second_request).as_mut()));
    assert!(!slot.created(), "a follow-up continues its conversation");
    assert_eq!(slot.id(), Some(conversation_id));
    assert_eq!(second.first(), Some(&Update::Started));
    assert_eq!(second.last(), Some(&Update::Completed));

    // A conversation ID that Pervue never made, and that could be taken for
    // an option or a path, is refused without running anything.
    let unknown = SendRequest {
        conversation_id: Some("--latest".to_owned()),
        history: Vec::new(),
        ..ask(provider, "third")
    };
    let refused = run_to_end(provider.send(unknown).as_mut());
    assert!(
        matches!(
            refused.last(),
            Some(Update::Failed(error)) if error.reason == "UNKNOWN_CONVERSATION"
        ),
        "{}: {refused:?}",
        provider.id()
    );
}

#[test]
fn every_provider_runs_the_same_provider_neutral_contract() {
    let codex = FakeCodex::install("answers", "signed-in");
    let claude = FakeClaude::install("answers", "signed-in");
    let gemini = FakeGemini::install();
    let grok = FakeGrok::install();
    let codex_adapter = codex.adapter();
    let claude_adapter = claude.adapter();
    let gemini_adapter = gemini.adapter();
    let grok_adapter = grok.adapter();

    for provider in [
        &codex_adapter as &dyn ConversationProvider,
        &claude_adapter as &dyn ConversationProvider,
        &gemini_adapter as &dyn ConversationProvider,
        &grok_adapter as &dyn ConversationProvider,
    ] {
        common_contract(provider);
    }
}

#[test]
fn provider_differences_are_expressed_only_as_capabilities() {
    let codex = FakeCodex::install("answers", "signed-in").adapter();
    let claude = FakeClaude::install("answers", "signed-in").adapter();
    let gemini = FakeGemini::install().adapter();
    let grok = FakeGrok::install().adapter();

    // Every provider isolates its tools, takes a model, continues a
    // conversation, and can be cancelled...
    for provider in [
        &codex as &dyn ConversationProvider,
        &claude as &dyn ConversationProvider,
        &gemini as &dyn ConversationProvider,
        &grok as &dyn ConversationProvider,
    ] {
        let capabilities = provider.capabilities();
        assert_eq!(capabilities.tool_isolation, Capability::Supported);
        assert_eq!(capabilities.model_selection, Capability::Supported);
        assert_eq!(capabilities.continuation, Capability::Supported);
        assert_eq!(capabilities.cancellation, Capability::Supported);
    }
    // ...and what they differ in, the capabilities say.
    assert_eq!(codex.capabilities().web_search, Capability::Supported);
    assert_eq!(claude.capabilities().web_search, Capability::Supported);
    assert_eq!(gemini.capabilities().web_search, Capability::Supported);
    assert_eq!(grok.capabilities().web_search, Capability::Unsupported);
    assert_eq!(grok.capabilities().streaming, Capability::Unsupported);
    assert_eq!(gemini.capabilities().streaming, Capability::Supported);
}
