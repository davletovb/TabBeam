//! Claude adapter tests against the fake Claude Code CLI.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::protocol::events::{Authentication, Availability, Capability, ErrorCode};
use pervue_host::providers::claude::{Claude, Limits};
use pervue_host::providers::discovery::SearchPath;
use pervue_host::providers::{Exchange, Provider, SendRequest, Timeouts, Update};
use serde_json::Value;

const PROVIDER: &str = env!("CARGO_BIN_EXE_pervue-fake-provider");
const DEADLINE: Duration = Duration::from_secs(20);
const TEST_LIMITS: Limits = Limits {
    timeouts: Timeouts {
        start: Duration::from_secs(10),
        idle: Duration::from_secs(10),
        stop_grace: Duration::from_millis(300),
    },
    probe: Duration::from_secs(5),
    finish: Duration::from_millis(300),
};

struct FakeClaude {
    dir: PathBuf,
}

impl FakeClaude {
    fn install(print: &str, auth: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "pervue-fake-claude-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(Self::file_name());
        if std::fs::hard_link(PROVIDER, &path).is_err() {
            std::fs::copy(PROVIDER, &path).unwrap();
        }
        let fake = Self { dir };
        fake.set(print, auth);
        fake
    }

    fn file_name() -> &'static str {
        if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        }
    }

    fn set(&self, print: &str, auth: &str) {
        std::fs::write(
            self.dir.join("claude-scenario"),
            format!("print={print}\nauth={auth}\n"),
        )
        .unwrap();
    }

    fn adapter(&self) -> Claude {
        Claude::new(SearchPath::new([self.dir.clone()]), self.dir.join("work"))
            .with_limits(TEST_LIMITS)
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name)).unwrap_or_default()
    }

    fn prompts(&self) -> Vec<String> {
        self.read("claude-prompts")
            .split('\0')
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn invocations(&self) -> Vec<String> {
        self.read("claude-invocations")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn pids(&self) -> Vec<u32> {
        self.read("claude-pids")
            .lines()
            .map(|pid| pid.parse().expect("a pid"))
            .collect()
    }

    fn assert_nothing_left_running(&self) {
        #[cfg(unix)]
        for pid in self.pids() {
            use nix::errno::Errno;
            use nix::sys::signal::kill;
            use nix::unistd::Pid;

            let pid = Pid::from_raw(i32::try_from(pid).expect("pid fits in pid_t"));
            assert_eq!(kill(pid, None), Err(Errno::ESRCH), "{pid} is still around");
        }
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ask(text: &str) -> SendRequest {
    SendRequest {
        text: text.to_owned(),
        history: Vec::new(),
        conversation_id: None,
        context: None,
    }
}

fn history() -> Vec<HistoryMessage> {
    vec![
        HistoryMessage {
            role: Role::User,
            text: "first question".to_owned(),
        },
        HistoryMessage {
            role: Role::Assistant,
            text: "first answer".to_owned(),
        },
    ]
}

fn run_to_end(exchange: &mut dyn Exchange) -> Vec<Update> {
    let deadline = Instant::now() + DEADLINE;
    let mut updates = Vec::new();
    loop {
        let update = exchange.next(deadline).expect("exchange timed out");
        let terminal = update.is_terminal();
        updates.push(update);
        if terminal {
            return updates;
        }
    }
}

fn run_until_started(exchange: &mut dyn Exchange) -> Vec<Update> {
    let deadline = Instant::now() + DEADLINE;
    let mut updates = Vec::new();
    loop {
        let update = exchange.next(deadline).expect("exchange did not start");
        assert!(!update.is_terminal(), "{update:?}");
        let started = matches!(update, Update::Started { .. });
        updates.push(update);
        if started {
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

fn failure(updates: &[Update]) -> (ErrorCode, &'static str) {
    match updates.last() {
        Some(Update::Failed(error)) => (error.code, error.reason),
        other => panic!("expected failure, got {other:?}"),
    }
}

fn status(claude: &Claude) -> (Availability, Authentication) {
    let updates = run_to_end(claude.status().as_mut());
    match &updates[0] {
        Update::Status {
            provider_id,
            status,
        } => {
            assert_eq!(provider_id, "claude");
            (status.availability, status.authentication)
        }
        other => panic!("expected status, got {other:?}"),
    }
}

#[test]
fn status_uses_shared_discovery_and_auth_exit_status() {
    let claude = FakeClaude::install("answers", "signed-in");
    assert_eq!(
        status(&claude.adapter()),
        (Availability::Available, Authentication::Authenticated)
    );
    claude.set("answers", "signed-out");
    assert_eq!(
        status(&claude.adapter()),
        (Availability::Available, Authentication::Unauthenticated)
    );
    claude.set("answers", "broken");
    assert_eq!(
        status(&claude.adapter()),
        (Availability::Available, Authentication::Unknown)
    );
    assert!(
        claude
            .invocations()
            .iter()
            .all(|line| line == "auth status")
    );
}

#[test]
fn missing_claude_is_not_found() {
    let claude = FakeClaude::install("answers", "signed-in");
    std::fs::remove_file(claude.dir.join(FakeClaude::file_name())).unwrap();
    assert_eq!(
        status(&claude.adapter()),
        (Availability::NotFound, Authentication::Unknown)
    );
    assert_eq!(
        failure(&run_to_end(claude.adapter().send(ask("hi")).as_mut())),
        (ErrorCode::ProviderNotFound, "EXECUTABLE_NOT_FOUND")
    );
}

#[test]
fn request_streams_with_tools_disabled_and_keeps_question_off_argv() {
    let claude = FakeClaude::install("answers", "signed-in");
    let question = "Why? $(id) ; rm -rf ~ é✓😀";
    let updates = visible(&run_to_end(claude.adapter().send(ask(question)).as_mut()));
    let Update::ConversationCreated(conversation) = &updates[0] else {
        panic!("missing conversation: {updates:?}");
    };
    assert!(conversation.starts_with("conv_"));
    assert_eq!(
        updates[1..],
        [
            Update::Started {
                conversation_id: Some(conversation.clone())
            },
            Update::Delta("You asked: ".to_owned()),
            Update::Delta(question.to_owned()),
            Update::Completed,
        ]
    );
    assert_eq!(claude.prompts(), [question]);
    let print = claude
        .invocations()
        .into_iter()
        .find(|line| line.starts_with("-p "))
        .unwrap();
    assert!(print.contains("--output-format stream-json"));
    assert!(print.contains("--input-format stream-json"));
    assert!(print.contains("--include-partial-messages"));
    assert!(print.contains("--permission-mode default"));
    assert!(print.contains("--tools  --disallowedTools mcp__*"));
    assert!(!print.contains("--permission-mode plan"));
    assert!(!print.contains(question));
}

#[test]
fn claude_inherits_node_extra_ca_certs_but_not_arbitrary_secrets() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter().with_environment([
        (
            OsString::from("NODE_EXTRA_CA_CERTS"),
            OsString::from("/tmp/company-ca.pem"),
        ),
        (
            OsString::from("HTTPS_PROXY"),
            OsString::from("http://proxy.example"),
        ),
        (
            OsString::from("SECRET_TOKEN"),
            OsString::from("do-not-pass"),
        ),
    ]);
    run_to_end(adapter.send(ask("hello")).as_mut());
    let environment = claude.read("claude-environment");
    let line = environment.lines().last().expect("provider environment");
    let value: Value = serde_json::from_str(line).unwrap();
    assert_eq!(value["env"]["NODE_EXTRA_CA_CERTS"], "/tmp/company-ca.pem");
    assert_eq!(value["env"]["HTTPS_PROXY"], "http://proxy.example");
    assert!(value["env"].get("SECRET_TOKEN").is_none());
}

#[test]
fn continuation_resumes_and_survives_adapter_restart() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation) = first[0].clone() else {
        panic!("missing conversation");
    };
    drop(adapter);

    let restarted = claude.adapter();
    let second = visible(&run_to_end(
        restarted
            .send(SendRequest {
                conversation_id: Some(conversation.clone()),
                ..ask("second")
            })
            .as_mut(),
    ));
    assert_eq!(
        second,
        [
            Update::Started {
                conversation_id: Some(conversation)
            },
            Update::Delta("You asked: ".to_owned()),
            Update::Delta("second".to_owned()),
            Update::Completed,
        ]
    );
    assert!(
        claude
            .invocations()
            .iter()
            .any(|line| line.contains("--resume claude-"))
    );
}

#[test]
fn stale_resume_rebuilds_once_from_bounded_history() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation) = first[0].clone() else {
        panic!("missing conversation");
    };

    claude.set("resume-fails", "signed-in");
    let updates = visible(&run_to_end(
        adapter
            .send(SendRequest {
                conversation_id: Some(conversation),
                history: history(),
                ..ask("follow up")
            })
            .as_mut(),
    ));
    assert!(matches!(updates[0], Update::ConversationCreated(_)));
    assert_eq!(updates.last(), Some(&Update::Completed));
    let prompts = claude.prompts();
    let prompt = prompts.last().expect("fallback prompt");
    assert!(prompt.contains("first question"));
    assert!(prompt.contains("first answer"));
    assert!(prompt.ends_with("follow up"));

    let prints: Vec<_> = claude
        .invocations()
        .into_iter()
        .filter(|line| line.starts_with("-p "))
        .collect();
    assert!(prints.iter().any(|line| line.contains("--resume claude-")));
    assert!(prints.last().is_some_and(|line| !line.contains("--resume")));
}

#[test]
fn result_session_id_replaces_the_mapping_for_the_next_turn() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation) = first[0].clone() else {
        panic!("missing conversation");
    };

    claude.set("forks-session", "signed-in");
    run_to_end(
        adapter
            .send(SendRequest {
                conversation_id: Some(conversation.clone()),
                ..ask("second")
            })
            .as_mut(),
    );

    claude.set("answers", "signed-in");
    run_to_end(
        adapter
            .send(SendRequest {
                conversation_id: Some(conversation),
                ..ask("third")
            })
            .as_mut(),
    );

    let prints: Vec<_> = claude
        .invocations()
        .into_iter()
        .filter(|line| line.starts_with("-p "))
        .collect();
    assert!(prints[1].contains("--resume claude-"));
    assert!(prints[2].contains("--resume forked-"), "{:?}", prints[2]);
}

