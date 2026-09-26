//! Claude adapter tests against the fake Claude Code CLI.

mod support;

use std::ffi::OsString;
use std::time::{Duration, Instant};

use pervue_host::conversation::{HistoryMessage, Role};
use pervue_host::protocol::events::{Authentication, Availability, Capability, ErrorCode};
use pervue_host::providers::claude::Claude;
use pervue_host::providers::{Exchange, Provider, SendRequest, Update};
use serde_json::Value;
use support::FakeClaude;

const DEADLINE: Duration = Duration::from_secs(20);

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
    assert!(print.contains("--tools  --strict-mcp-config --disallowedTools mcp__*"));
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

fn prints(claude: &FakeClaude) -> Vec<String> {
    claude
        .read("claude-invocations")
        .lines()
        .filter(|line| line.starts_with("-p "))
        .map(str::to_owned)
        .collect()
}

fn first_conversation(adapter: &Claude) -> String {
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation) = first[0].clone() else {
        panic!("missing conversation: {first:?}");
    };
    conversation
}

fn follow_up(conversation: &str, history: Vec<HistoryMessage>) -> SendRequest {
    SendRequest {
        conversation_id: Some(conversation.to_owned()),
        history,
        ..ask("follow up")
    }
}

#[test]
fn stale_resume_rebuilds_once_from_bounded_history_under_the_same_conversation() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let conversation = first_conversation(&adapter);

    claude.set("resume-fails", "signed-in");
    let updates = visible(&run_to_end(
        adapter.send(follow_up(&conversation, history())).as_mut(),
    ));
    // The stale session is replaced behind the same conversation ID: no new
    // conversation, and one Started.
    assert_eq!(
        updates[0],
        Update::Started {
            conversation_id: Some(conversation.clone())
        }
    );
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::ConversationCreated(_)))
    );
    assert_eq!(updates.last(), Some(&Update::Completed));
    let prompts = claude.prompts();
    let prompt = prompts.last().expect("fallback prompt");
    assert!(prompt.contains("first question"));
    assert!(prompt.contains("first answer"));
    assert!(prompt.ends_with("follow up"));

    let runs = prints(&claude);
    assert!(runs[1].contains("--resume claude-"));
    assert!(!runs[2].contains("--resume"));

    // The next turn resumes the rebuilt session, even after a restart.
    claude.set("answers", "signed-in");
    drop(adapter);
    let updates = visible(&run_to_end(
        claude
            .adapter()
            .send(follow_up(&conversation, Vec::new()))
            .as_mut(),
    ));
    assert_eq!(updates.last(), Some(&Update::Completed));
    let resumed = prints(&claude);
    let stale = resumed_session(&resumed[1]).expect("the stale session");
    let rebuilt = resumed_session(&resumed[3]).expect("the rebuilt session");
    assert_ne!(stale, rebuilt);
}

fn resumed_session(print: &str) -> Option<&str> {
    let mut args = print.split(' ');
    args.find(|arg| *arg == "--resume")?;
    args.next()
}

#[test]
fn resume_crash_before_init_keeps_the_native_session() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let conversation = first_conversation(&adapter);

    claude.set("resume-crashes", "signed-in");
    let updates = run_to_end(adapter.send(follow_up(&conversation, history())).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::ProviderFailed, "PROCESS_EXITED")
    );
    // No rebuild from history: one print run, and the mapping survives.
    assert_eq!(prints(&claude).len(), 2);
    claude.set("answers", "signed-in");
    let updates = visible(&run_to_end(
        adapter.send(follow_up(&conversation, history())).as_mut(),
    ));
    assert_eq!(updates.last(), Some(&Update::Completed));
    let resumed = prints(&claude);
    assert!(resumed[2].contains("--resume claude-"), "{resumed:?}");
}

#[test]
fn missing_session_reported_in_result_rebuilds_without_a_second_started() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let conversation = first_conversation(&adapter);

    claude.set("result-session-gone", "signed-in");
    let updates = visible(&run_to_end(
        adapter.send(follow_up(&conversation, history())).as_mut(),
    ));
    let started: Vec<_> = updates
        .iter()
        .filter(|update| matches!(update, Update::Started { .. }))
        .collect();
    assert_eq!(
        started,
        [&Update::Started {
            conversation_id: Some(conversation)
        }]
    );
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::ConversationCreated(_)))
    );
    assert_eq!(updates.last(), Some(&Update::Completed));
    assert!(!prints(&claude)[2].contains("--resume"));
}

