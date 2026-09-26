//! Claude Code CLI adapter (PRO-05/06).
//!
//! Pervue discovers the fixed `claude` executable through the shared provider
//! search path, checks `claude auth status`, and drives print mode through
//! stream-json on stdin/stdout. Provider session IDs remain private to this
//! adapter; the host and extension see only opaque Pervue conversation IDs.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::codex::workspace;
use super::discovery;
use super::environment;
use super::forget;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::provider_prompt;
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use pervue_core::discovery::SearchPath;
use pervue_core::process::{Event, Exit, Process, ProcessSpec};
use pervue_core::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "claude";
const EXECUTABLE: &str = "claude";

/// Non-secret Claude/Node configuration needed to reproduce a working CLI
/// launch from Chrome's much smaller environment. Proxies are already in
/// [`environment::INHERITED`]; Node reads extra CA certificates only from
/// `NODE_EXTRA_CA_CERTS`.
pub const CLAUDE_VARIABLES: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_GIT_BASH_PATH",
    "NODE_EXTRA_CA_CERTS",
];

/// How much of Claude's stderr a turn keeps, to tell a missing session apart
/// from any other early exit. Never logged or forwarded.
const STDERR_TAIL_BYTES: usize = 4096;

pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeouts: Timeouts,
    pub probe: Duration,
    /// How long Claude may linger after its terminal `result` event.
    pub finish: Duration,
}

pub const LIMITS: Limits = Limits {
    timeouts: Timeouts {
        start: Duration::from_secs(60),
        idle: Duration::from_secs(300),
        stop_grace: Duration::from_secs(2),
    },
    probe: Duration::from_secs(10),
    finish: Duration::from_secs(5),
};

/// Claude's first proven adapter surface. Page context is deliberately
/// unsupported until its own untrusted-context/tool boundary is implemented.
pub const CAPABILITIES: Capabilities = Capabilities {
    streaming: Capability::Supported,
    continuation: Capability::Supported,
    web_search: Capability::Unknown,
    page_context: Capability::Unsupported,
    attachments: Capability::Unsupported,
    model_selection: Capability::Unsupported,
    cancellation: Capability::Supported,
};

const NOT_INSTALLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotFound,
    reason: "EXECUTABLE_NOT_FOUND",
    message: "Claude isn't installed. Install Claude Code, then try again.",
    retryable: false,
};

const NOT_SIGNED_IN: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "LOGIN_REQUIRED",
    message: "Claude isn't signed in. Run \"claude auth login\" in a terminal, then try again.",
    retryable: false,
};

const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "This conversation can't be continued. Start a new one.",
    retryable: false,
};

const START_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Claude couldn't start. Reinstall Claude Code, then try again.",
    retryable: false,
};

const NO_WORKSPACE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_UNAVAILABLE",
    message: "Pervue couldn't prepare a private folder for Claude. Check your cache folder and try again.",
    retryable: false,
};

const PROCESS_EXITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROCESS_EXITED",
    message: "Claude stopped unexpectedly. Try again.",
    retryable: true,
};

const MALFORMED_OUTPUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MALFORMED_PROVIDER_OUTPUT",
    message: "Claude answered in a way Pervue doesn't understand. Update Claude Code and Pervue, then try again.",
    retryable: false,
};

const SESSION_GONE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "Claude's saved session no longer exists. Start a new conversation.",
    retryable: false,
};

const SESSION_STORE_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InternalError,
    reason: "SESSION_STORE_FAILED",
    message: "Claude's conversation could not be saved. Check available disk space and try again.",
    retryable: true,
};

type Conversations = Rc<RefCell<HashMap<String, String>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Launch {
    work_dir: PathBuf,
    inherited: Vec<(OsString, OsString)>,
    path: Option<OsString>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        let path = environment::lookup(&host, "PATH").map(OsStr::to_os_string);
        Self {
            work_dir,
            inherited: environment::inherit(host, CLAUDE_VARIABLES),
            path,
        }
    }

    fn workspace(&self) -> std::io::Result<PathBuf> {
        workspace::prepare(&self.work_dir)
    }

    fn command<I>(&self, workspace: &Path, executable: &Path, args: I) -> ProcessSpec
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env("DISABLE_AUTOUPDATER", "1")
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            .current_dir(workspace)
    }
}

