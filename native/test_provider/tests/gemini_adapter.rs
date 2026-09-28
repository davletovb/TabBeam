mod support;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pervue_core::discovery::SearchPath;
use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::providers::gemini::Gemini;
use pervue_host::providers::{Exchange, Provider, SendRequest, Update};

use support::PROVIDER;

struct FakeGemini {
    dir: PathBuf,
    home: PathBuf,
}

impl FakeGemini {
    fn install() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "pervue-fake-agy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "agy.exe" } else { "agy" };
        let path = dir.join(name);
        if std::fs::hard_link(PROVIDER, &path).is_err() {
            std::fs::copy(PROVIDER, &path).unwrap();
        }
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        Self { dir, home }
    }

    fn adapter(&self) -> Gemini {
        let home_name = if cfg!(unix) { "HOME" } else { "USERPROFILE" };
        Gemini::new(
            SearchPath::new([self.dir.clone()]),
            self.dir.join("workspace"),
        )
        .with_environment([
            (
                OsString::from(home_name),
                self.home.as_os_str().to_os_string(),
            ),
            (OsString::from("PATH"), self.dir.as_os_str().to_os_string()),
        ])
    }

    fn brain(&self) -> PathBuf {
        self.home.join(".gemini/antigravity-cli/brain")
    }
}

impl Drop for FakeGemini {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

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
    }
}

#[test]
fn status_uses_agy_models_to_confirm_authentication() {
    let fake = FakeGemini::install();
    let updates = collect(fake.adapter().status());
    match &updates[0] {
        Update::Status {
            provider_id,
            status,
        } => {
            assert_eq!(provider_id, "gemini");
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
                pervue_host::providers::gemini::CAPABILITIES
            );
        }
        other => panic!("unexpected update: {other:?}"),
    }
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn one_shot_turns_continue_from_bounded_pervue_history() {
    let fake = FakeGemini::install();
    let adapter = fake.adapter();

    let first = collect(adapter.send(request(None, false)));
    let conversation = first
        .iter()
        .find_map(|update| match update {
            Update::ConversationCreated(id) => Some(id.clone()),
            _ => None,
        })
        .expect("conversation created");
    assert!(
        first
            .iter()
            .any(|update| matches!(update, Update::Delta(text) if text == "Gemini answer"))
    );
    assert!(matches!(first.last(), Some(Update::Completed)));

    let second = collect(adapter.send(request(Some(conversation.clone()), false)));
    assert!(
        !second
            .iter()
            .any(|update| matches!(update, Update::ConversationCreated(_)))
    );
    assert!(second.iter().any(|update| matches!(update, Update::Started { conversation_id: Some(id) } if id == &conversation)));
    assert!(
        second.iter().any(
            |update| matches!(update, Update::Delta(text) if text == "Gemini continued answer")
        )
    );
    assert!(matches!(second.last(), Some(Update::Completed)));
}

#[test]
fn native_search_requires_the_actual_search_tool_and_emits_sources() {
    let fake = FakeGemini::install();
    let updates = collect(fake.adapter().send(request(None, true)));
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::Delta(text) if text.contains("I will search")))
    );
    assert!(updates.iter().any(|update| matches!(
        update,
        Update::Source(source)
            if source.backend_id == "gemini"
                && source.url == "https://example.com/agy-search"
    )));
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn every_finished_turn_removes_antigravitys_persisted_transcript() {
    let fake = FakeGemini::install();
    let updates = collect(fake.adapter().send(request(None, false)));
    assert!(matches!(updates.last(), Some(Update::Completed)));
    let remaining = std::fs::read_dir(fake.brain())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        remaining, 0,
        "Pervue-owned agy transcript survived the turn"
    );
}

#[test]
fn unexpected_antigravity_tools_fail_closed() {
    let fake = FakeGemini::install();
    let mut request = request(None, false);
    request.model = Some("gemini-tool-violation".to_owned());
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
    ));
}

#[test]
fn a_follow_up_without_history_is_refused_before_agy_runs() {
    let fake = FakeGemini::install();
    let mut request = request(Some("conv_known".to_owned()), false);
    request.history.clear();
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.as_slice(),
        [Update::Failed(error)] if error.reason == "UNKNOWN_CONVERSATION"
    ));
}

#[test]
fn non_gemini_antigravity_models_are_refused() {
    let fake = FakeGemini::install();
    let mut request = request(None, false);
    request.model = Some("claude-test".to_owned());
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.as_slice(),
        [Update::Failed(error)] if error.reason == "MODEL_NOT_SUPPORTED"
    ));
}
