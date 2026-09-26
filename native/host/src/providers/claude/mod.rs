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
use super::discovery::SearchPath;
use super::environment;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::provider_prompt;
use crate::process::{Event, Exit, Process, ProcessSpec};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "claude";
const EXECUTABLE: &str = "claude";

/// Non-secret Claude configuration needed to find the user's normal CLI state.
pub const CLAUDE_VARIABLES: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_GIT_BASH_PATH",
    "NODE_EXTRA_CA_CERTS",
];

pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub timeouts: Timeouts,
    pub probe: Duration,
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
            // Do not update itself while Pervue owns the lifecycle.
            .env("DISABLE_AUTOUPDATER", "1")
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            .current_dir(workspace)
    }
}

pub struct Claude {
    search: SearchPath,
    launch: Rc<Launch>,
    session_dir: Option<PathBuf>,
    limits: Limits,
    conversations: Conversations,
}

impl Claude {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        let mut claude = Self::new(
            SearchPath::from_env(),
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
            Some(id) => match self
                .conversations
                .borrow()
                .get(id)
                .cloned()
                .or_else(|| {
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
            saw_delta: false,
            result_seen: false,
            messages: 0,
            pending_separator: false,
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

/// Current Claude Code exposes a machine-readable authentication probe.
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
                        // auth status can contain account details; discard it.
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
    prompt: String,
    resume: Option<String>,
    conversation_id: Option<String>,
    conversations: Conversations,
    queue: VecDeque<Update>,
    cancelled: bool,
    started: bool,
    saw_delta: bool,
    result_seen: bool,
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
            "plan",
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
                self.stage = Stage::Running(LineStream::new(process, MAX_LINE_BYTES));
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
        if self.cancelled {
            return;
        }
        match output::parse(line) {
            Err(_) => self.end(Update::Failed(MALFORMED_OUTPUT)),
            Ok(Line::Init(session)) => {
                if self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                self.started = true;
                let conversation = match &self.conversation_id {
                    Some(id) => id.clone(),
                    None => {
                        let id = new_conversation_id(&self.conversations.borrow());
                        self.conversations.borrow_mut().insert(id.clone(), session);
                        self.conversation_id = Some(id.clone());
                        self.queue.push_back(Update::ConversationCreated(id.clone()));
                        id
                    }
                };
                self.queue.push_back(Update::Started {
                    conversation_id: Some(conversation),
                });
            }
            Ok(Line::TextDelta(text)) => {
                if !self.started || self.result_seen {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                if !text.is_empty() {
                    self.saw_delta = true;
                    self.queue.push_back(Update::Delta(text));
                }
            }
            Ok(Line::Progress) => self.queue.push_back(Update::Activity),
            Ok(Line::ResultSuccess { session_id, text }) => {
                if !self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                // On resume, Claude may report an invocation-local init ID.
                // The session originally supplied to --resume remains canonical.
                if self.resume.is_none() {
                    if let (Some(id), Some(conversation)) =
                        (session_id, self.conversation_id.as_ref())
                    {
                        self.conversations.borrow_mut().insert(conversation.clone(), id);
                    }
                }
                if !self.saw_delta && !text.is_empty() {
                    self.saw_delta = true;
                    self.queue.push_back(Update::Delta(text));
                }
                self.result_seen = true;
            }
            Ok(Line::ResultFailed(error)) => self.end(Update::Failed(error)),
            Ok(Line::Ignored) => {}
        }
    }

    fn final_update(result_seen: bool, exit: &Exit) -> Update {
        if result_seen {
            Update::Completed
        } else if exit.status.is_some_and(|status| status.success()) {
            Update::Failed(MALFORMED_OUTPUT)
        } else {
            Update::Failed(PROCESS_EXITED)
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
                Stage::Probing { process, give_up } => {
                    let authentication = if Instant::now() >= *give_up {
                        process.kill();
                        Authentication::Unknown
                    } else {
                        match process.next_event(deadline.min(*give_up)) {
                            Some(Event::Exited(exit)) => signed_in(&exit),
                            Some(Event::Stdout(_) | Event::Stderr(_)) => continue,
                            None if Instant::now() >= *give_up => continue,
                            None => return None,
                        }
                    };
                    if authentication == Authentication::Unauthenticated {
                        self.end(Update::Failed(NOT_SIGNED_IN));
                    } else {
                        self.start();
                    }
                }
                Stage::Running(stream) => match stream.next(deadline)? {
                    Output::Line(line) => self.on_line(&line),
                    Output::Final(exit) => {
                        let update = Self::final_update(self.result_seen, &exit);
                        self.end(update);
                    }
                    Output::Error(_) => self.end(Update::Failed(MALFORMED_OUTPUT)),
                    Output::Stopped(_) => self.end(Update::Stopped),
                },
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        if matches!(self.stage, Stage::Done) {
            return;
        }
        self.cancelled = true;
        self.queue.clear();
        match &mut self.stage {
            Stage::Probing { .. } => self.end(Update::Stopped),
            Stage::Running(stream) => stream.cancel(grace),
            Stage::Done => {}
        }
    }
}