pub struct Claude {
    search: SearchPath,
    launch: Rc<Launch>,
    /// Where conversation-to-session mappings are kept across host restarts.
    /// Without one (no data directory), they are kept in memory only.
    session_dir: Option<PathBuf>,
    limits: Limits,
    conversations: Conversations,
}

impl Claude {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        let mut claude = Self::new(
            discovery::installed(),
            workspace::default_for(&host, "claude"),
        );
        claude.session_dir = installed_session_dir();
        claude
    }

    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        Self {
            search,
            session_dir: Some(work_dir.with_extension("sessions")),
            launch: Rc::new(Launch::new(work_dir, std::env::vars_os().collect())),
            limits: LIMITS,
            conversations: Rc::default(),
        }
    }

    #[must_use]
    pub fn with_environment<I>(mut self, host: I) -> Self
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        self.launch = Rc::new(Launch::new(
            self.launch.work_dir.clone(),
            host.into_iter().collect(),
        ));
        self
    }

    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn with_session_dir(mut self, session_dir: PathBuf) -> Self {
        self.session_dir = Some(session_dir);
        self
    }

    /// Keeps conversation mappings in memory only, as when the host has no
    /// data directory.
    #[must_use]
    pub fn without_session_dir(mut self) -> Self {
        self.session_dir = None;
        self
    }

    fn executable(&self) -> Option<PathBuf> {
        self.search.find(EXECUTABLE)
    }
}

impl Provider for Claude {
    fn id(&self) -> &str {
        ID
    }

    fn timeouts(&self) -> Timeouts {
        self.limits.timeouts
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    fn status(&self) -> Box<dyn Exchange> {
        let Some(executable) = self.executable() else {
            return Box::new(Scripted::new([
                status_update(Availability::NotFound, Authentication::Unknown),
                Update::Completed,
            ]));
        };
        Box::new(match probe(&self.launch, &executable) {
            Ok(process) => StatusCheck::Probing {
                process,
                give_up: after(self.limits.probe),
            },
            Err(_) => StatusCheck::Done(VecDeque::from([
                status_update(Availability::Unavailable, Authentication::Unknown),
                Update::Completed,
            ])),
        })
    }

    fn forget(&self, conversation_id: &str) -> Box<dyn Exchange> {
        let session = self
            .conversations
            .borrow()
            .get(conversation_id)
            .cloned()
            .or_else(|| {
                self.session_dir
                    .as_deref()
                    .and_then(|dir| read_session(dir, conversation_id))
            });
        let config = claude_config_dir(&self.launch);
        let workspace = self.launch.work_dir.clone();
        let session_dir = self.session_dir.clone();
        let conversation = conversation_id.to_owned();
        let conversations = Rc::clone(&self.conversations);
        let forgotten = conversation_id.to_owned();
        // The files go on their own thread. The mappings go last, the one in
        // memory only once everything else is gone: if Claude's files can't
        // all be removed, a retry can still find them.
        forget::in_background(
            move || {
                if let (Some(config), Some(session)) = (config, session) {
                    forget_transcript(&config, &workspace, &session)?;
                }
                session_dir.map_or(Ok(()), |dir| forget_session(&dir, &conversation))
            },
            move || {
                conversations.borrow_mut().remove(&forgotten);
            },
        )
    }

    fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
        let Some(executable) = self.executable() else {
            return Box::new(Scripted::failed(NOT_INSTALLED));
        };
        // The host enforces page_context=false before this method is called.
        if request.context.is_some() {
            return Box::new(Scripted::failed(MALFORMED_OUTPUT));
        }

        let mut conversation_id = request.conversation_id;
        let mut fallback_prompt = (!request.history.is_empty())
            .then(|| provider_prompt(&request.history, None, &request.text));
        let mut prompt = request.text;
        let resume = match &conversation_id {
            None => None,
            Some(id) => match self.conversations.borrow().get(id).cloned().or_else(|| {
                self.session_dir
                    .as_deref()
                    .and_then(|dir| read_session(dir, id))
            }) {
                Some(session) => Some(session),
                None if !request.history.is_empty() => None,
                None => return Box::new(Scripted::failed(UNKNOWN_CONVERSATION)),
            },
        };
        if resume.is_none() && !request.history.is_empty() {
            prompt = fallback_prompt.take().expect("history is present");
            conversation_id = None;
        }

        let mut turn = Turn {
            stage: Stage::Done,
            executable,
            launch: Rc::clone(&self.launch),
            session_dir: self.session_dir.clone(),
            prompt,
            fallback_prompt,
            resume,
            conversation_id,
            conversations: Rc::clone(&self.conversations),
            finish_grace: self.limits.finish,
            queue: VecDeque::new(),
            cancelled: false,
            started: false,
            announced: false,
            rebuilding: false,
            saw_delta: false,
            messages: 0,
            break_before_text: false,
            outcome: None,
            finish_by: None,
        };
        match probe(&turn.launch, &turn.executable) {
            Ok(process) => {
                turn.stage = Stage::Probing {
                    process,
                    give_up: after(self.limits.probe),
                };
            }
            Err(_) => turn.start(),
        }
        Box::new(turn)
    }
}

