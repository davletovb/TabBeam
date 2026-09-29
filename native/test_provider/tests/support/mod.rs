//! The fake `codex` harness shared by the tests that run the Codex adapter, or
//! the whole host, against it.

#![allow(dead_code, reason = "each test crate uses part of the harness")]

use std::collections::{HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pervue_host::conversations::{Conversations, Durability, SessionStore};
use pervue_host::diagnostics::Diagnostics;
use pervue_host::framing;
use pervue_host::host;
use pervue_host::providers::claude::{Claude, Limits as ClaudeLimits};
use pervue_host::providers::codex::{Codex, Limits};
use pervue_host::providers::{ConversationProvider, Providers, Timeouts};
use runtime_core::discovery::SearchPath;
use serde_json::Value;

pub const PROVIDER: &str = env!("CARGO_BIN_EXE_pervue-fake-provider");

/// Short limits, so failures show up quickly.
pub const TEST_LIMITS: Limits = Limits {
    timeouts: Timeouts {
        start: Duration::from_secs(10),
        idle: Duration::from_secs(10),
        max_turn: Duration::from_secs(30),
        stop_grace: Duration::from_millis(300),
    },
    probe: Duration::from_secs(5),
    finish: Duration::from_millis(300),
};

/// The grace period of a cancel that should stop a process promptly. On POSIX
/// the stop request, SIGTERM, ends the process well inside a long grace period.
/// Windows has no stop request that reaches a process not reading its input,
/// so there a short grace period ends in a kill.
pub const PROMPT_STOP_GRACE: Duration = if cfg!(unix) {
    Duration::from_secs(5)
} else {
    Duration::from_millis(300)
};

/// A directory holding a fake `codex` and the scenario it follows.
pub struct FakeCodex {
    pub dir: PathBuf,
}

impl FakeCodex {
    pub fn install(exec: &str, login: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        // In the target directory, on the same file system as the binary.
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "pervue-fake-codex-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fake codex directory");
        // A hard link, not a copy. A copy is open for writing while it is
        // made, and a process another test thread starts at that moment
        // inherits it, so running the copy fails as a busy text file
        // (ETXTBSY) until that process has started. A link is never open.
        // A test that changes the installed `codex` must replace it, never
        // write through it, which would change the fake provider itself.
        let codex_path = dir.join(Self::file_name());
        if std::fs::hard_link(PROVIDER, &codex_path).is_err() {
            std::fs::copy(PROVIDER, &codex_path).expect("install the fake codex");
        }
        let codex = Self { dir };
        codex.set(exec, login);
        codex
    }

    pub fn file_name() -> &'static str {
        if cfg!(windows) { "codex.exe" } else { "codex" }
    }

    pub fn set(&self, exec: &str, login: &str) {
        std::fs::write(
            self.dir.join("codex-scenario"),
            format!("exec={exec}\nlogin={login}\n"),
        )
        .expect("write the scenario");
    }

    /// Codex, served the way Pervue serves it: conversations over the adapter,
    /// mapped to Codex threads in a directory beside the workspace.
    pub fn adapter(&self) -> Conversations<Codex> {
        self.adapter_with(TEST_LIMITS)
    }

    pub fn adapter_with(&self, limits: Limits) -> Conversations<Codex> {
        Conversations::new(self.codex(limits), self.sessions())
    }

    /// [`FakeCodex::adapter`] with the environment Codex gets from `host`.
    pub fn adapter_with_env<I>(&self, host: I) -> Conversations<Codex>
    where
        I: IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    {
        Conversations::new(
            self.codex(TEST_LIMITS).with_environment(host),
            self.sessions(),
        )
    }

    /// The bare adapter, which knows no conversations.
    pub fn codex(&self, limits: Limits) -> Codex {
        Codex::new(SearchPath::new([self.dir.clone()]), self.dir.join("work")).with_limits(limits)
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(Some(self.dir.join("work.sessions")))
            .with_durability(Durability::Required)
    }

    pub fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.dir.join(file)).unwrap_or_default()
    }

    pub fn invocations(&self) -> Vec<String> {
        self.read("codex-invocations")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn prompts(&self) -> Vec<String> {
        self.read("codex-prompts")
            .split('\0')
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn pids(&self) -> Vec<u32> {
        self.read("codex-pids")
            .lines()
            .map(|pid| pid.parse().expect("a pid"))
            .collect()
    }

    /// Every `codex exec` this directory saw has exited and been reaped.
    pub fn assert_nothing_left_running(&self) {
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

/// Short Claude adapter limits for integration tests.
pub const CLAUDE_TEST_LIMITS: ClaudeLimits = ClaudeLimits {
    timeouts: Timeouts {
        start: Duration::from_secs(10),
        idle: Duration::from_secs(10),
        max_turn: Duration::from_secs(30),
        stop_grace: Duration::from_millis(300),
    },
    probe: Duration::from_secs(5),
    finish: Duration::from_millis(300),
};

/// A directory holding a fake `claude` CLI and its scenario.
pub struct FakeClaude {
    pub dir: PathBuf,
}

impl FakeClaude {
    pub fn install(print: &str, auth: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "pervue-fake-claude-support-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fake Claude directory");
        let path = dir.join(Self::file_name());
        if std::fs::hard_link(PROVIDER, &path).is_err() {
            std::fs::copy(PROVIDER, &path).expect("install fake Claude");
        }
        let fake = Self { dir };
        fake.set(print, auth);
        fake
    }

    pub fn file_name() -> &'static str {
        if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        }
    }

    pub fn set(&self, print: &str, auth: &str) {
        std::fs::write(
            self.dir.join("claude-scenario"),
            format!("print={print}\nauth={auth}\n"),
        )
        .expect("write Claude scenario");
    }

    /// Claude, served the way Pervue serves it: conversations over the
    /// adapter, mapped to Claude sessions in a directory beside the workspace.
    pub fn adapter(&self) -> Conversations<Claude> {
        Conversations::new(self.claude(), self.sessions())
    }

    /// [`FakeClaude::adapter`] with the environment Claude gets from `host`.
    pub fn adapter_with_env<I>(&self, host: I) -> Conversations<Claude>
    where
        I: IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
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
        I: IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    {
        Conversations::new(
            self.claude().with_environment(host),
            SessionStore::new(None),
        )
    }

    /// The bare adapter, which knows no conversations.
    pub fn claude(&self) -> Claude {
        Claude::new(
            SearchPath::new([self.dir.clone()]),
            self.dir.join("claude-work"),
        )
        .with_limits(CLAUDE_TEST_LIMITS)
    }

    pub fn sessions(&self) -> SessionStore {
        SessionStore::new(Some(self.dir.join("claude-work.sessions")))
    }

    pub fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.dir.join(file)).unwrap_or_default()
    }

    pub fn invocations(&self) -> Vec<String> {
        self.read("claude-invocations")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn prompts(&self) -> Vec<String> {
        self.read("claude-prompts")
            .split('\0')
            .filter(|prompt| !prompt.is_empty())
            .map(str::to_owned)
            .collect()
    }

    pub fn pids(&self) -> Vec<u32> {
        self.read("claude-pids")
            .lines()
            .map(|pid| pid.parse().expect("a pid"))
            .collect()
    }

    pub fn assert_nothing_left_running(&self) {
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
