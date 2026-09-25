//! The Codex adapter against a fake `codex` (this crate's binary, copied under
//! that name): discovery and sign-in status (PRO-02), requests and streaming
//! (PRO-03), and cancellation, timeouts, and failures (PRO-04).

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use pervue_host::diagnostics::Diagnostics;
use pervue_host::framing;
use pervue_host::host;
use pervue_host::protocol::events::{Authentication, Availability, Capability, ErrorCode};
use pervue_host::providers::codex::{Codex, LIMITS, Limits};
use pervue_host::providers::discovery::SearchPath;
use pervue_host::providers::{Exchange, Provider, Providers, SendRequest, Timeouts, Update};
use serde_json::Value;

const PROVIDER: &str = env!("CARGO_BIN_EXE_pervue-fake-provider");
const DEADLINE: Duration = Duration::from_secs(20);

/// Short limits, so failures show up quickly.
const TEST_LIMITS: Limits = Limits {
    timeouts: Timeouts {
        start: Duration::from_secs(10),
        idle: Duration::from_secs(10),
        stop_grace: Duration::from_millis(300),
    },
    probe: Duration::from_secs(5),
    finish: Duration::from_millis(300),
};

/// The grace period of a cancel that should stop a process promptly. On POSIX
/// the stop request, SIGTERM, ends the process well inside a long grace period.
/// Windows has no stop request that reaches a process not reading its input,
/// so there a short grace period ends in a kill.
const PROMPT_STOP_GRACE: Duration = if cfg!(unix) {
    Duration::from_secs(5)
} else {
    Duration::from_millis(300)
};

/// A directory holding a fake `codex` and the scenario it follows.
struct FakeCodex {
    dir: PathBuf,
}

impl FakeCodex {
    fn install(exec: &str, login: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pervue-fake-codex-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fake codex directory");
        let name = if cfg!(windows) { "codex.exe" } else { "codex" };
        std::fs::copy(PROVIDER, dir.join(name)).expect("install the fake codex");
        let codex = Self { dir };
        codex.set(exec, login);
        codex
    }

    fn set(&self, exec: &str, login: &str) {
        std::fs::write(
            self.dir.join("codex-scenario"),
            format!("exec={exec}\nlogin={login}\n"),
        )
        .expect("write the scenario");
    }

    fn adapter(&self) -> Codex {
        self.adapter_with(TEST_LIMITS)
    }