fn status_update(availability: Availability, authentication: Authentication) -> Update {
    Update::Status {
        provider_id: ID.to_owned(),
        status: ProviderState {
            availability,
            authentication,
            capabilities: CAPABILITIES,
        },
    }
}

fn probe(launch: &Launch, executable: &Path) -> std::io::Result<Process> {
    let workspace = launch.workspace()?;
    let mut process = Process::spawn(&launch.command(&workspace, executable, ["auth", "status"]))?;
    process.close_stdin();
    Ok(process)
}

fn signed_in(exit: &Exit) -> Authentication {
    match exit.status.and_then(|status| status.code()) {
        Some(0) => Authentication::Authenticated,
        Some(1) => Authentication::Unauthenticated,
        _ => Authentication::Unknown,
    }
}

fn search_path_for(executable: &Path, inherited: Option<&OsStr>) -> OsString {
    let dirs = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(inherited.into_iter().flat_map(std::env::split_paths));
    std::env::join_paths(dirs)
        .unwrap_or_else(|_| inherited.map(OsStr::to_os_string).unwrap_or_default())
}

fn after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

fn installed_session_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA"));
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|path| Path::new(path).is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share").into_os_string())
        });
    base.map(PathBuf::from)
        .map(|path| path.join("pervue/claude-sessions"))
}

fn session_name(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("conv_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_session(dir: &Path, id: &str) -> Option<String> {
    if !session_name(id) {
        return None;
    }
    let mut content = String::new();
    std::fs::File::open(dir.join(id))
        .ok()?
        .take(129)
        .read_to_string(&mut content)
        .ok()?;
    output::is_session_id(&content).then_some(content)
}

fn save_session(dir: &Path, id: &str, session: &str) -> io::Result<()> {
    if !session_name(id) || !output::is_session_id(session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session mapping",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;

    let path = dir.join(id);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(session.as_bytes())?;
    file.sync_all()
}

/// Removes a stored mapping, if any.
fn forget_session(dir: &Path, id: &str) -> io::Result<()> {
    if session_name(id) {
        forget::remove(&dir.join(id))
    } else {
        Ok(())
    }
}

/// Claude Code's own directory: `CLAUDE_CONFIG_DIR`, or `.claude` in the
/// user's home.
fn claude_config_dir(launch: &Launch) -> Option<PathBuf> {
    environment::lookup(&launch.inherited, "CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| environment::home_dir(&launch.inherited).map(|home| home.join(".claude")))
}

/// Removes Claude Code's saved files for `session` when Claude wrote them for
/// Pervue: `projects/<project>/<session>.jsonl` transcripts that name the
/// session and record Pervue's workspace as where they ran, the directory
/// beside each, and the session's own `session-env`, `tasks`, and
/// `file-history` directories. The transcripts, which prove the session is
/// Pervue's, go last, so a removal that fails can be retried.
fn forget_transcript(config: &Path, workspace: &Path, session: &str) -> io::Result<()> {
    let projects = match std::fs::read_dir(config.join("projects")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        projects => projects?,
    };
    let mut transcripts = Vec::new();
    for project in projects {
        let project = project?;
        if !project.file_type()?.is_dir() {
            continue;
        }
        let transcript = project.path().join(format!("{session}.jsonl"));
        if transcript_written_for_pervue(&transcript, session, workspace) {
            transcripts.push((transcript, project.path().join(session)));
        }
    }
    if transcripts.is_empty() {
        return Ok(());
    }
    for dir in ["session-env", "tasks", "file-history"] {
        forget::remove(&config.join(dir).join(session))?;
    }
    for (_, beside) in &transcripts {
        forget::remove(beside)?;
    }
    for (transcript, _) in &transcripts {
        forget::remove(transcript)?;
    }
    Ok(())
}

/// Whether the first record of `transcript` that says where it ran names
/// `session` and Pervue's `workspace`.
fn transcript_written_for_pervue(transcript: &Path, session: &str, workspace: &Path) -> bool {
    forget::head_lines(transcript).is_some_and(|lines| {
        lines
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|record| record.get("cwd").is_some())
            .is_some_and(|record| {
                record.get("sessionId").and_then(serde_json::Value::as_str) == Some(session)
                    && record
                        .get("cwd")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|cwd| forget::same_directory(cwd, workspace))
            })
    })
}

fn new_conversation_id(conversations: &HashMap<String, String>) -> String {
    loop {
        let id = format!(
            "conv_{:016x}",
            RandomState::new().hash_one((SystemTime::now(), conversations.len()))
        );
        if !conversations.contains_key(&id) {
            return id;
        }
    }
}

enum StatusCheck {
    Probing { process: Process, give_up: Instant },
    Done(VecDeque<Update>),
}

impl Exchange for StatusCheck {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        let busy_until = deadline.max(after(BUSY_LIMIT));
        loop {
            let authentication = match self {
                Self::Done(updates) => return updates.pop_front(),
                Self::Probing { process, give_up } if Instant::now() >= *give_up => {
                    process.kill();
                    Authentication::Unknown
                }
                Self::Probing { process, give_up } => {
                    match process.next_event(deadline.min(*give_up)) {
                        Some(Event::Exited(exit)) => signed_in(&exit),
                        Some(Event::Stdout(_) | Event::Stderr(_)) => {
                            if Instant::now() >= busy_until {
                                return None;
                            }
                            continue;
                        }
                        None if Instant::now() >= *give_up => continue,
                        None => return None,
                    }
                }
            };
            *self = Self::Done(VecDeque::from([
                status_update(Availability::Available, authentication),
                Update::Completed,
            ]));
        }
    }

    fn cancel(&mut self, _grace: Duration) {
        *self = Self::Done(VecDeque::from([Update::Stopped]));
    }
}

