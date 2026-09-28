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
fn a_failed_first_turn_can_retry_with_empty_completed_history() {
    let fake = FakeGemini::install();
    let adapter = fake.adapter();

    let mut first_request = request(None, false);
    first_request.model = Some("gemini-tool-violation".to_owned());
    let first = collect(adapter.send(first_request));
    let conversation = first
        .iter()
        .find_map(|update| match update {
            Update::ConversationCreated(id) => Some(id.clone()),
            _ => None,
        })
        .expect("failed first turn still announced its Pervue conversation");
    assert!(matches!(
        first.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
    ));

    let mut retry = request(Some(conversation.clone()), false);
    retry.history.clear();
    retry.model = Some("gemini-test".to_owned());
    let updates = collect(adapter.send(retry));
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, Update::Started { conversation_id: Some(id) } if id == &conversation))
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
fn unsafe_init_still_cleans_the_transcript_it_already_created() {
    let fake = FakeGemini::install();
    let mut request = request(None, false);
    request.model = Some("gemini-bad-init".to_owned());
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
    ));
    let remaining = std::fs::read_dir(fake.brain())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(remaining, 0, "unsafe init leaked its Antigravity transcript");
}

#[test]
fn cancellation_before_init_scans_the_unique_workspace_and_cleans_transcript() {
    let fake = FakeGemini::install();
    let mut request = request(None, false);
    request.model = Some("gemini-slow-init".to_owned());
    let mut exchange = fake.adapter().send(request);

    let give_up = Instant::now() + Duration::from_secs(2);
    while Instant::now() < give_up {
        if std::fs::read_dir(fake.brain())
            .map(|entries| entries.count() > 0)
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        std::fs::read_dir(fake.brain())
            .map(|entries| entries.count() > 0)
            .unwrap_or(false),
        "fake never created its pre-init transcript"
    );

    exchange.cancel(Duration::from_millis(10));
    let updates = collect(exchange);
    assert!(matches!(updates.last(), Some(Update::Stopped)));
    let remaining = std::fs::read_dir(fake.brain())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(remaining, 0, "pre-init cancellation leaked its transcript");
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
    let adapter = fake.adapter().with_cleanup_dir(cleanup_dir.clone());
    let updates = collect(adapter.forget(conversation));
    assert!(matches!(updates.as_slice(), [Update::Completed]));
    assert!(!fake.brain().join(agy_id).exists());
    assert!(!record.exists());
}

#[test]
fn narration_before_each_search_is_not_saved_into_the_answer() {
    let fake = FakeGemini::install();
    let mut request = request(None, true);
    request.model = Some("gemini-multi-search".to_owned());
    let updates = collect(fake.adapter().send(request));
    let answer: String = updates
        .iter()
        .filter_map(|update| match update {
            Update::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        answer,
        "Gemini search answer [Example](https://example.com/agy-search)."
    );
    assert!(!answer.contains("Let me check"));
    assert!(matches!(updates.last(), Some(Update::Completed)));
}

#[test]
fn unknown_antigravity_step_types_fail_closed() {
    let fake = FakeGemini::install();
    let mut request = request(None, false);
    request.model = Some("gemini-unknown-step".to_owned());
    let updates = collect(fake.adapter().send(request));
    assert!(matches!(
        updates.last(),
        Some(Update::Failed(error)) if error.reason == "PROVIDER_BOUNDARY_VIOLATION"
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
