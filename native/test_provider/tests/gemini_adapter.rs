mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pervue_core::discovery::SearchPath;
use pervue_host::providers::gemini::Gemini;
use pervue_host::providers::{Exchange, Provider, SendRequest, Update};

use support::PROVIDER;

struct FakeGemini {
    dir: PathBuf,
}

impl FakeGemini {
    fn install() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "pervue-fake-gemini-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "gemini.exe" } else { "gemini" };
        let path = dir.join(name);
        if std::fs::hard_link(PROVIDER, &path).is_err() {
            std::fs::copy(PROVIDER, &path).unwrap();
        }
        Self { dir }
    }

    fn adapter(&self) -> Gemini {
        Gemini::new(
            SearchPath::new([self.dir.clone()]),
            self.dir.join("workspace"),
        )
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
        let terminal = matches!(update, Update::Completed | Update::Failed(_) | Update::Stopped);
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
        history: Vec::new(),
        conversation_id,
        context: None,
        model: Some("gemini-test".to_owned()),
        native_search,
    }
}

#[test]
fn status_reports_installed_capabilities_without_claiming_auth() {
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
                pervue_host::protocol::events::Authentication::Unknown
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
fn streams_and_resumes_the_same_conversation() {
    let fake = FakeGemini::install();
    let adapter = fake.adapter();

    let first = collect(adapter.send(request(None, false)));
    let conversation = first.iter().find_map(|update| match update {
        Update::ConversationCreated(id) => Some(id.clone()),
        _ => None,
    }).expect("conversation created");
    assert!(first.iter().any(|update| matches!(update, Update::Started { conversation_id: Some(id) } if id == &conversation)));
    assert!(first.iter().any(|update| matches!(update, Update::Delta(text) if text == "Gemini answer")));
    assert!(matches!(first.last(), Some(Update::Completed)));

    let second = collect(adapter.send(request(Some(conversation.clone()), false)));
    assert!(!second.iter().any(|update| matches!(update, Update::ConversationCreated(_))));
    assert!(second.iter().any(|update| matches!(update, Update::Started { conversation_id: Some(id) } if id == &conversation)));
    assert!(matches!(second.last(), Some(Update::Completed)));
}

#[test]
fn native_search_emits_normalized_sources() {
    let fake = FakeGemini::install();
    let updates = collect(fake.adapter().send(request(None, true)));
    assert!(updates.iter().any(|update| matches!(
        update,
        Update::Source(source)
            if source.backend_id == "gemini"
                && source.url == "https://example.com/gemini-search"
    )));
    assert!(matches!(updates.last(), Some(Update::Completed)));
}