struct Turn {
    stage: Stage,
    executable: PathBuf,
    launch: Rc<Launch>,
    session_dir: Option<PathBuf>,
    prompt: String,
    /// Used once when a mapped Claude session is stale.
    fallback_prompt: Option<String>,
    resume: Option<String>,
    conversation_id: Option<String>,
    conversations: Conversations,
    finish_grace: Duration,
    queue: VecDeque<Update>,
    cancelled: bool,
    /// This Claude run sent `init`.
    started: bool,
    /// `Started` went to the host. A history rebuild after that doesn't send
    /// it again.
    announced: bool,
    /// Rerunning from history under the same conversation ID, after Claude
    /// said the mapped session no longer exists.
    rebuilding: bool,
    saw_delta: bool,
    messages: usize,
    break_before_text: bool,
    outcome: Option<Result<(), ErrorBody<'static>>>,
    finish_by: Option<Instant>,
}

enum Stage {
    Probing { process: Process, give_up: Instant },
    Running(LineStream),
    Done,
}

impl Turn {
    fn start(&mut self) {
        let Ok(workspace) = self.launch.workspace() else {
            return self.end(Update::Failed(NO_WORKSPACE));
        };
        let mut args: Vec<OsString> = [
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-mode",
            "default",
            // Pervue's Claude adapter is a conversational provider, not an
            // agent. Disable built-in tools, load none of the user's MCP
            // servers, and deny MCP tools explicitly as well.
            "--tools",
            "",
            "--strict-mcp-config",
            "--disallowedTools",
            "mcp__*",
        ]
        .map(OsString::from)
        .into();
        if let Some(session) = &self.resume {
            args.extend([OsString::from("--resume"), OsString::from(session)]);
        }
        let input = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": &self.prompt}]
            }
        })
        .to_string()
            + "\n";
        match Process::spawn(&self.launch.command(&workspace, &self.executable, args)) {
            Ok(mut process) => {
                let _ = process.write(input.as_bytes());
                process.close_stdin();
                self.prompt.clear();
                self.stage = Stage::Running(
                    LineStream::new(process, MAX_LINE_BYTES).keeping_stderr_tail(STDERR_TAIL_BYTES),
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.end(Update::Failed(NOT_INSTALLED))
            }
            Err(_) => self.end(Update::Failed(START_FAILED)),
        }
    }

    fn end(&mut self, update: Update) {
        self.queue.push_back(update);
        self.stage = Stage::Done;
    }

    fn on_line(&mut self, line: &str) {
        if self.cancelled || self.outcome.is_some() {
            return;
        }
        match output::parse(line) {
            Err(_) => self.end(Update::Failed(MALFORMED_OUTPUT)),
            Ok(Line::Init(session)) => {
                if self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                self.started = true;
                let conversation = match self.conversation_id.clone() {
                    // A rebuild replaces the stale session behind the same
                    // conversation ID, so the extension sees no new one.
                    Some(id) if self.rebuilding => {
                        if self.remember(&id, session).is_err() {
                            return self.end(Update::Failed(SESSION_STORE_FAILED));
                        }
                        id
                    }
                    Some(id) => id,
                    None => {
                        let id = new_conversation_id(&self.conversations.borrow());
                        if self.remember(&id, session).is_err() {
                            return self.end(Update::Failed(SESSION_STORE_FAILED));
                        }
                        self.conversation_id = Some(id.clone());
                        self.queue
                            .push_back(Update::ConversationCreated(id.clone()));
                        id
                    }
                };
                if !self.announced {
                    self.announced = true;
                    self.queue.push_back(Update::Started {
                        conversation_id: Some(conversation),
                    });
                }
            }
            Ok(Line::MessageStart) => {
                if !self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                if self.messages > 0 {
                    self.break_before_text = true;
                }
                self.messages += 1;
            }
            Ok(Line::TextDelta(mut text)) => {
                if !self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                if !text.is_empty() {
                    if self.messages == 0 {
                        self.messages = 1;
                    }
                    if self.break_before_text && self.saw_delta {
                        text.insert_str(0, "\n\n");
                    }
                    self.break_before_text = false;
                    self.saw_delta = true;
                    self.queue.push_back(Update::Delta(text));
                }
            }
            Ok(Line::Progress) => self.queue.push_back(Update::Activity),
            Ok(Line::ResultSuccess { session_id, text }) => {
                if !self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                if let (Some(session), Some(conversation)) =
                    (session_id, self.conversation_id.clone())
                {
                    self.follow_session(&conversation, session);
                }
                if !self.saw_delta && !text.is_empty() {
                    self.saw_delta = true;
                    self.queue.push_back(Update::Delta(text));
                }
                self.turn_ended(Ok(()));
            }
            Ok(Line::ResultFailed(error)) => self.turn_ended(Err(error)),
            Ok(Line::Ignored) => {}
        }
    }

    /// Records `session` for `conversation`: on disk when there is a session
    /// directory, and in memory.
    fn remember(&self, conversation: &str, session: String) -> io::Result<()> {
        if let Some(dir) = &self.session_dir {
            save_session(dir, conversation, &session)?;
        }
        self.conversations
            .borrow_mut()
            .insert(conversation.to_owned(), session);
        Ok(())
    }

    /// Follows the session a finished turn reports, which a resumed or forked
    /// session may have changed. The answer is already out, so a failed write
    /// doesn't fail the turn: the new session is kept in memory, and the stale
    /// mapping is dropped so a restarted host rebuilds from history instead of
    /// resuming the wrong session.
    fn follow_session(&self, conversation: &str, session: String) {
        let current = self
            .conversations
            .borrow()
            .get(conversation)
            .cloned()
            .or_else(|| self.resume.clone());
        if current.as_deref() == Some(session.as_str()) {
            return;
        }
        if self.remember(conversation, session.clone()).is_err() {
            if let Some(dir) = &self.session_dir {
                let _ = forget_session(dir, conversation);
            }
            self.conversations
                .borrow_mut()
                .insert(conversation.to_owned(), session);
        }
    }

    /// Drops a mapping Claude says no longer exists. Best effort: a mapping
    /// that can't be removed is replaced by the rebuild's own.
    fn drop_stale_mapping(&self, conversation: &str) {
        self.conversations.borrow_mut().remove(conversation);
        if let Some(dir) = &self.session_dir {
            let _ = forget_session(dir, conversation);
        }
    }

    fn turn_ended(&mut self, outcome: Result<(), ErrorBody<'static>>) {
        self.outcome = Some(outcome);
        self.finish_by = Some(after(self.finish_grace));
    }

    /// Reruns the turn from bounded history under the same conversation ID,
    /// once, after Claude said the mapped session no longer exists.
    fn rebuild_from_history(&mut self) {
        self.resume = None;
        self.rebuilding = true;
        self.outcome = None;
        self.finish_by = None;
        self.started = false;
        self.saw_delta = false;
        self.messages = 0;
        self.break_before_text = false;
        self.prompt = self
            .fallback_prompt
            .take()
            .expect("checked before fallback");
        self.start();
    }

    /// Claude exited. `session_gone` says it reported, on stderr before `init`,
    /// that the session it was asked to resume doesn't exist.
    fn ended(&mut self, exit: &Exit, session_gone: bool) {
        if self.cancelled {
            return self.end(Update::Stopped);
        }
        let stale = self.resume.is_some()
            && !self.saw_delta
            && match self.outcome {
                Some(Err(error)) => error.reason == "UNKNOWN_CONVERSATION",
                Some(Ok(())) => false,
                None => session_gone && exit.status.is_some_and(|status| !status.success()),
            };
        if !stale {
            let update = self.exited(exit);
            return self.end(update);
        }
        if let Some(conversation) = self.conversation_id.clone() {
            self.drop_stale_mapping(&conversation);
        }
        if self.fallback_prompt.is_some() {
            self.rebuild_from_history();
        } else {
            self.end(Update::Failed(SESSION_GONE));
        }
    }

    fn exited(&mut self, exit: &Exit) -> Update {
        match self.outcome.take() {
            Some(Ok(())) => Update::Completed,
            Some(Err(error)) => Update::Failed(error),
            None if exit.status.is_some_and(|status| status.success()) => {
                Update::Failed(MALFORMED_OUTPUT)
            }
            None => Update::Failed(PROCESS_EXITED),
        }
    }
}

