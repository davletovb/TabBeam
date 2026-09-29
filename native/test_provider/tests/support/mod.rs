//! The fake `codex` harness shared by the tests that run the Codex adapter, or
//! the whole host, against it.

#![allow(dead_code, reason = "each test crate uses part of the harness")]

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pervue_host::conversations::{Conversations, Durability, SessionStore};
use pervue_host::diagnostics::Diagnostics;
use pervue_host::framing;
use pervue_host::host;
use pervue_host::providers::claude::Claude;
use pervue_host::providers::codex::{Codex, Limits};
use pervue_host::providers::gemini::Gemini;
use pervue_host::providers::grok::Grok;
use pervue_host::providers::{ConversationProvider, Providers};
use seatline_fake_provider::harness::{self, Fixtures};
#[allow(unused_imports)]
pub use seatline_fake_provider::harness::{PROMPT_STOP_GRACE, TEST_LIMITS};
use serde_json::Value;

pub const PROVIDER: &str = env!("CARGO_BIN_EXE_pervue-fake-provider");
pub const FIXTURES: Fixtures = Fixtures::new(
    PROVIDER,
    env!("CARGO_TARGET_TMPDIR"),
    pervue_host::NAMESPACE,
);

/// A directory holding a fake `codex`, served the way Pervue serves it:
/// conversations over the adapter, mapped to Codex threads in a directory
/// beside the workspace.
pub struct FakeCodex(harness::FakeCodex);

impl Deref for FakeCodex {
    type Target = harness::FakeCodex;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FakeCodex {
    pub fn file_name() -> &'static str {
        harness::FakeCodex::file_name()
    }

    pub fn install(exec: &str, login: &str) -> Self {
        Self(harness::FakeCodex::install(FIXTURES, exec, login))
    }

    pub fn adapter(&self) -> Conversations<Codex> {
        self.adapter_with(TEST_LIMITS)
    }

    pub fn adapter_with(&self, limits: Limits) -> Conversations<Codex> {
        Conversations::new(self.codex(limits), self.sessions())
    }

    /// [`FakeCodex::adapter`] with the environment Codex gets from `host`.
    pub fn adapter_with_env<I>(&self, host: I) -> Conversations<Codex>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        Conversations::new(
            self.codex(TEST_LIMITS).with_environment(host),
            self.sessions(),
        )
    }

    /// The bare adapter, which knows no conversations.
    pub fn codex(&self, limits: Limits) -> Codex {
        self.0.adapter_with(limits)
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(Some(self.dir.join("work.sessions")))
            .with_durability(Durability::Required)
    }
}

/// A directory holding a fake `claude`, served the way Pervue serves it:
/// conversations over the adapter, mapped to Claude sessions in a directory
/// beside the workspace.
pub struct FakeClaude(harness::FakeClaude);

impl Deref for FakeClaude {
    type Target = harness::FakeClaude;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FakeClaude {
    pub fn file_name() -> &'static str {
        harness::FakeClaude::file_name()
    }

    pub fn install(print: &str, auth: &str) -> Self {
        Self(harness::FakeClaude::install(FIXTURES, print, auth))
    }

    pub fn adapter(&self) -> Conversations<Claude> {
        Conversations::new(self.claude(), self.sessions())
    }

    /// [`FakeClaude::adapter`] with the environment Claude gets from `host`.
    pub fn adapter_with_env<I>(&self, host: I) -> Conversations<Claude>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        Conversations::new(self.claude().with_environment(host), self.sessions())
    }

    /// As [`FakeClaude::adapter`], with no directory for mappings: they live
    /// in memory, as when the host has no data directory.
    pub fn adapter_in_memory(&self) -> Conversations<Claude> {
        Conversations::new(self.claude(), SessionStore::new(None))
    }

    pub fn adapter_in_memory_with_env<I>(&self, host: I) -> Conversations<Claude>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        Conversations::new(
            self.claude().with_environment(host),
            SessionStore::new(None),
        )
    }

    /// The bare adapter, which knows no conversations.
    pub fn claude(&self) -> Claude {
        self.0.adapter()
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(Some(self.dir.join("claude-work.sessions")))
    }
}

/// A directory holding a fake `agy`, served the way Pervue serves it:
/// conversations over the adapter.
pub struct FakeGemini(harness::FakeGemini);

impl Deref for FakeGemini {
    type Target = harness::FakeGemini;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FakeGemini {
    pub fn install() -> Self {
        Self(harness::FakeGemini::install(FIXTURES))
    }

    pub fn adapter(&self) -> Conversations<Gemini> {
        Conversations::new(self.gemini(), SessionStore::new(None))
    }

    /// The bare adapter, which knows no conversations.
    pub fn gemini(&self) -> Gemini {
        self.0.adapter()
    }
}

/// A directory holding a fake `grok`, served the way Pervue serves it:
/// conversations over the adapter.
pub struct FakeGrok(harness::FakeGrok);

impl Deref for FakeGrok {
    type Target = harness::FakeGrok;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FakeGrok {
    pub fn install() -> Self {
        Self(harness::FakeGrok::install(FIXTURES))
    }

    pub fn adapter(&self) -> Conversations<Grok> {
        Conversations::new(self.0.adapter(), SessionStore::new(None))
    }

    pub fn adapter_with_environment(
        &self,
        extra: &[(&str, std::path::PathBuf)],
    ) -> Conversations<Grok> {
        Conversations::new(
            self.0.adapter_with_environment(extra),
            SessionStore::new(None),
        )
    }
}

/// Frames, each sent after a pause, then the end of input: after `linger`, or
/// as soon as the session says every request it waits for has ended.
pub struct PacedInput {
    pending: VecDeque<(Duration, Vec<u8>)>,
    current: io::Cursor<Vec<u8>>,
    linger: Duration,
    ended: Option<Receiver<()>>,
    sent: Arc<Mutex<Vec<Instant>>>,
}

impl PacedInput {
    pub fn new(frames: &[(Duration, &str)], linger: Duration) -> Self {
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
            current: io::Cursor::new(Vec::new()),
            linger,
            ended: None,
            sent: Arc::default(),
        }
    }
}

impl Read for PacedInput {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            let count = self.current.read(buffer)?;
            if count > 0 {
                return Ok(count);
            }
            match self.pending.pop_front() {
                Some((pause, frame)) => {
                    thread::sleep(pause);
                    self.sent.lock().unwrap().push(Instant::now());
                    self.current = io::Cursor::new(frame);
                }
                None => {
                    let linger = std::mem::take(&mut self.linger);
                    match &self.ended {
                        Some(ended) => {
                            let _ = ended.recv_timeout(linger);
                        }
                        None => thread::sleep(linger),
                    }
                    return Ok(0);
                }
            }
        }
    }
}

