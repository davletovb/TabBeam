//! The Codex CLI adapter: discovery and sign-in status (PRO-02), requests and
//! streaming (PRO-03), and cancellation, timeouts, and failures (PRO-04).
//!
//! A request runs `codex exec --json` in a read-only sandbox, from an empty
//! working directory, with the question on stdin. Its JSON lines become
//! protocol updates (see [`output`]). Codex's own session IDs stay inside the
//! adapter: each conversation gets a Pervue ID, mapped to the Codex thread it
//! continues with `codex exec resume`. The provider-specific mapping is kept
//! in private native files, so a new host can resume a stored conversation.
//!
//! Before each request, `codex login status` checks the sign-in, because a
//! signed-out `codex exec` retries the network instead of failing. Only its
//! exit status is read: its output names the account and a masked key.
//!
//! Codex runs in a workspace nobody but its user can change ([`workspace`]),
//! with a minimal environment (SEC-02): the variables every provider gets
//! ([`environment::INHERITED`]), Codex's own settings, and a `PATH` that
//! starts with Codex's directory.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::discovery::SearchPath;
use super::environment;
use super::forget;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::provider_prompt;
use crate::process::{Event, Exit, Process, ProcessSpec};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;
pub(crate) mod workspace;

use output::Line;

pub const ID: &str = "codex";

/// The executable the adapter looks for.
const EXECUTABLE: &str = "codex";

/// Codex's own settings, passed on when set: where it keeps its settings,
/// sign-in, and state, and an extra CA certificate for its connections.
pub const CODEX_VARIABLES: &[&str] = &["CODEX_HOME", "CODEX_SQLITE_HOME", "CODEX_CA_CERTIFICATE"];

/// Longest line of Codex output. One line holds a whole answer, which can be
/// long; a line past this ends the request instead of growing memory.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// The adapter's time limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The host's limits for each request.
    pub timeouts: Timeouts,
    /// How long `codex login status` may take before the sign-in counts as
    /// unknown.
    pub probe: Duration,
    /// How long Codex gets to save its session and exit after the turn ended.
    pub finish: Duration,
}

/// Codex answers without token deltas, so a long silence can be a model
/// thinking; five minutes without any progress means it is stuck.
pub const LIMITS: Limits = Limits {
    timeouts: Timeouts {
        start: Duration::from_secs(60),
        idle: Duration::from_secs(300),
        stop_grace: Duration::from_secs(2),
    },
    probe: Duration::from_secs(10),
    finish: Duration::from_secs(5),
};

/// What this adapter supports. Answers arrive a message at a time, not token
/// by token. Browser context is framed as untrusted reference data in the
/// prompt; attachments and model selection are not passed to Codex yet.
pub const CAPABILITIES: Capabilities = Capabilities {
    streaming: Capability::Supported,
    continuation: Capability::Supported,
    web_search: Capability::Unknown,
    page_context: Capability::Supported,
    attachments: Capability::Unsupported,
    model_selection: Capability::Unsupported,
    cancellation: Capability::Supported,
};

const NOT_INSTALLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotFound,
    reason: "EXECUTABLE_NOT_FOUND",
    message: "Codex isn't installed. Install the Codex CLI, then try again.",
    retryable: false,
};

const NOT_SIGNED_IN: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "LOGIN_REQUIRED",
    message: "Codex isn't signed in. Run \"codex login\" in a terminal, then try again.",
    retryable: false,
};

const CONTEXT_TOOLS_ENABLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "PAGE_CONTEXT_TOOLS_ENABLED",
    message: "Pervue won't send browser context to Codex while user-configured MCP servers are enabled. Disable them or choose No context.",
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
    message: "Codex couldn't start. Reinstall the Codex CLI, then try again.",
    retryable: false,
};

const NO_WORKSPACE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_UNAVAILABLE",
    message: "Pervue couldn't prepare a private folder for Codex. Make sure your cache folder exists and only you can change it, then try again.",
    retryable: false,
};