#[test]
fn missing_session_without_history_fails_as_unknown() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter();
    let conversation = first_conversation(&adapter);

    claude.set("resume-fails", "signed-in");
    let updates = run_to_end(adapter.send(follow_up(&conversation, Vec::new())).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::InvalidRequest, "UNKNOWN_CONVERSATION")
    );
    // The stale mapping is gone, so the next attempt fails before Claude runs.
    let runs = prints(&claude).len();
    let updates = run_to_end(adapter.send(follow_up(&conversation, Vec::new())).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::InvalidRequest, "UNKNOWN_CONVERSATION")
    );
    assert_eq!(prints(&claude).len(), runs);
}

#[test]
fn without_a_session_directory_conversations_continue_in_memory() {
    let claude = FakeClaude::install("answers", "signed-in");
    let adapter = claude.adapter().without_session_dir();
    let conversation = first_conversation(&adapter);
    let updates = visible(&run_to_end(
        adapter.send(follow_up(&conversation, Vec::new())).as_mut(),
    ));
    assert_eq!(updates.last(), Some(&Update::Completed));
    assert!(prints(&claude)[1].contains("--resume claude-"));
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
fn output_after_the_result_does_not_put_off_completion() {
    let claude = FakeClaude::install("keeps-talking", "signed-in");
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

/// Writes a Claude Code transcript whose records name `session` and `cwd`.
fn transcript(path: &std::path::Path, session: &str, cwd: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let queued = serde_json::json!({"type": "queue-operation", "sessionId": session});
    let user = serde_json::json!({
        "type": "user",
        "cwd": cwd.to_string_lossy(),
        "sessionId": session,
        "message": {"role": "user", "content": "first"}
    });
    std::fs::write(path, format!("{queued}\n{user}\n")).unwrap();
}

#[test]
fn forget_removes_the_mapping_and_only_pervues_claude_files() {
    let claude = FakeClaude::install("answers", "signed-in");
    let config = claude.dir.join("claude-config");
    let adapter = claude.adapter().with_environment([(
        OsString::from("CLAUDE_CONFIG_DIR"),
        config.clone().into_os_string(),
    )]);
    let conversation = first_conversation(&adapter);
    let mapping = claude.dir.join("claude-work.sessions").join(&conversation);
    let session = std::fs::read_to_string(&mapping).unwrap();
    let workspace = std::fs::canonicalize(claude.dir.join("claude-work")).unwrap();

    let ours = config.join("projects/-pervue-claude-workspace");
    let theirs = config.join("projects/-home-someone-project");
    transcript(&ours.join(format!("{session}.jsonl")), &session, &workspace);
    std::fs::create_dir_all(ours.join(&session).join("subagents")).unwrap();
    std::fs::create_dir_all(config.join("session-env").join(&session)).unwrap();
    // The same session ID, but run somewhere else: not Pervue's to remove.
    transcript(
        &theirs.join(format!("{session}.jsonl")),
        &session,
        std::path::Path::new("/home/someone/project"),
    );
    transcript(
        &ours.join("other-session.jsonl"),
        "other-session",
        &workspace,
    );

    assert_eq!(
        run_to_end(adapter.forget(&conversation).as_mut()),
        [Update::Completed]
    );
    assert!(!ours.join(format!("{session}.jsonl")).exists());
    assert!(!ours.join(&session).exists());
    assert!(!config.join("session-env").join(&session).exists());
    assert!(theirs.join(format!("{session}.jsonl")).exists());
    assert!(ours.join("other-session.jsonl").exists());
    assert!(!mapping.exists());

    // Forgotten: it can't be continued, and forgetting again is harmless.
    let again = run_to_end(adapter.send(follow_up(&conversation, Vec::new())).as_mut());
    assert_eq!(
        failure(&again),
        (ErrorCode::InvalidRequest, "UNKNOWN_CONVERSATION")
    );
    assert_eq!(
        run_to_end(adapter.forget(&conversation).as_mut()),
        [Update::Completed]
    );
}