/// Runs a host serving only `codex`, and returns its events and records.
pub fn serve(
    codex: impl ConversationProvider + 'static,
    mut input: PacedInput,
) -> (Vec<Value>, Vec<Value>) {
    let providers = Providers::new(vec![Box::new(codex)]);
    let mut output = Vec::new();
    let mut log = Diagnostics::new(Vec::new());
    host::run_with(&providers, &mut input, &mut output, &mut log).expect("a clean session");

    let mut wire = output.as_slice();
    let mut events = Vec::new();
    while let Some(frame) = framing::read_frame(&mut wire).unwrap() {
        events.push(serde_json::from_slice(&frame).unwrap());
    }
    (events, records(log))
}

fn records(log: Diagnostics<Vec<u8>>) -> Vec<Value> {
    String::from_utf8(log.into_inner())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The ID the extension would give the request named `name`: `req_` and a
/// UUID, the one shape the host's diagnostics keep, here spelling `name` (16
/// bytes at most) in hex.
pub fn request_id_for(name: &str) -> String {
    assert!(name.len() <= 16, "{name} is too long");
    let value = name
        .bytes()
        .fold(0_u128, |value, byte| (value << 8) | u128::from(byte));
    let hex = format!("{value:032x}");
    format!(
        "req_{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// The `(request_id, event)` of each event after `host.ready`.
pub fn names(events: &[Value]) -> Vec<(&str, &str)> {
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

/// An event the host wrote, and when it finished writing it.
#[derive(Debug, Clone)]
pub struct Timed {
    pub at: Instant,
    pub event: Value,
}

/// What a timed host session did.
pub struct Session {
    pub events: Vec<Timed>,
    pub records: Vec<Value>,
    /// When each input frame was sent, in order.
    pub sent: Vec<Instant>,
    pub started: Instant,
    pub ended: Instant,
}

impl Session {
    /// The events of `request_id`, in order.
    pub fn of(&self, request_id: &str) -> Vec<&Timed> {
        self.events
            .iter()
            .filter(|timed| timed.event["request_id"] == request_id)
            .collect()
    }
}

/// Collects the frames the host writes, noting when each was complete, and
/// says when every request in `waiting` has ended.
struct TimedSink {
    bytes: Vec<u8>,
    parsed: usize,
    events: Vec<Timed>,
    waiting: HashSet<String>,
    ended: Option<Sender<()>>,
}

impl Write for TimedSink {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        let now = Instant::now();
        loop {
            let mut rest = &self.bytes[self.parsed..];
            let before = rest.len();
            let Ok(Some(frame)) = framing::read_frame(&mut rest) else {
                break;
            };
            self.parsed += before - rest.len();
            let event: Value = serde_json::from_slice(&frame).expect("a JSON event");
            if matches!(
                event["event"].as_str(),
                Some("response.completed" | "response.failed" | "request.cancelled")
            ) {
                if let Some(request_id) = event["request_id"].as_str() {
                    self.waiting.remove(request_id);
                }
                if self.waiting.is_empty() {
                    if let Some(ended) = self.ended.take() {
                        let _ = ended.send(());
                    }
                }
            }
            self.events.push(Timed { at: now, event });
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs a host serving only `provider` on `input`, whose end comes as soon as
/// every request in `waiting` has ended, and records when each event came.
pub fn serve_timed(
    provider: impl ConversationProvider + 'static,
    mut input: PacedInput,
    waiting: &[&str],
) -> Session {
    let (ended, when_ended) = mpsc::channel();
    input.ended = Some(when_ended);
    let sent = Arc::clone(&input.sent);
    let mut sink = TimedSink {
        bytes: Vec::new(),
        parsed: 0,
        events: Vec::new(),
        waiting: waiting.iter().map(|id| (*id).to_owned()).collect(),
        ended: Some(ended),
    };
    let providers = Providers::new(vec![Box::new(provider)]);
    let mut log = Diagnostics::new(Vec::new());
    let started = Instant::now();
    host::run_with(&providers, &mut input, &mut sink, &mut log).expect("a clean session");
    let ended = Instant::now();
    assert_eq!(
        sink.parsed,
        sink.bytes.len(),
        "the host wrote an incomplete frame"
    );
    let sent = sent.lock().unwrap().clone();
    Session {
        events: sink.events,
        records: records(log),
        sent,
        started,
        ended,
    }
}