const PROCESS_EXITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROCESS_EXITED",
    message: "Codex stopped unexpectedly. Try again.",
    retryable: true,
};

const MALFORMED_OUTPUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MALFORMED_PROVIDER_OUTPUT",
    message: "Codex answered in a way Pervue doesn't understand. Update Codex and Pervue, then try again.",
    retryable: false,
};

const SESSION_STORE_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InternalError,
    reason: "SESSION_STORE_FAILED",
    message: "Codex's conversation could not be saved. Check available disk space and try again.",
    retryable: true,
};

/// Pervue conversation IDs mapped to the Codex threads they continue.
type Conversations = Rc<RefCell<HashMap<String, String>>>;

/// How Codex processes start: where they run, and what environment they get.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Launch {
    work_dir: PathBuf,
    /// Variables copied from the host's environment.
    inherited: Vec<(OsString, OsString)>,
    /// The host's `PATH`.
    path: Option<OsString>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        let path = environment::lookup(&host, "PATH").map(OsStr::to_os_string);
        Self {
            work_dir,
            inherited: environment::inherit(host, CODEX_VARIABLES),
            path,
        }
    }

    /// The workspace, created if needed and checked before every launch:
    /// the path Codex runs in and is pointed at.
    fn workspace(&self) -> std::io::Result<PathBuf> {
        workspace::prepare(&self.work_dir)
    }

    /// `codex` with `args`, in `workspace`, with only its environment.
    fn command<I>(&self, workspace: &Path, executable: &Path, args: I) -> ProcessSpec
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            .current_dir(workspace)
    }
}

/// The Codex CLI adapter.
pub struct Codex {
    search: SearchPath,
    launch: Rc<Launch>,
    session_dir: Option<PathBuf>,
    limits: Limits,
    conversations: Conversations,
}

impl Codex {
    /// The adapter of an installed host: the platform lookup rules, an empty
    /// workspace in the user's own cache directory, and conversation mappings
    /// in the user's data directory.
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        let mut codex = Self::new(
            SearchPath::from_env(crate::providers::discovery::SEARCH_PATH_VARIABLE),
            workspace::default(&host),
        );
        codex.session_dir = installed_session_dir();
        codex
    }

    /// Looks for `codex` in `search`, and runs it in `work_dir`, which it
    /// creates when needed and refuses if other users could change it, with
    /// variables from the host's environment. Conversation mappings go
    /// beside `work_dir`, never inside it, in `<work_dir>.sessions`.
    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        Self {
            search,
            session_dir: Some(work_dir.with_extension("sessions")),
            launch: Rc::new(Launch::new(work_dir, std::env::vars_os().collect())),
            limits: LIMITS,
            conversations: Rc::default(),
        }
    }

    /// Takes the variables Codex gets from `host` instead of the host's own
    /// environment.
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

    /// Replaces the default time limits.
    #[must_use]
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Overrides the native mapping directory, for isolated host tests.
    #[must_use]
    pub fn with_session_dir(mut self, session_dir: PathBuf) -> Self {
        self.session_dir = Some(session_dir);
        self
    }

    fn executable(&self) -> Option<PathBuf> {
        self.search.find(EXECUTABLE)
    }
}