    fn adapter_with(&self, limits: Limits) -> Codex {
        Codex::new(SearchPath::new([self.dir.clone()]), self.dir.join("work")).with_limits(limits)
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.dir.join(file)).unwrap_or_default()
    }

    fn invocations(&self) -> Vec<String> {
        self.read("codex-invocations")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn prompts(&self) -> Vec<String> {
        self.read("codex-prompts")
            .split('\0')
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn pids(&self) -> Vec<u32> {
        self.read("codex-pids")
            .lines()
            .map(|pid| pid.parse().expect("a pid"))
            .collect()
    }

    /// Every `codex exec` this directory saw has exited and been reaped.
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

impl Drop for FakeCodex {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ask(text: &str) -> SendRequest {
    SendRequest {
        text: text.to_owned(),
        conversation_id: None,
        has_context: false,
    }
}

/// Pulls updates until a terminal one.
fn run_to_end(exchange: &mut dyn Exchange) -> Vec<Update> {
    let deadline = Instant::now() + DEADLINE;
    let mut updates = Vec::new();
    loop {
        let update = exchange
            .next(deadline)
            .expect("the exchange should end before the deadline");
        let terminal = update.is_terminal();
        updates.push(update);
        if terminal {
            return updates;
        }
    }
}

/// Pulls updates until `Started`.
fn run_until_started(exchange: &mut dyn Exchange) -> Vec<Update> {
    let deadline = Instant::now() + DEADLINE;
    let mut updates = Vec::new();
    loop {
        let update = exchange
            .next(deadline)
            .expect("the exchange should start before the deadline");
        assert!(!update.is_terminal(), "ended early: {update:?}");
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
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn status(codex: &Codex) -> (Availability, Authentication) {
    let updates = run_to_end(codex.status().as_mut());
    assert_eq!(updates.last(), Some(&Update::Completed));
    match &updates[0] {
        Update::Status {
            provider_id,
            status,
        } => {
            assert_eq!(provider_id, "codex");
            (status.availability, status.authentication)
        }
        other => panic!("expected a status, got {other:?}"),
    }
}

#[test]
fn status_reports_a_missing_codex_as_not_found() {
    let empty = FakeCodex::install("answers", "signed-in");
    std::fs::remove_file(
        empty
            .dir
            .join(if cfg!(windows) { "codex.exe" } else { "codex" }),
    )
    .unwrap();
    assert_eq!(
        status(&empty.adapter()),
        (Availability::NotFound, Authentication::Unknown)
    );
}

#[test]
fn status_reports_the_sign_in_from_the_exit_status_alone() {
    let codex = FakeCodex::install("answers", "signed-in");
    assert_eq!(
        status(&codex.adapter()),
        (Availability::Available, Authentication::Authenticated)
    );
    codex.set("answers", "signed-out");
    assert_eq!(
        status(&codex.adapter()),
        (Availability::Available, Authentication::Unauthenticated)
    );
    codex.set("answers", "broken");
    assert_eq!(
        status(&codex.adapter()),
        (Availability::Available, Authentication::Unknown)
    );
    assert!(
        codex
            .invocations()
            .iter()
            .all(|line| line.starts_with("login status"))
    );
}

#[test]
fn a_status_check_that_hangs_gives_up_as_unknown() {
    let codex = FakeCodex::install("answers", "hangs");
    let limits = Limits {
        probe: Duration::from_millis(300),
        ..TEST_LIMITS
    };
    let started = Instant::now();
    assert_eq!(
        status(&codex.adapter_with(limits)),
        (Availability::Available, Authentication::Unknown)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_codex_fails_the_request_as_not_found() {
    let codex = FakeCodex::install("answers", "signed-in");
    std::fs::remove_file(
        codex
            .dir
            .join(if cfg!(windows) { "codex.exe" } else { "codex" }),
    )
    .unwrap();
    let updates = run_to_end(codex.adapter().send(ask("hi")).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::ProviderNotFound, "EXECUTABLE_NOT_FOUND")
    );
}

#[test]
fn a_codex_that_cannot_be_started_is_unavailable() {
    // Found, since an execute bit is set, but it can't start. On POSIX its
    // owner may not execute it: macOS runs a text file it may execute with
    // /bin/sh. Root may execute it anyway, and Windows ignores the mode, but
    // neither runs a file that isn't a program.
    let codex = FakeCodex::install("answers", "signed-in");
    let path = codex
        .dir
        .join(if cfg!(windows) { "codex.exe" } else { "codex" });
    std::fs::write(&path, b"not a program\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o601)).unwrap();
    }

    assert_eq!(
        status(&codex.adapter()),
        (Availability::Unavailable, Authentication::Unknown)
    );
    let updates = run_to_end(codex.adapter().send(ask("hi")).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::ProviderFailed, "PROVIDER_UNAVAILABLE")
    );
}

#[test]
fn page_context_fails_the_request_instead_of_being_dropped() {
    // Codex doesn't receive browser context yet, so a request that attaches
    // some fails before anything runs rather than answer without it.
    let codex = FakeCodex::install("answers", "signed-in");
    let adapter = codex.adapter();
    let updates = run_to_end(
        adapter
            .send(SendRequest {
                has_context: true,
                ..ask("Summarize this page")
            })
            .as_mut(),
    );
    assert_eq!(
        failure(&updates),
        (ErrorCode::InvalidRequest, "PAGE_CONTEXT_UNSUPPORTED")
    );
    assert_eq!(updates.len(), 1);
    assert!(codex.invocations().is_empty(), "codex ran");

    // The status says so up front.
    let updates = run_to_end(adapter.status().as_mut());
    let Update::Status { status, .. } = &updates[0] else {
        panic!("expected a status, got {updates:?}");
    };
    assert_eq!(status.capabilities.page_context, Capability::Unsupported);
}

#[test]
fn a_signed_out_codex_fails_the_request_before_it_runs() {
    let codex = FakeCodex::install("answers", "signed-out");
    let updates = run_to_end(codex.adapter().send(ask("hi")).as_mut());
    assert_eq!(
        failure(&updates),
        (ErrorCode::ProviderNotAuthenticated, "LOGIN_REQUIRED")
    );
    assert_eq!(updates.len(), 1);
    assert!(codex.pids().is_empty(), "codex exec ran");
}

#[test]
fn a_question_streams_its_answer_and_opens_a_conversation() {
    let codex = FakeCodex::install("answers", "signed-in");
    let question = "Why? \"quoted\" $(id) `id` ; rm -rf ~\n é✓😀";
    let all = run_to_end(codex.adapter().send(ask(question)).as_mut());
    assert!(all.contains(&Update::Activity));
    let updates = visible(&all);

    let Update::ConversationCreated(conversation_id) = &updates[0] else {
        panic!("expected a new conversation: {updates:?}");
    };
    assert!(conversation_id.starts_with("conv_"), "{conversation_id}");
    assert!(
        !conversation_id.contains("thread"),
        "the Codex thread leaked"
    );
    assert_eq!(
        updates[1..],
        [
            Update::Started {
                conversation_id: Some(conversation_id.clone())
            },
            Update::Delta(format!("You asked: {question}")),
            Update::Completed,
        ]
    );

    // The question went to stdin, verbatim, and never onto the command line.
    assert_eq!(codex.prompts(), [question]);
    let invocations = codex.invocations();
    assert_eq!(invocations.len(), 2);
    assert!(invocations[0].starts_with("login status\t"));
    let (command, path) = invocations[1].split_once('\t').unwrap();
    let work = codex.dir.join("work");
    assert_eq!(
        command,
        format!(
            "exec --json --skip-git-repo-check --sandbox read-only -C {} -",
            work.display()
        )
    );
    // Codex's own directory comes first on its PATH, for `node`.
    assert_eq!(path, format!("PATH0={}", codex.dir.display()));
    codex.assert_nothing_left_running();
}

#[test]
fn a_conversation_continues_its_codex_thread() {
    let codex = FakeCodex::install("answers", "signed-in");
    let adapter = codex.adapter();
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation_id) = first[0].clone() else {
        panic!("expected a new conversation: {first:?}");
    };

    let second = run_to_end(
        adapter
            .send(SendRequest {
                conversation_id: Some(conversation_id.clone()),
                ..ask("second")
            })
            .as_mut(),
    );
    assert_eq!(
        visible(&second),
        [
            Update::Started {
                conversation_id: Some(conversation_id)
            },
            Update::Delta("You asked: second".to_owned()),
            Update::Completed,
        ]
    );
    let first_thread = format!("thread-{}", codex.pids()[0]);
    let resumed = codex.invocations()[3].clone();
    assert!(
        resumed.contains(&format!(" resume {first_thread} -\t")),
        "{resumed}"
    );
}

#[test]
fn an_unknown_conversation_fails_without_running_codex() {
    let codex = FakeCodex::install("answers", "signed-in");
    let updates = run_to_end(
        codex
            .adapter()
            .send(SendRequest {
                conversation_id: Some("conv_from_elsewhere".to_owned()),
                ..ask("hi")
            })
            .as_mut(),
    );
    assert_eq!(
        failure(&updates),
        (ErrorCode::InvalidRequest, "UNKNOWN_CONVERSATION")
    );
    assert!(codex.invocations().is_empty());
}

#[test]
fn several_messages_arrive_as_separate_deltas() {
    let codex = FakeCodex::install("two-messages", "signed-in");
    let updates = visible(&run_to_end(codex.adapter().send(ask("hi")).as_mut()));
    assert_eq!(
        updates[2..],
        [
            Update::Delta("First.".to_owned()),
            Update::Delta("\n\nSecond.".to_owned()),
            Update::Completed,
        ]
    );
}

#[test]
fn failed_turns_map_to_normalized_errors() {
    for (scenario, expected) in [
        (
            "fails-401",
            (ErrorCode::ProviderNotAuthenticated, "AUTH_REJECTED"),
        ),
        (
            "fails-429",
            (ErrorCode::ProviderFailed, "PROVIDER_RATE_LIMITED"),
        ),
        (
            "fails-500",
            (ErrorCode::ProviderFailed, "PROVIDER_UNAVAILABLE"),
        ),
        ("crashes", (ErrorCode::ProviderFailed, "PROCESS_EXITED")),
        (
            "no-result",
            (ErrorCode::ProviderFailed, "MALFORMED_PROVIDER_OUTPUT"),
        ),
        (
            "malformed",
            (ErrorCode::ProviderFailed, "MALFORMED_PROVIDER_OUTPUT"),
        ),
        (
            "oversized",
            (ErrorCode::ProviderFailed, "MALFORMED_PROVIDER_OUTPUT"),
        ),
    ] {
        let codex = FakeCodex::install(scenario, "signed-in");
        let updates = run_to_end(codex.adapter().send(ask("hi")).as_mut());
        assert_eq!(failure(&updates), expected, "{scenario}");
        let Some(Update::Failed(error)) = updates.last() else {
            unreachable!()
        };
        assert!(
            !error.message.contains("sk-"),
            "{scenario}: {}",
            error.message
        );
        codex.assert_nothing_left_running();
    }
}

#[test]
fn a_lost_codex_session_fails_as_a_process_exit() {
    let codex = FakeCodex::install("answers", "signed-in");
    let adapter = codex.adapter();
    let first = visible(&run_to_end(adapter.send(ask("first")).as_mut()));
    let Update::ConversationCreated(conversation_id) = first[0].clone() else {
        panic!("expected a new conversation: {first:?}");
    };
    codex.set("resume-fails", "signed-in");
    let updates = run_to_end(
        adapter
            .send(SendRequest {
                conversation_id: Some(conversation_id),
                ..ask("again")
            })
            .as_mut(),
    );
    assert_eq!(
        failure(&updates),
        (ErrorCode::ProviderFailed, "PROCESS_EXITED")
    );
}

#[test]
fn a_codex_that_lingers_after_its_turn_is_stopped_and_the_answer_kept() {
    let codex = FakeCodex::install("lingers", "signed-in");
    let started = Instant::now();
    let updates = run_to_end(codex.adapter().send(ask("hi")).as_mut());
    assert_eq!(updates.last(), Some(&Update::Completed));
    assert!(started.elapsed() >= TEST_LIMITS.finish);
    codex.assert_nothing_left_running();
}

#[test]
fn cancelling_mid_turn_stops_codex_promptly() {
    let codex = FakeCodex::install("goes-quiet", "signed-in");
    let mut exchange = codex.adapter().send(ask("hi"));
    run_until_started(exchange.as_mut());

    let started = Instant::now();
    exchange.cancel(PROMPT_STOP_GRACE);
    assert_eq!(run_to_end(exchange.as_mut()), [Update::Stopped]);
    assert!(started.elapsed() < Duration::from_secs(2));
    codex.assert_nothing_left_running();
}

#[test]
fn a_codex_that_ignores_cancellation_is_killed_after_the_grace_period() {
    let codex = FakeCodex::install("ignores-cancel", "signed-in");
    let mut exchange = codex.adapter().send(ask("hi"));
    run_until_started(exchange.as_mut());

    let grace = Duration::from_millis(300);
    let started = Instant::now();
    exchange.cancel(grace);
    assert_eq!(run_to_end(exchange.as_mut()), [Update::Stopped]);
    // Everywhere the process outlives the stop request: Windows has none to
    // send, and on POSIX it ignores SIGTERM.
    assert!(started.elapsed() >= grace);
    codex.assert_nothing_left_running();
}

#[test]
fn cancelling_during_the_sign_in_check_runs_nothing() {
    let codex = FakeCodex::install("answers", "hangs");
    let mut exchange = codex.adapter().send(ask("hi"));
    assert_eq!(exchange.next(Instant::now()), None);
    exchange.cancel(Duration::ZERO);
    assert_eq!(run_to_end(exchange.as_mut()), [Update::Stopped]);
    assert!(codex.pids().is_empty());
}

/// Frames, each sent after a pause, then the end of input after `linger`.
struct PacedInput {
    pending: VecDeque<(Duration, Vec<u8>)>,
    current: std::io::Cursor<Vec<u8>>,
    linger: Duration,
}

impl PacedInput {
    fn new(frames: &[(Duration, &str)], linger: Duration) -> Self {
        let pending = frames
            .iter()
            .map(|(pause, payload)| {
                let mut frame = Vec::new();
                framing::write_frame(&mut frame, payload.as_bytes()).unwrap();
                (*pause, frame)
            })
            .collect();
        Self {
            pending,
            current: std::io::Cursor::new(Vec::new()),
            linger,
        }
    }
}

impl Read for PacedInput {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let count = self.current.read(buffer)?;
            if count > 0 {
                return Ok(count);
            }
            match self.pending.pop_front() {
                Some((pause, frame)) => {
                    thread::sleep(pause);
                    self.current = std::io::Cursor::new(frame);
                }
                None => {
                    thread::sleep(std::mem::take(&mut self.linger));
                    return Ok(0);
                }
            }
        }
    }
}

/// Runs a host serving only `codex`, and returns its events and records.
fn serve(codex: Codex, mut input: PacedInput) -> (Vec<Value>, Vec<Value>) {
    let providers = Providers::new(vec![Box::new(codex)]);
    let mut output = Vec::new();
    let mut log = Diagnostics::new(Vec::new());
    host::run_with(&providers, &mut input, &mut output, &mut log).expect("a clean session");

    let mut wire = output.as_slice();
    let mut events = Vec::new();
    while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
        events.push(serde_json::from_slice(&frame).unwrap());
    }
    let records = String::from_utf8(log.into_inner())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (events, records)
}

fn names(events: &[Value]) -> Vec<(&str, &str)> {
    events[1..]
        .iter()
        .map(|event| {
            (
                event["request_id"].as_str().unwrap(),
                event["event"].as_str().unwrap(),
            )
        })
        .collect()
}

/// How long the tests keep the input open for an answer to arrive.
const ANSWER_TIME: Duration = Duration::from_secs(3);

const SEND: &str = r#"{"version":1,"type":"request","request_id":"req_ask","method":"conversation.send","payload":{"provider_id":"codex","input":{"text":"What is this page about?"}}}"#;

#[test]
fn a_question_travels_through_the_host_as_protocol_events() {
    let codex = FakeCodex::install("slow", "signed-in");
    // The input stays open while Codex answers: closing it would cancel.
    let (events, records) = serve(
        codex.adapter(),
        PacedInput::new(&[(Duration::ZERO, SEND)], ANSWER_TIME),
    );
    assert_eq!(
        names(&events),
        [
            ("req_ask", "conversation.created"),
            ("req_ask", "response.started"),
            ("req_ask", "response.delta"),
            ("req_ask", "response.delta"),
            ("req_ask", "response.completed"),
        ]
    );
    let conversation_id = events[1]["payload"]["conversation_id"].as_str().unwrap();
    assert_eq!(
        events[2]["payload"],
        serde_json::json!({"provider_id": "codex", "conversation_id": conversation_id})
    );
    assert_eq!(events[3]["payload"]["text"], "one");
    assert_eq!(events[4]["payload"]["text"], "\n\ntwo");

    // Nothing Codex wrote to stderr, and none of the question, reaches the
    // events or the diagnostics.
    let everything = format!("{events:?}{records:?}");
    assert!(!everything.contains("SECRET"));
    assert!(!everything.contains("What is this page about?"));
    assert_eq!(records[1]["event"], "request.completed");
    assert_eq!(records[1]["provider_id"], "codex");
    assert_eq!(records[1]["conversation_id"], conversation_id);
}

#[test]
fn a_long_answer_is_split_into_frames_that_fit() {
    let codex = FakeCodex::install("huge", "signed-in");
    let (events, _) = serve(
        codex.adapter(),
        PacedInput::new(&[(Duration::ZERO, SEND)], ANSWER_TIME),
    );
    let deltas: Vec<&str> = events
        .iter()
        .filter(|event| event["event"] == "response.delta")
        .map(|event| event["payload"]["text"].as_str().unwrap())
        .collect();
    assert!(deltas.len() > 1);
    assert_eq!(deltas.concat(), "é✓😀 ".repeat(30_000));
}

#[test]
fn a_cancel_request_stops_codex_mid_turn() {
    let codex = FakeCodex::install("goes-quiet", "signed-in");
    let cancel = r#"{"version":1,"type":"request","request_id":"req_stop","method":"request.cancel","payload":{"target_request_id":"req_ask"}}"#;
    let (events, records) = serve(
        codex.adapter(),
        PacedInput::new(
            &[
                (Duration::ZERO, SEND),
                (Duration::from_millis(1500), cancel),
            ],
            Duration::ZERO,
        ),
    );
    assert_eq!(
        names(&events),
        [
            ("req_ask", "conversation.created"),
            ("req_ask", "response.started"),
            ("req_ask", "response.failed"),
            ("req_stop", "request.cancelled"),
        ]
    );
    assert_eq!(events[3]["payload"]["error"]["code"], "REQUEST_CANCELLED");
    assert_eq!(records[2]["event"], "request.completed");
    assert_eq!(records[2]["target_request_id"], "req_ask");
    codex.assert_nothing_left_running();
}

#[test]
fn a_codex_that_goes_quiet_times_out() {
    let codex = FakeCodex::install("goes-quiet", "signed-in");
    let limits = Limits {
        timeouts: Timeouts {
            idle: Duration::from_millis(500),
            ..TEST_LIMITS.timeouts
        },
        ..TEST_LIMITS
    };
    let (events, _) = serve(
        codex.adapter_with(limits),
        PacedInput::new(&[(Duration::ZERO, SEND)], Duration::from_secs(4)),
    );
    let last = events.last().unwrap();
    assert_eq!(last["event"], "response.failed");
    assert_eq!(last["payload"]["error"]["code"], "REQUEST_TIMEOUT");
    assert_eq!(
        last["payload"]["error"]["reason"],
        "PROVIDER_RESPONSE_TIMEOUT"
    );
    codex.assert_nothing_left_running();
}

#[test]
fn a_codex_that_never_starts_its_turn_times_out() {
    let codex = FakeCodex::install("never-starts", "signed-in");
    let limits = Limits {
        timeouts: Timeouts {
            start: Duration::from_millis(800),
            ..TEST_LIMITS.timeouts
        },
        ..TEST_LIMITS
    };
    let (events, _) = serve(
        codex.adapter_with(limits),
        PacedInput::new(&[(Duration::ZERO, SEND)], Duration::from_secs(4)),
    );
    assert_eq!(names(&events), [("req_ask", "response.failed")]);
    assert_eq!(
        events[1]["payload"]["error"]["reason"],
        "PROVIDER_START_TIMEOUT"
    );
    codex.assert_nothing_left_running();
}

#[test]
fn closing_the_input_stops_a_running_codex() {
    let codex = FakeCodex::install("goes-quiet", "signed-in");
    let (events, _) = serve(
        codex.adapter(),
        PacedInput::new(&[(Duration::ZERO, SEND)], Duration::from_millis(1500)),
    );
    let last = events.last().unwrap();
    assert_eq!(last["payload"]["error"]["reason"], "INPUT_CLOSED");
    codex.assert_nothing_left_running();
}

#[test]
fn the_default_limits_allow_a_slow_answer() {
    // Codex sends no token deltas, so the idle limit must allow for a model
    // thinking for minutes, while a stuck start is caught within a minute.
    assert!(LIMITS.timeouts.idle >= Duration::from_secs(300));
    assert!(LIMITS.timeouts.start <= Duration::from_secs(60));
    assert!(LIMITS.timeouts.stop_grace <= Duration::from_secs(2));
}