#[test]
fn missing_native_session_rebuilds_from_bounded_history() {
    let claude = FakeClaude::install("no-partial", "signed-in");
    let updates = visible(&run_to_end(
        claude
            .adapter()
            .send(SendRequest {
                conversation_id: Some("conv_missing".to_owned()),
                history: history(),
                ..ask("follow up")
            })
            .as_mut(),
    ));
    assert!(matches!(updates[0], Update::ConversationCreated(_)));
    let prompts = claude.prompts();
    let prompt = prompts.last().expect("history prompt");
    assert!(prompt.contains("first question"));
    assert!(prompt.contains("first answer"));
    assert!(prompt.ends_with("follow up"));
}

#[test]
fn separate_assistant_messages_receive_a_blank_line() {
    let claude = FakeClaude::install("two-messages", "signed-in");
    let updates = visible(&run_to_end(claude.adapter().send(ask("hello")).as_mut()));
    assert!(updates.contains(&Update::Delta("First.".to_owned())));
    assert!(updates.contains(&Update::Delta("\n\nSecond.".to_owned())));
}

#[test]
fn result_text_is_fallback_when_partial_deltas_are_absent() {
    let claude = FakeClaude::install("no-partial", "signed-in");
    let updates = visible(&run_to_end(claude.adapter().send(ask("hello")).as_mut()));
    assert!(updates.contains(&Update::Delta("You asked: hello".to_owned())));
}