impl Provider for Codex {
    fn id(&self) -> &str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    fn timeouts(&self) -> Timeouts {
        self.limits.timeouts
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
        let thread = self
            .conversations
            .borrow()
            .get(conversation_id)
            .cloned()
            .or_else(|| {
                self.session_dir
                    .as_deref()
                    .and_then(|dir| read_thread(dir, conversation_id))
            });
        let home = codex_home(&self.launch);
        let workspace = self.launch.work_dir.clone();
        let session_dir = self.session_dir.clone();
        let conversation = conversation_id.to_owned();
        let conversations = Rc::clone(&self.conversations);
        let forgotten = conversation_id.to_owned();
        // The files go on their own thread. The mappings go last, the one in
        // memory only once everything else is gone: if Codex's files can't
        // all be removed, a retry can still find them.
        forget::in_background(
            move || {
                if let (Some(home), Some(thread)) = (home, thread) {
                    forget_rollouts(&home, &workspace, &thread)?;
                }
                session_dir.map_or(Ok(()), |dir| forget_thread(&dir, &conversation))
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
        let context_turn = request.context.is_some();
        if context_turn && !context_configuration_is_safe(&self.launch) {
            return Box::new(Scripted::failed(CONTEXT_TOOLS_ENABLED));
        }

        let mut conversation_id = request.conversation_id;
        let mut fallback_prompt = (!request.history.is_empty())
            .then(|| provider_prompt(&request.history, request.context.as_ref(), &request.text));
        let mut prompt = match request.context.as_ref() {
            Some(context) => provider_prompt(&[], Some(context), &request.text),
            None => request.text,
        };
        let resume = match &conversation_id {
            None => None,
            Some(conversation_id) => match self
                .conversations
                .borrow()
                .get(conversation_id)
                .cloned()
                .or_else(|| {
                    self.session_dir
                        .as_deref()
                        .and_then(|dir| read_thread(dir, conversation_id))
                }) {
                Some(thread_id) => Some(thread_id),
                None if !request.history.is_empty() => None,
                None => return Box::new(Scripted::failed(UNKNOWN_CONVERSATION)),
            },
        };
        if resume.is_none() && !request.history.is_empty() {
            // If a provider has no native session (or its mapping was lost),
            // the ordered, bounded dialogue still reaches the new turn.
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
            restrict_tools: context_turn,
            queue: VecDeque::new(),
            cancelled: false,
            thread_id: None,
            started: false,
            messages: 0,
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
            // The sign-in can't be checked; the request itself will tell.
            Err(_) => turn.start(),
        }
        Box::new(turn)
    }
}

/// Codex's own directory: `CODEX_HOME`, or `.codex` in the user's home.
fn codex_home(launch: &Launch) -> Option<PathBuf> {
    environment::lookup(&launch.inherited, "CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| environment::home_dir(&launch.inherited).map(|home| home.join(".codex")))
}

fn context_configuration_is_safe(launch: &Launch) -> bool {
    let home = codex_home(launch);
    let Some(home) = home else {
        return true;
    };

    // Plugin caches/install directories and hook files are safe to leave in
    // place because context turns explicitly disable those Codex features at
    // invocation time. User-configured MCP servers are different: Codex
    // exposes them independently of the plugin/apps feature gates, so fail
    // closed until Pervue can disable each effective server deterministically.
    let mut configs = vec![home.join("config.toml")];
    if home.exists() {
        let Ok(entries) = std::fs::read_dir(&home) else {
            return false;
        };
        configs.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(OsStr::to_str)
                        .is_some_and(|name| name.ends_with(".config.toml"))
                }),
        );
    }
    configs.into_iter().all(|path| {
        if !path.exists() {
            return true;
        }
        let Ok(config) = std::fs::read_to_string(path) else {
            return false;
        };
        !config.to_ascii_lowercase().contains("mcp_servers")
    })
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

/// `codex login status`: exit status 0 means signed in, 1 signed out.
fn probe(launch: &Launch, executable: &Path) -> std::io::Result<Process> {
    let workspace = launch.workspace()?;
    let mut process = Process::spawn(&launch.command(&workspace, executable, ["login", "status"]))?;
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

/// The host's `PATH` with the executable's own directory first. npm installs
/// `codex` as a Node script next to `node`, and a host started by Chrome
/// inherits a `PATH` that may not include that directory.
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
        .map(|path| path.join("pervue/codex-sessions"))
}

fn session_name(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("conv_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn read_thread(dir: &Path, id: &str) -> Option<String> {
    if !session_name(id) {
        return None;
    }
    let mut content = String::new();
    std::fs::File::open(dir.join(id))
        .ok()?
        .take(129)
        .read_to_string(&mut content)
        .ok()?;
    output::is_thread_id(&content).then_some(content)
}

/// Removes a conversation's thread mapping.
fn forget_thread(dir: &Path, id: &str) -> io::Result<()> {
    if session_name(id) {
        forget::remove(&dir.join(id))
    } else {
        Ok(())
    }
}

/// Removes Codex's saved sessions of `thread` that Codex wrote for Pervue:
/// `sessions/YYYY/MM/DD/rollout-…-<thread>.jsonl` and
/// `archived_sessions/rollout-…-<thread>.jsonl` files whose `session_meta`
/// names the thread and records Pervue's workspace as where it ran. Codex's
/// own state database is left alone.
fn forget_rollouts(home: &Path, workspace: &Path, thread: &str) -> io::Result<()> {
    let suffix = format!("-{thread}.jsonl");
    // Each directory with how many levels of directories may lie below it.
    let mut pending = vec![
        (home.join("sessions"), 3),
        (home.join("archived_sessions"), 0),
    ];
    while let Some((dir, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            entries => entries?,
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                if depth > 0 {
                    pending.push((path, depth - 1));
                }
            } else if kind.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&suffix))
                && rollout_written_for_pervue(&path, thread, workspace)
            {
                forget::remove(&path)?;
            }
        }
    }
    Ok(())
}

