//! TST-04: the hostile fake-process matrix.
//!
//! Each case runs the whole host (request loop, Codex adapter, stream
//! manager, and process manager) against a fake `codex` that misbehaves in
//! one way: a slow stream, a stderr flood, a nonzero exit, a hang, an ignored
//! cancellation, malformed output, or large output, alone or next to others.
//! Every request must end in its normalized outcome within a time bound, with
//! each process reaped. Where the platform shows them (Linux), the host's
//! threads and file descriptors must return to where they were, and its peak
//! memory must stay within a bound far below what the provider wrote.
//!
//! The cases run one after another in a single test, so the resource
//! measurements see only the case under way.
//!
//! The runtime runs the same misbehaviour under its own scheduler, with none
//! of Pervue's host around it, in `runtime-tests`.

mod support;

use std::time::Duration;

use pervue_host::providers::Timeouts;
use pervue_host::providers::codex::Limits;
use runtime_fake_provider::resources::{held, peak_memory_growth, reset_peak_memory, settle};
use serde_json::Value;
use support::{FakeCodex, PacedInput, Session, TEST_LIMITS, request_id_for, serve_timed};

/// How long a session may wait for its requests to end. Every case expects
/// far less: this only keeps a broken host from hanging the test.
const SESSION_LIMIT: Duration = Duration::from_secs(90);

/// How long a case may take before the test gives up on a stuck host.
const CASE_LIMIT: Duration = Duration::from_secs(120);

/// How much the host's peak memory may grow during a case. The floods write
/// far more than this, and the longest line the Codex adapter holds is 8 MiB.
const MEMORY_GROWTH_LIMIT_KIB: u64 = 64 * 1024;

/// The largest `response.delta` text the host writes.
const MAX_DELTA_BYTES: usize = 64 * 1024;