impl Exchange for Turn {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        let busy_until = deadline.max(after(BUSY_LIMIT));
        loop {
            if let Some(update) = self.queue.pop_front() {
                return Some(update);
            }
            if Instant::now() >= busy_until {
                return None;
            }
            match &mut self.stage {
                Stage::Done => return None,
                Stage::Probing { process, give_up } if Instant::now() >= *give_up => {
                    process.kill();
                    self.start();
                }
                Stage::Probing { process, give_up } => {
                    match process.next_event(deadline.min(*give_up)) {
                        Some(Event::Exited(exit)) => match signed_in(&exit) {
                            Authentication::Unauthenticated => {
                                self.end(Update::Failed(NOT_SIGNED_IN));
                            }
                            _ => self.start(),
                        },
                        Some(Event::Stdout(_) | Event::Stderr(_)) => {}
                        None if Instant::now() >= *give_up => {}
                        None => return None,
                    }
                }
                Stage::Running(stream) => {
                    // Checked first, so output that keeps coming after the
                    // turn ended can't put it off.
                    if self
                        .finish_by
                        .is_some_and(|finish_by| Instant::now() >= finish_by)
                    {
                        self.finish_by = None;
                        stream.cancel(Duration::ZERO);
                    }
                    let wait = self
                        .finish_by
                        .map_or(deadline, |finish_by| deadline.min(finish_by));
                    match stream.next(wait) {
                        Some(Output::Line(line)) => self.on_line(&line),
                        Some(Output::Final(exit) | Output::Stopped(exit)) => {
                            let session_gone = !self.started
                                && output::names_unknown_session(&String::from_utf8_lossy(
                                    stream.stderr_tail(),
                                ));
                            self.ended(&exit, session_gone);
                        }
                        Some(Output::Error(_)) => {
                            let update = if self.cancelled {
                                Update::Stopped
                            } else {
                                Update::Failed(MALFORMED_OUTPUT)
                            };
                            self.end(update);
                        }
                        None if self
                            .finish_by
                            .is_some_and(|finish_by| Instant::now() >= finish_by) => {}
                        None => return None,
                    }
                }
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        if self.cancelled {
            return;
        }
        self.cancelled = true;
        let finished = self.queue.iter().any(Update::is_terminal);
        self.queue.clear();
        match &mut self.stage {
            Stage::Running(stream) => stream.cancel(grace),
            Stage::Probing { .. } => self.end(Update::Stopped),
            Stage::Done if finished => self.queue.push_back(Update::Stopped),
            Stage::Done => {}
        }
    }
}
