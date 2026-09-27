//! PRO-06: the opt-in smoke test against the real Claude CLI. The built host
//! finds the installed Claude, checks its sign-in, asks it one question, and
//! streams the answer to completion: discovery → send → stream → completion.
//!
//! `PERVUE_LIVE_CLAUDE` turns it on. Unset, as in normal CI runs, the test
//! passes at once. Set to `1`, it runs when Claude is installed and signed in,
//! and is skipped, passing, when it isn't. Set to `required`, as in a job set
//! up with a sign-in, a missing or signed-out Claude fails it instead.
//!
//! Nothing it prints can hold a credential. The host never forwards or logs
//! what Claude writes, and the test checks everything it is about to print,
//! events and diagnostics alike, for the values of credential variables in its
//! environment and for anything shaped like an API key.
//!
//! ```bash
//! cd native
//! PERVUE_LIVE_CLAUDE=1 cargo test -p pervue-host --test live_claude -- --nocapture
//! ```

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pervue_core::framing;
use serde_json::Value;

const HOST: &str = env!("CARGO_BIN_EXE_pervue-host");
const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/";
/// Claude's own status check gives up after 10 seconds.
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
/// A real model can take a while.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// Request IDs in the shape the extension gives its requests, which the
/// host's diagnostics keep.
const STATUS_ID: &str = "req_5a7a0000-0000-4000-8000-000000000001";
const ASK_ID: &str = "req_5a7a0000-0000-4000-8000-000000000002";
const SEARCH_ID: &str = "req_5a7a0000-0000-4000-8000-000000000003";
const FOLLOW_ID: &str = "req_5a7a0000-0000-4000-8000-000000000004";
const STATUS: &str = r#"{"version":1,"type":"request","request_id":"req_5a7a0000-0000-4000-8000-000000000001","method":"provider.status","payload":{"provider_id":"claude"}}"#;
const QUESTION: &str = "Reply with the single word: pong";
const ASK: &str = r#"{"version":1,"type":"request","request_id":"req_5a7a0000-0000-4000-8000-000000000002","method":"conversation.send","payload":{"provider_id":"claude","input":{"text":"Reply with the single word: pong"}}}"#;
const SEARCH_QUESTION: &str = "Search the web for the official Rust language website and answer in one short sentence with a source.";
const SEARCH: &str = r#"{"version":1,"type":"request","request_id":"req_5a7a0000-0000-4000-8000-000000000003","method":"conversation.send","payload":{"provider_id":"claude","input":{"text":"Search the web for the official Rust language website and answer in one short sentence with a source."},"search":{}}}"#;

/// Environment variables that may hold a credential in a CI job.
const CREDENTIAL_VARIABLES: &[&str] = &["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    /// Run when Claude is installed and signed in; skip otherwise.
    IfReady,
    /// Run, and fail when Claude isn't installed or signed in.
    Required,
}

fn mode() -> Mode {
    match std::env::var("PERVUE_LIVE_CLAUDE").as_deref() {
        Err(_) | Ok("" | "0") => Mode::Off,
        Ok("required") => Mode::Required,
        Ok(_) => Mode::IfReady,
    }
}

/// The host under test, with its stdout read as events and its stderr kept.
/// It is killed and reaped however the test ends, even on a timeout, so a
/// stuck Claude never leaves it running.
struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Value>,
    readers: Vec<JoinHandle<()>>,
    stderr: Receiver<String>,
    /// The host's data directory, so its Claude session mapping lands here
    /// instead of in the user's own.
    data: PathBuf,
}