/// How a request ends.
#[derive(Debug, Clone)]
enum Ending {
    /// `response.completed`, after deltas that read this answer.
    Answer(String),
    /// `response.failed` with this code and reason.
    Failed(&'static str, &'static str),
    /// `request.cancelled`, confirming a cancellation.
    Confirmed,
    /// `provider.status` with this authentication, then `response.completed`.
    Status(&'static str),
}

/// One input frame, sent this long after the previous one.
struct Frame {
    pause: Duration,
    request_id: &'static str,
    json: String,
}

fn ask(pause_ms: u64, request_id: &'static str, text: &str) -> Frame {
    let json = serde_json::json!({
        "version": 1,
        "type": "request",
        "request_id": request_id_for(request_id),
        "method": "conversation.send",
        "payload": {"provider_id": "codex", "input": {"text": text}}
    });
    Frame {
        pause: Duration::from_millis(pause_ms),
        request_id,
        json: json.to_string(),
    }
}

fn status(pause_ms: u64, request_id: &'static str) -> Frame {
    let json = serde_json::json!({
        "version": 1,
        "type": "request",
        "request_id": request_id_for(request_id),
        "method": "provider.status",
        "payload": {"provider_id": "codex"}
    });
    Frame {
        pause: Duration::from_millis(pause_ms),
        request_id,
        json: json.to_string(),
    }
}

fn cancel(pause_ms: u64, request_id: &'static str, target: &str) -> Frame {
    let json = serde_json::json!({
        "version": 1,
        "type": "request",
        "request_id": request_id_for(request_id),
        "method": "request.cancel",
        "payload": {"target_request_id": request_id_for(target)}
    });
    Frame {
        pause: Duration::from_millis(pause_ms),
        request_id,
        json: json.to_string(),
    }
}

/// Test limits, with the start and idle timeouts given in milliseconds.
fn limits(start_ms: u64, idle_ms: u64) -> Limits {
    Limits {
        timeouts: Timeouts {
            start: Duration::from_millis(start_ms),
            idle: Duration::from_millis(idle_ms),
            ..TEST_LIMITS.timeouts
        },
        ..TEST_LIMITS
    }
}

struct Case {
    name: &'static str,
    /// The fake `codex exec` behavior. With `by-prompt`, each question's
    /// first word names its own.
    exec: &'static str,
    /// The fake `codex login status` behavior.
    login: &'static str,
    limits: Limits,
    input: Vec<Frame>,
    /// Each request's ending, and at the latest how long after its frame was
    /// sent it comes.
    expect: Vec<(&'static str, Ending, Duration)>,
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn answer(text: &str) -> Ending {
    Ending::Answer(text.to_owned())
}

const TIMED_OUT: Ending = Ending::Failed("REQUEST_TIMEOUT", "PROVIDER_RESPONSE_TIMEOUT");
const EXITED: Ending = Ending::Failed("PROVIDER_FAILED", "PROCESS_EXITED");
const MALFORMED: Ending = Ending::Failed("PROVIDER_FAILED", "MALFORMED_PROVIDER_OUTPUT");
const CANCELLED: Ending = Ending::Failed("REQUEST_CANCELLED", "USER_CANCELLED");

fn cases() -> Vec<Case> {
    let quick = limits(1_000, 1_000);
    vec![
        Case {
            name: "slow stream: every line arrives a few bytes at a time",
            login: "signed-in",
            exec: "dribble",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Dribble é✓😀 for me")],
            expect: vec![("req", answer("You asked: Dribble é✓😀 for me"), secs(5))],
        },
        Case {
            name: "stderr flood: 128 MiB of stderr while answering",
            login: "signed-in",
            exec: "stderr-flood",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Answer through the noise")],
            expect: vec![(
                "req",
                answer("You asked: Answer through the noise"),
                secs(30),
            )],
        },
        Case {
            name: "stderr without end: stderr is not progress",
            login: "signed-in",
            exec: "endless-stderr",
            limits: quick,
            input: vec![ask(0, "req", "Say nothing")],
            expect: vec![("req", TIMED_OUT, secs(5))],
        },
        Case {
            name: "stdout flood: 100,000 progress events, then the answer",
            login: "signed-in",
            exec: "stdout-flood",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Work hard")],
            expect: vec![("req", answer("Done flooding."), secs(30))],
        },
        Case {
            name: "progress without end, cancelled",
            login: "signed-in",
            exec: "endless-flood",
            limits: TEST_LIMITS,
            input: vec![
                ask(0, "req", "Never finish"),
                cancel(500, "req_stop", "req"),
            ],
            expect: vec![
                ("req", CANCELLED, secs(5)),
                ("req_stop", Ending::Confirmed, secs(2)),
            ],
        },
        Case {
            name: "unknown events without end: not progress",
            login: "signed-in",
            exec: "unknown-flood",
            limits: quick,
            input: vec![ask(0, "req", "Speak in riddles")],
            expect: vec![("req", TIMED_OUT, secs(5))],
        },
        Case {
            name: "nonzero exit mid-turn",
            login: "signed-in",
            exec: "exits-nonzero",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Give up")],
            expect: vec![("req", EXITED, secs(5))],
        },
        Case {
            name: "crash mid-turn",
            login: "signed-in",
            exec: "crashes",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Fall over")],
            expect: vec![("req", EXITED, secs(5))],
        },
        Case {
            name: "hang before the turn starts",
            login: "signed-in",
            exec: "never-starts",
            limits: quick,
            input: vec![ask(0, "req", "Wait forever")],
            expect: vec![(
                "req",
                Ending::Failed("REQUEST_TIMEOUT", "PROVIDER_START_TIMEOUT"),
                secs(5),
            )],
        },
        Case {
            name: "hang mid-turn",
            login: "signed-in",
            exec: "goes-quiet",
            limits: quick,
            input: vec![ask(0, "req", "Go quiet")],
            expect: vec![("req", TIMED_OUT, secs(5))],
        },
        Case {
            name: "ignored cancellation: killed after the grace period",
            login: "signed-in",
            exec: "ignores-cancel",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Ignore me"), cancel(500, "req_stop", "req")],
            expect: vec![
                ("req", CANCELLED, secs(5)),
                ("req_stop", Ending::Confirmed, secs(2)),
            ],
        },
        Case {
            name: "ignored cancellation while flooding",
            login: "signed-in",
            exec: "floods-and-ignores-cancel",
            limits: TEST_LIMITS,
            input: vec![
                ask(0, "req", "Ignore me loudly"),
                cancel(500, "req_stop", "req"),
            ],
            expect: vec![
                ("req", CANCELLED, secs(5)),
                ("req_stop", Ending::Confirmed, secs(2)),
            ],
        },
        Case {
            name: "malformed output: not JSON",
            login: "signed-in",
            exec: "malformed",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Garble")],
            expect: vec![("req", MALFORMED, secs(5))],
        },
        Case {
            name: "malformed output: not UTF-8",
            login: "signed-in",
            exec: "invalid-utf8",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Garble bytes")],
            expect: vec![("req", MALFORMED, secs(5))],
        },
        Case {
            name: "large output: a 330 KB answer, split into frames that fit",
            login: "signed-in",
            exec: "huge",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Say a lot")],
            expect: vec![("req", Ending::Answer("é✓😀 ".repeat(30_000)), secs(10))],
        },
        Case {
            name: "large output: a 9 MiB line",
            login: "signed-in",
            exec: "oversized",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Say too much")],
            expect: vec![("req", MALFORMED, secs(10))],
        },
        Case {
            name: "large output: a line without end",
            login: "signed-in",
            exec: "endless-line",
            limits: TEST_LIMITS,
            input: vec![ask(0, "req", "Never stop talking")],
            expect: vec![("req", MALFORMED, secs(10))],
        },
        Case {
            name: "a sign-in check that floods: given up on, and the question asked",
            login: "floods",
            exec: "answers",
            limits: Limits {
                probe: Duration::from_millis(1_000),
                ..TEST_LIMITS
            },
            input: vec![status(0, "req_status"), ask(0, "req", "Ask anyway")],
            expect: vec![
                ("req_status", Ending::Status("unknown"), secs(4)),
                ("req", answer("You asked: Ask anyway"), secs(5)),
            ],
        },
        Case {
            name: "hostile neighbours: a plain answer and a cancel get through",
            login: "signed-in",
            exec: "by-prompt",
            limits: limits(10_000, 2_000),
            input: vec![
                ask(0, "req_flood", "endless-flood"),
                ask(0, "req_stderr", "endless-stderr"),
                ask(0, "req_line", "endless-line"),
                ask(0, "req_quiet", "goes-quiet"),
                ask(300, "req_answer", "answers please"),
                cancel(700, "req_stop", "req_flood"),
            ],
            expect: vec![
                ("req_answer", answer("You asked: answers please"), secs(2)),
                ("req_stop", Ending::Confirmed, secs(2)),
                ("req_flood", CANCELLED, secs(4)),
                ("req_stderr", TIMED_OUT, secs(6)),
                ("req_line", MALFORMED, secs(10)),
                ("req_quiet", TIMED_OUT, secs(6)),
            ],
        },
    ]
}

#[test]
fn hostile_providers_end_normalized_with_bounded_resources() {
    let baseline = held();
    for case in cases() {
        let name = case.name;
        let memory_before = reset_peak_memory();
        let started = std::time::Instant::now();
        // A watchdog: a host stuck in a loop fails the case instead of
        // hanging the test.
        let (done, finished) = std::sync::mpsc::channel();
        let runner = std::thread::spawn(move || {
            run(&case);
            let _ = done.send(());
        });
        match finished.recv_timeout(CASE_LIMIT) {
            Ok(()) => runner.join().expect("the case runs"),
            // The case panicked: report its own message.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(panic) = runner.join() {
                    std::panic::resume_unwind(panic);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("{name}: the host is stuck")
            }
        }
        let growth = peak_memory_growth(memory_before);
        eprintln!(
            "{:>8.2?}  {name} (peak memory +{} KiB)",
            started.elapsed(),
            growth.map_or_else(|| "?".to_owned(), |growth| growth.to_string())
        );
        if let Some(growth) = growth {
            assert!(
                growth <= MEMORY_GROWTH_LIMIT_KIB,
                "{name}: peak memory grew by {growth} KiB"
            );
        }
        assert_eq!(
            settle(baseline),
            baseline,
            "{name}: threads or file descriptors were left behind"
        );
    }
}

fn run(case: &Case) {
    let codex = FakeCodex::install(case.exec, case.login);
    let frames: Vec<(Duration, &str)> = case
        .input
        .iter()
        .map(|frame| (frame.pause, frame.json.as_str()))
        .collect();
    let waiting: Vec<String> = case
        .expect
        .iter()
        .map(|(name, _, _)| request_id_for(name))
        .collect();
    let waiting: Vec<&str> = waiting.iter().map(String::as_str).collect();
    let session = serve_timed(
        codex.adapter_with(case.limits),
        PacedInput::new(&frames, SESSION_LIMIT),
        &waiting,
    );

    for (request_id, ending, within) in &case.expect {
        check(case, &session, request_id, ending, *within);
    }

    // Nothing the provider wrote to stderr, and no question, reaches the
    // events or the diagnostics.
    let events: Vec<&Value> = session.events.iter().map(|timed| &timed.event).collect();
    let everything = format!("{events:?}{:?}", session.records);
    assert!(
        !everything.contains("SECRET"),
        "{}: a secret leaked",
        case.name
    );
    for frame in &case.input {
        let question =
            serde_json::from_str::<Value>(&frame.json).unwrap()["payload"]["input"]["text"].clone();
        if let Some(question) = question.as_str() {
            let records = format!("{:?}", session.records);
            assert!(
                !records.contains(question),
                "{}: a question was logged",
                case.name
            );
        }
    }
    codex.assert_nothing_left_running();
}

fn check(case: &Case, session: &Session, request_id: &str, ending: &Ending, within: Duration) {
    let name = case.name;
    let index = case
        .input
        .iter()
        .position(|frame| frame.request_id == request_id)
        .expect("an expected request is sent");
    let sent = session.sent[index];
    let wire_id = request_id_for(request_id);
    let events = session.of(&wire_id);
    let kinds: Vec<&str> = events
        .iter()
        .map(|timed| timed.event["event"].as_str().unwrap())
        .collect();
    let terminal = |kind: &&str| {
        matches!(
            *kind,
            "response.completed" | "response.failed" | "request.cancelled"
        )
    };
    assert_eq!(
        kinds.iter().filter(|kind| terminal(kind)).count(),
        1,
        "{name}: {request_id} should end exactly once: {kinds:?}"
    );
    let last = events.last().expect("the request has events");
    assert!(
        terminal(&kinds[kinds.len() - 1]),
        "{name}: {request_id} ended early: {kinds:?}"
    );
    let took = last.at.saturating_duration_since(sent);
    eprintln!("          {request_id}: {took:.2?} of {within:?}");
    assert!(
        took <= within,
        "{name}: {request_id} took {took:?}, more than {within:?}"
    );

    let record = session
        .records
        .iter()
        .find(|record| {
            record["request_id"] == wire_id.as_str()
                && matches!(
                    record["event"].as_str(),
                    Some("request.completed" | "request.failed")
                )
        })
        .unwrap_or_else(|| panic!("{name}: {request_id} has no outcome record"));

    match ending {
        Ending::Answer(text) => {
            assert_eq!(
                kinds[..2],
                ["conversation.created", "response.started"],
                "{name}: {request_id}"
            );
            assert_eq!(
                kinds[kinds.len() - 1],
                "response.completed",
                "{name}: {request_id}"
            );
            let deltas: Vec<&str> = events
                .iter()
                .filter(|timed| timed.event["event"] == "response.delta")
                .map(|timed| timed.event["payload"]["text"].as_str().unwrap())
                .collect();
            assert!(
                deltas.iter().all(|delta| delta.len() <= MAX_DELTA_BYTES),
                "{name}: {request_id} has an oversized delta"
            );
            assert!(
                deltas.concat() == *text,
                "{name}: {request_id} answered differently"
            );
            assert_eq!(record["event"], "request.completed", "{name}: {request_id}");
        }
        Ending::Failed(code, reason) => {
            assert_eq!(
                kinds[kinds.len() - 1],
                "response.failed",
                "{name}: {request_id}"
            );
            let error = &last.event["payload"]["error"];
            assert_eq!(
                (error["code"].as_str(), error["reason"].as_str()),
                (Some(*code), Some(*reason)),
                "{name}: {request_id}"
            );
            assert_eq!(record["event"], "request.failed", "{name}: {request_id}");
            assert_eq!(record["error"]["code"], *code, "{name}: {request_id}");
            assert_eq!(record["error"]["reason"], *reason, "{name}: {request_id}");
        }
        Ending::Confirmed => {
            assert_eq!(kinds, ["request.cancelled"], "{name}: {request_id}");
            assert_eq!(record["event"], "request.completed", "{name}: {request_id}");
        }
        Ending::Status(authentication) => {
            assert_eq!(
                kinds,
                ["provider.status", "response.completed"],
                "{name}: {request_id}"
            );
            assert_eq!(
                events[0].event["payload"]["status"]["authentication"], *authentication,
                "{name}: {request_id}"
            );
            assert_eq!(record["event"], "request.completed", "{name}: {request_id}");
        }
    }
}