fn rollout_written_for_pervue(path: &Path, thread: &str, workspace: &Path) -> bool {
    forget::head_lines(path).is_some_and(|lines| {
        lines
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|record| {
                record.get("type").and_then(serde_json::Value::as_str) == Some("session_meta")
            })
            .and_then(|record| record.get("payload").cloned())
            .is_some_and(|meta| {
                meta.get("id").and_then(serde_json::Value::as_str) == Some(thread)
                    && meta
                        .get("cwd")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|cwd| forget::same_directory(cwd, workspace))
            })
    })
}

fn save_thread(dir: &Path, id: &str, thread: &str) -> io::Result<()> {
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
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    if let Err(error) = file
        .write_all(thread.as_bytes())
        .and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// A new Pervue conversation ID. It is random so it reveals nothing about the
/// Codex thread behind it.
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

/// The `provider.status` check.
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
                // Checked first, so output that keeps coming can't put it off.
                Self::Probing { process, give_up } if Instant::now() >= *give_up => {
                    process.kill();
                    Authentication::Unknown
                }
                Self::Probing { process, give_up } => {
                    match process.next_event(deadline.min(*give_up)) {
                        Some(Event::Exited(exit)) => signed_in(&exit),
                        // The probe's output names the account: never read.
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

/// One `conversation.send`: the sign-in probe, then the `codex exec` turn.
struct Turn {
    stage: Stage,
    executable: PathBuf,
    launch: Rc<Launch>,
    session_dir: Option<PathBuf>,
    prompt: String,
    /// Used once if a mapped Codex thread no longer exists before the turn starts.
    fallback_prompt: Option<String>,
    /// The Codex thread to resume, when continuing a conversation.
    resume: Option<String>,
    conversation_id: Option<String>,
    conversations: Conversations,
    finish_grace: Duration,
    /// Browser context is untrusted, so context turns run with Codex's
    /// interactive/tool surfaces disabled.
    restrict_tools: bool,
    /// Updates produced but not yet returned.
    queue: VecDeque<Update>,
    cancelled: bool,
    thread_id: Option<String>,
    started: bool,
    messages: usize,
    /// How the turn ended, once Codex said so.
    outcome: Option<Result<(), ErrorBody<'static>>>,
    /// When to stop waiting for Codex to exit after the turn ended.
    finish_by: Option<Instant>,
}

enum Stage {
    Probing {
        process: Process,
        give_up: Instant,
    },
    Running(LineStream),
    /// Finished: any process has been dropped, which reaps it.
    Done,
}

impl Turn {
    /// Starts `codex exec` and hands it the question.
    fn start(&mut self) {
        let Ok(workspace) = self.launch.workspace() else {
            return self.end(Update::Failed(NO_WORKSPACE));
        };
        let mut args: Vec<OsString> = [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
        ]
        .map(OsString::from)
        .into();
        if self.restrict_tools {
            // Page text is attacker-controlled. A context turn is deliberately
            // answer-only: no local shell/image tools, apps/plugins/hooks,
            // web search, orchestrator MCP, or subagents. User-configured MCP
            // servers are refused before this point because they are not all
            // controlled by those feature gates.
            for setting in [
                "features.shell_tool=false",
                "features.view_image=false",
                "features.apps=false",
                "features.plugins=false",
                "features.hooks=false",
                "features.multi_agent=false",
                "features.multi_agent_v2=false",
                "features.web_search_request=false",
                "features.web_search_cached=false",
                "features.standalone_web_search=false",
                "web_search=\"disabled\"",
                "orchestrator.mcp.enabled=false",
            ] {
                args.extend(["-c".into(), setting.into()]);
            }
        }
        args.push("-C".into());
        args.push(workspace.clone().into());
        if let Some(thread_id) = &self.resume {
            args.extend(["resume", thread_id].map(OsString::from));
        }
        args.push("-".into());
        match Process::spawn(&self.launch.command(&workspace, &self.executable, args)) {
            Ok(mut process) => {
                // The question goes on stdin: it can exceed the size one
                // argument may have, and no shell ever sees it.
                let _ = process.write(std::mem::take(&mut self.prompt).as_bytes());
                process.close_stdin();
                self.stage = Stage::Running(LineStream::new(process, MAX_LINE_BYTES));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.end(Update::Failed(NOT_INSTALLED))
            }
            Err(_) => self.end(Update::Failed(START_FAILED)),
        }
    }

    /// Queues the terminal update and drops any process, which reaps it.
    fn end(&mut self, update: Update) {
        self.queue.push_back(update);
        self.stage = Stage::Done;
    }

    /// Acts on one line of Codex output.
    fn on_line(&mut self, line: &str) {
        if self.cancelled || self.outcome.is_some() {
            return;
        }
        match output::parse(line) {
            Err(_) => self.end(Update::Failed(MALFORMED_OUTPUT)),
            Ok(Line::ThreadStarted(thread_id)) => self.thread_id = Some(thread_id),
            Ok(Line::TurnStarted) => self.turn_started(),
            Ok(Line::AgentMessage(text)) => {
                if !self.started {
                    return self.end(Update::Failed(MALFORMED_OUTPUT));
                }
                if !text.is_empty() {
                    // Later messages continue the answer after a blank line.
                    let text = if self.messages == 0 {
                        text
                    } else {
                        format!("\n\n{text}")
                    };
                    self.messages += 1;
                    self.queue.push_back(Update::Delta(text));
                }
            }
            Ok(Line::Progress) => self.queue.push_back(Update::Activity),
            Ok(Line::TurnCompleted) if !self.started => self.end(Update::Failed(MALFORMED_OUTPUT)),
            Ok(Line::TurnCompleted) => self.turn_ended(Ok(())),
            Ok(Line::TurnFailed(message)) => self.turn_ended(Err(output::turn_failure(&message))),
            Ok(Line::Ignored) => {}
        }
    }

    fn turn_started(&mut self) {
        if self.started {
            return;
        }
        let Some(thread_id) = self.thread_id.clone() else {
            return self.end(Update::Failed(MALFORMED_OUTPUT));
        };
        self.started = true;
        let conversation_id = match &self.conversation_id {
            Some(conversation_id) => conversation_id.clone(),
            None => {
                let conversation_id = new_conversation_id(&self.conversations.borrow());
                if self
                    .session_dir
                    .as_deref()
                    .is_none_or(|dir| save_thread(dir, &conversation_id, &thread_id).is_err())
                {
                    return self.end(Update::Failed(SESSION_STORE_FAILED));
                }
                self.conversations
                    .borrow_mut()
                    .insert(conversation_id.clone(), thread_id);
                self.queue
                    .push_back(Update::ConversationCreated(conversation_id.clone()));
                conversation_id
            }
        };
        self.queue.push_back(Update::Started {
            conversation_id: Some(conversation_id),
        });
    }

    fn turn_ended(&mut self, outcome: Result<(), ErrorBody<'static>>) {
        self.outcome = Some(outcome);
        self.finish_by = Some(after(self.finish_grace));
    }

    /// The terminal update once the process has exited.
    fn exited(&mut self, exit: &Exit) -> Update {
        match self.outcome.take() {
            Some(Ok(())) => Update::Completed,
            Some(Err(error)) => Update::Failed(error),
            // The process ended without finishing the turn.
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
            // Output that keeps coming without an update, such as lines the
            // adapter ignores, gives the caller its turn back.
            if Instant::now() >= busy_until {
                return None;
            }
            match &mut self.stage {
                Stage::Done => return None,
                // Checked first, so output that keeps coming can't put it off.
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
                        // The probe's output names the account: never read.
                        Some(Event::Stdout(_) | Event::Stderr(_)) => {}
                        // Its time is up: the arm above stops it.
                        None if Instant::now() >= *give_up => {}
                        None => return None,
                    }
                }
                Stage::Running(stream) => {
                    let wait = self
                        .finish_by
                        .map_or(deadline, |finish_by| deadline.min(finish_by));
                    match stream.next(wait) {
                        Some(Output::Line(line)) => self.on_line(&line),
                        Some(Output::Final(_))
                            if !self.cancelled
                                && !self.started
                                && self.resume.is_some()
                                && self.fallback_prompt.is_some() =>
                        {
                            self.resume = None;
                            self.conversation_id = None;
                            self.thread_id = None;
                            self.outcome = None;
                            self.finish_by = None;
                            self.prompt = self.fallback_prompt.take().expect("checked above");
                            self.start();
                        }
                        Some(Output::Final(exit) | Output::Stopped(exit)) => {
                            let update = if self.cancelled {
                                Update::Stopped
                            } else {
                                self.exited(&exit)
                            };
                            self.end(update);
                        }
                        Some(Output::Error(_)) => {
                            let update = if self.cancelled {
                                Update::Stopped
                            } else {
                                Update::Failed(MALFORMED_OUTPUT)
                            };
                            self.end(update);
                        }
                        None => match self.finish_by {
                            // The turn is over but Codex hasn't exited.
                            Some(finish_by) if Instant::now() >= finish_by => {
                                self.finish_by = None;
                                stream.cancel(Duration::ZERO);
                            }
                            _ => return None,
                        },
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
            // The probe is harmless to kill, and nothing has started.
            Stage::Probing { .. } => self.end(Update::Stopped),
            Stage::Done if finished => self.queue.push_back(Update::Stopped),
            Stage::Done => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_s_directory_comes_first_on_its_path() {
        let (codex, inherited) = if cfg!(unix) {
            ("/opt/codex/bin/codex", "/usr/bin:/bin")
        } else {
            (r"C:\npm\codex.cmd", r"C:\Windows;C:\bin")
        };
        let path = search_path_for(Path::new(codex), Some(OsStr::new(inherited)));
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs[0], Path::new(codex).parent().unwrap());
        assert_eq!(dirs.len(), 3);
        let alone = search_path_for(Path::new(codex), None);
        assert_eq!(
            std::env::split_paths(&alone).collect::<Vec<_>>(),
            [Path::new(codex).parent().unwrap()]
        );
    }
}