#[test]
fn completed_result_does_not_wait_for_a_lingering_process() {
    let claude = FakeClaude::install("lingers", "signed-in");
    let started = Instant::now();
    let updates = visible(&run_to_end(claude.adapter().send(ask("hello")).as_mut()));
    assert_eq!(updates.last(), Some(&Update::Completed));
    assert!(started.elapsed() < Duration::from_secs(5));
    claude.assert_nothing_left_running();
}

#[test]
fn ignored_event_flood_yields_and_can_be_cancelled() {
    let claude = FakeClaude::install("flooding", "signed-in");
    let adapter = claude.adapter();
    let mut exchange = adapter.send(ask("flood"));
    run_until_started(exchange.as_mut());

    let started = Instant::now();
    assert_eq!(
        exchange.next(Instant::now() + Duration::from_millis(20)),
        None
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    exchange.cancel(Duration::from_millis(300));
    assert_eq!(run_to_end(exchange.as_mut()).last(), Some(&Update::Stopped));
    claude.assert_nothing_left_running();
}

#[test]
fn cancellation_and_provider_errors_are_normalized() {
    let claude = FakeClaude::install("hangs", "signed-in");
    let adapter = claude.adapter();
    let mut exchange = adapter.send(ask("wait"));
    run_until_started(exchange.as_mut());
    exchange.cancel(Duration::from_millis(300));
    assert_eq!(run_to_end(exchange.as_mut()).last(), Some(&Update::Stopped));

    claude.set("fails-auth", "signed-in");
    assert_eq!(
        failure(&run_to_end(claude.adapter().send(ask("hi")).as_mut())),
        (ErrorCode::ProviderNotAuthenticated, "AUTH_REJECTED")
    );
    claude.set("fails-rate", "signed-in");
    assert_eq!(
        failure(&run_to_end(claude.adapter().send(ask("hi")).as_mut())),
        (ErrorCode::ProviderFailed, "PROVIDER_RATE_LIMITED")
    );
}

#[test]
fn capabilities_express_claudes_observed_differences() {
    let capabilities = pervue_host::providers::claude::CAPABILITIES;
    assert_eq!(capabilities.streaming, Capability::Supported);
    assert_eq!(capabilities.continuation, Capability::Supported);
    assert_eq!(capabilities.page_context, Capability::Unsupported);
    assert_eq!(capabilities.model_selection, Capability::Unsupported);
    assert_eq!(capabilities.cancellation, Capability::Supported);
}