impl Host {
    #[allow(
        clippy::disallowed_methods,
        reason = "this test starts the built host binary; the spawn guard is for the host itself"
    )]
    fn start() -> Self {
        let (event_sender, events) = mpsc::channel();
        let (stderr_sender, stderr) = mpsc::channel();
        let data = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pervue-live-claude-{}", std::process::id()));
        let mut command = Command::new(HOST);
        command
            .arg(ORIGIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The host keeps Claude session mappings under XDG_DATA_HOME, which
        // Claude itself doesn't inherit.
        #[cfg(unix)]
        command.env("XDG_DATA_HOME", &data);
        // Straight into the guard, so the host is reaped whatever happens next.
        let mut host = Self {
            child: command.spawn().expect("start the host"),
            stdin: None,
            events,
            readers: Vec::new(),
            stderr,
            data,
        };
        host.stdin = host.child.stdin.take();
        let mut stdout = host.child.stdout.take().expect("stdout");
        let mut stderr_pipe = host.child.stderr.take().expect("stderr");
        host.readers.push(thread::spawn(move || {
            while let Ok(Some(frame)) = framing::read_frame(&mut stdout) {
                let event = serde_json::from_slice(&frame).expect("a JSON event");
                if event_sender.send(event).is_err() {
                    return;
                }
            }
        }));
        host.readers.push(thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr_pipe.read_to_string(&mut text);
            let _ = stderr_sender.send(text);
        }));
        host
    }

    fn send(&mut self, request: &str) {
        let mut wire = Vec::new();
        framing::write_frame(&mut wire, request.as_bytes()).unwrap();
        let stdin = self.stdin.as_mut().expect("the input is open");
        stdin.write_all(&wire).unwrap();
        stdin.flush().unwrap();
    }

    /// The events of `request_id` up to its terminal one.
    fn until_end(&self, request_id: &str, timeout: Duration) -> Vec<Value> {
        let deadline = Instant::now() + timeout;
        let mut received = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("{request_id} should end within {timeout:?}"));
            if event["request_id"] != request_id {
                continue;
            }
            let terminal = matches!(
                event["event"].as_str(),
                Some("response.completed" | "response.failed")
            );
            received.push(event);
            if terminal {
                return received;
            }
        }
    }

    /// Closes the input, which ends the host, and returns its diagnostics.
    fn finish(mut self) -> String {
        drop(self.stdin.take());
        assert!(self.child.wait().expect("the host exits").success());
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        self.stderr.recv().unwrap_or_default()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

/// Panics, without printing it, if `text` holds a credential: the value of a
/// credential variable in this environment, or anything shaped like an API
/// key.
fn assert_no_credentials(what: &str, text: &str) {
    for name in CREDENTIAL_VARIABLES {
        if let Ok(value) = std::env::var(name) {
            assert!(
                value.len() < 8 || !text.contains(&value),
                "{what}: the value of {name} appears"
            );
        }
    }
    let shaped_like_a_key = text.match_indices("sk-").any(|(start, _)| {
        text[start + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .count()
            >= 16
    });
    assert!(
        !shaped_like_a_key,
        "{what}: something shaped like an API key appears"
    );
}

/// Why the test can't run here, when Claude isn't ready.
fn skip_or_fail(mode: Mode, reason: &str) {
    assert_ne!(
        mode,
        Mode::Required,
        "PERVUE_LIVE_CLAUDE=required: {reason}"
    );
    eprintln!("skipped: {reason}");
}

#[test]
fn live_claude_answers_a_question() {
    let mode = mode();
    if mode == Mode::Off {
        eprintln!("skipped: set PERVUE_LIVE_CLAUDE=1 to ask the installed Claude");
        return;
    }
    let mut host = Host::start();
    let started = Instant::now();

    // Discovery and sign-in.
    host.send(STATUS);
    let status = host.until_end(STATUS_ID, STATUS_TIMEOUT);
    let state = &status
        .iter()
        .find(|event| event["event"] == "provider.status")
        .expect("a provider.status event")["payload"]["status"];
    let (availability, authentication) = (
        state["availability"].as_str().unwrap(),
        state["authentication"].as_str().unwrap(),
    );
    eprintln!("Claude is {availability}, {authentication}");
    if availability != "available" {
        return skip_or_fail(mode, &format!("Claude is {availability}"));
    }
    if authentication == "unauthenticated" {
        return skip_or_fail(mode, "Claude isn't signed in");
    }

    // Send, stream, and complete.
    host.send(ASK);
    let answer = host.until_end(ASK_ID, ANSWER_TIMEOUT);
    let took = started.elapsed();
    let last = answer.last().unwrap();
    if last["event"] == "response.failed"
        && last["payload"]["error"]["code"] == "PROVIDER_NOT_AUTHENTICATED"
    {
        return skip_or_fail(mode, "Claude isn't signed in");
    }
    let kinds: Vec<&str> = answer
        .iter()
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    eprintln!("events: {kinds:?}");

    assert_eq!(kinds.first(), Some(&"conversation.created"), "{last}");
    assert_eq!(kinds.get(1), Some(&"response.started"), "{last}");
    assert_eq!(answer[1]["payload"]["provider_id"], "claude");
    assert!(kinds.contains(&"response.delta"), "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"response.completed"), "{last}");
    let text: String = answer
        .iter()
        .filter(|event| event["event"] == "response.delta")
        .map(|event| event["payload"]["text"].as_str().unwrap())
        .collect();
    eprintln!("Claude answered in {took:.1?}: {text}");
    assert!(text.to_lowercase().contains("pong"), "unexpected answer");

    // A native WebSearch turn must return structured sources, then the same
    // Claude session must still accept a plain follow-up with tools disabled.
    host.send(SEARCH);
    let searched = host.until_end(SEARCH_ID, ANSWER_TIMEOUT);
    let search_last = searched.last().unwrap();
    assert_eq!(
        search_last["event"], "response.completed",
        "native search failed: {search_last}"
    );
    assert!(
        searched
            .iter()
            .any(|event| event["event"] == "response.source"),
        "native search silently completed without sources: {searched:?}"
    );
    let conversation = searched
        .iter()
        .find(|event| event["event"] == "conversation.created")
        .and_then(|event| event["payload"]["conversation_id"].as_str())
        .expect("search opens a conversation")
        .to_owned();
    let follow = format!(
        r#"{{"version":1,"type":"request","request_id":"{FOLLOW_ID}","method":"conversation.send","payload":{{"provider_id":"claude","conversation_id":"{conversation}","input":{{"text":"Reply with the single word: followup"}}}}}}"#
    );
    host.send(&follow);
    let followed = host.until_end(FOLLOW_ID, ANSWER_TIMEOUT);
    let follow_last = followed.last().unwrap();
    assert_eq!(
        follow_last["event"], "response.completed",
        "plain follow-up after search failed: {follow_last}"
    );
    assert!(
        followed
            .iter()
            .any(|event| event["event"] == "response.delta"),
        "plain follow-up returned no answer"
    );

    let diagnostics = host.finish();
    let printed = format!("{answer:?}{searched:?}{followed:?}{diagnostics}");
    assert_no_credentials("the events and diagnostics", &printed);
    eprintln!("diagnostics:\n{diagnostics}");

    // The host's own records name the request, never its content.
    assert!(!diagnostics.contains(QUESTION));
    assert!(!diagnostics.contains(SEARCH_QUESTION));
    assert!(!diagnostics.to_lowercase().contains("pong"));
    assert!(!diagnostics.to_lowercase().contains("followup"));
}
