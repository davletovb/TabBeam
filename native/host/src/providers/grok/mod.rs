//! xAI Grok adapter through one-shot Grok Build headless mode (PRO-09).
//!
//! Pervue intentionally does not run Grok as a persistent ACP/app server.
//! Every turn gets a fresh headless `grok` process, a private working
//! directory and GROK_HOME, and the protocol's bounded history. Only the
//! existing Grok/X OAuth file is referenced outside that directory.
//!
//! Ordinary turns expose no tools. Web turns expose only `web_search`.
//! The headless stream's init event is verified before any answer text is
//! forwarded, and the private workspace (including Grok's session files and
//! the prompt file) is removed after the child has exited.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::codex::workspace;
use super::discovery;
use super::environment;
use super::forget;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::{SEARCH_INSTRUCTIONS, provider_prompt};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::search::{NATIVE_SEARCH_NO_SOURCES, SourceCollector};
use pervue_core::discovery::SearchPath;
use pervue_core::process::{Event, Exit, Process, ProcessSpec};
use pervue_core::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "grok";
const EXECUTABLE: &str = "grok";
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;
const STATUS_OUTPUT_BYTES: usize = 16 * 1024;
const STATUS_PROBE: Duration = Duration::from_secs(10);
const FINISH_GRACE: Duration = Duration::from_secs(5);

pub const CAPABILITIES: Capabilities = Capabilities {
    streaming: Capability::Supported,
    continuation: Capability::Supported,
    web_search: Capability::Supported,
    page_context: Capability::Supported,
    attachments: Capability::Unsupported,
    model_selection: Capability::Supported,
    cancellation: Capability::Supported,
};

pub const TIMEOUTS: Timeouts = Timeouts {
    start: Duration::from_secs(60),
    idle: Duration::from_secs(300),
    stop_grace: Duration::from_secs(2),
};

const NOT_INSTALLED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotFound,
    reason: "EXECUTABLE_NOT_FOUND",
    message: "Grok Build isn't installed. Install the Grok CLI, then try again.",
    retryable: false,
};
const START_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Grok Build couldn't start. Reinstall it, then try again.",
    retryable: false,
};
const NO_WORKSPACE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_UNAVAILABLE",
    message: "Pervue couldn't prepare a private folder for Grok. Check your cache folder, then try again.",
    retryable: false,
};
const PROCESS_EXITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROCESS_EXITED",
    message: "Grok stopped unexpectedly. Try again.",
    retryable: true,
};
const MALFORMED_OUTPUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MALFORMED_PROVIDER_OUTPUT",
    message: "Grok answered in a way Pervue doesn't understand. Update Grok Build and Pervue, then try again.",
    retryable: false,
};
const BOUNDARY_VIOLATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_BOUNDARY_VIOLATION",
    message: "Grok exposed or used a tool Pervue doesn't allow, so the turn was stopped.",
    retryable: false,
};
const AUTH_MODE_REJECTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "AUTH_REJECTED",
    message: "Pervue uses Grok/X account sign-in, not API-key billing. Run grok login, then try again.",
    retryable: false,
};
const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "This Grok conversation ID is not one Pervue issued. Start a new conversation.",
    retryable: false,
};
const MODEL_NOT_SUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "MODEL_NOT_SUPPORTED",
    message: "The Grok provider accepts Grok model IDs only.",
    retryable: false,
};

const PLAIN_AGENT: &str = r#"---
name: pervue-text
description: Text-only Pervue Grok responder.
promptMode: full
tools: []
discoverSkills: false
inheritSkills: false
agentsMd: false
disallowedTools:
  - Agent
mcpInheritance: none
permissionMode: dontAsk
---
Answer the user's request directly as text. Do not use tools, files, commands, MCP servers, skills, plugins, hooks, subagents, memory, or external side effects.
"#;

const SEARCH_AGENT: &str = r#"---
name: pervue-search
description: Pervue Grok responder allowed to use only native web search.
promptMode: full
tools:
  - web_search
discoverSkills: false
inheritSkills: false
agentsMd: false
disallowedTools:
  - Agent
mcpInheritance: none
permissionMode: dontAsk
---
Use web_search when answering. Do not use files, commands, web_fetch, MCP servers, skills, plugins, hooks, subagents, memory, or any other external side effect.
"#;

#[derive(Debug, Clone)]
struct Launch {
    work_dir: PathBuf,
    inherited: Vec<(OsString, OsString)>,
    path: Option<OsString>,
    auth_path: Option<PathBuf>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        let auth_path = environment::home_dir(&host).map(|home| home.join(".grok/auth.json"));
        Self {
            work_dir,
            inherited: environment::inherit(host.clone(), &[]),
            path: environment::lookup(&host, "PATH").map(OsStr::to_os_string),
            auth_path,
        }
    }

    fn base_workspace(&self) -> io::Result<PathBuf> {
        workspace::prepare(&self.work_dir)
    }

    fn command(
        &self,
        cwd: &Path,
        grok_home: &Path,
        executable: &Path,
        args: Vec<OsString>,
    ) -> ProcessSpec {
        let mut spec = ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env(
                "PATH",
                environment::search_path_for(executable, self.path.as_deref()),
            )
            // Isolate every mutable/customizable Grok surface while keeping
            // the first-party cached OAuth file as the sole auth input.
            .env("GROK_HOME", grok_home.as_os_str())
            .env("GROK_DISABLE_API_KEY_AUTH", "1")
            .env("GROK_DISABLE_AUTOUPDATER", "1")
            .env("GROK_SUBAGENTS", "0")
            .env("GROK_MEMORY", "0")
            .env("GROK_WORKFLOWS", "0")
            .env("GROK_WEB_FETCH", "0")
            .env("GROK_TELEMETRY_ENABLED", "false")
            .env("GROK_TELEMETRY_TRACE_UPLOAD", "false")
            .env("GROK_FEEDBACK_ENABLED", "false")
            .env("GROK_FOLDER_TRUST", "0")
            .env("GROK_PROMPT_SUGGESTIONS", "false")
            .current_dir(cwd);
        if let Some(auth_path) = &self.auth_path {
            spec = spec.env("GROK_AUTH_PATH", auth_path.as_os_str());
        }
        spec
    }
}

pub struct Grok {
    search: SearchPath,
    launch: Rc<Launch>,
    timeouts: Timeouts,
}

impl Grok {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        Self::new(
            discovery::installed(),
            workspace::default_for(&host, "grok"),
        )
    }

    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        // A hard-killed host cannot run TurnWorkspace::drop. Clear only the
        // Grok directories Pervue names itself before this adapter starts
        // serving requests, so prompt/context files from an interrupted prior
        // host do not accumulate in the cache.
        if let Ok(base) = workspace::prepare(&work_dir) {
            sweep_stale_workspaces(&base);
        }
        Self {
            search,
            launch: Rc::new(Launch::new(work_dir, std::env::vars_os().collect())),
            timeouts: TIMEOUTS,
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
    pub fn with_timeouts(mut self, timeouts: Timeouts) -> Self {
        self.timeouts = timeouts;
        self
    }

    fn executable(&self) -> Option<PathBuf> {
        self.search.find(EXECUTABLE)
    }
}

impl Provider for Grok {
    fn id(&self) -> &str {
        ID
    }

    fn timeouts(&self) -> Timeouts {
        self.timeouts
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
        let Ok(base) = self.launch.base_workspace() else {
            return Box::new(Scripted::new([
                status_update(Availability::Unavailable, Authentication::Unknown),
                Update::Completed,
            ]));
        };
        let Ok(workspace) = ProbeWorkspace::create(&base) else {
            return Box::new(Scripted::new([
                status_update(Availability::Unavailable, Authentication::Unknown),
                Update::Completed,
            ]));
        };
        let spec = self.launch.command(
            workspace.path(),
            workspace.grok_home(),
            &executable,
            vec![OsString::from("models")],
        );
        Box::new(match Process::spawn(&spec) {
            Ok(mut process) => {
                process.close_stdin();
                StatusCheck::Probing {
                    process: Box::new(process),
                    workspace: Some(workspace),
                    give_up: after(STATUS_PROBE),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }
            }
            Err(_) => StatusCheck::Done(VecDeque::from([
                status_update(Availability::Unavailable, Authentication::Unknown),
                Update::Completed,
            ])),
        })
    }

    fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
        if request
            .model
            .as_deref()
            .is_some_and(|model| !model.starts_with("grok-"))
        {
            return Box::new(Scripted::failed(MODEL_NOT_SUPPORTED));
        }
        if request
            .conversation_id
            .as_deref()
            .is_some_and(|id| !is_pervue_conversation_id(id))
        {
            return Box::new(Scripted::failed(UNKNOWN_CONVERSATION));
        }
        let Some(executable) = self.executable() else {
            return Box::new(Scripted::failed(NOT_INSTALLED));
        };
        let Ok(base) = self.launch.base_workspace() else {
            return Box::new(Scripted::failed(NO_WORKSPACE));
        };

        let mut prompt = provider_prompt(&request.history, request.context.as_ref(), &request.text);
        if request.native_search {
            prompt.insert_str(0, SEARCH_INSTRUCTIONS);
        }
        let workspace = match TurnWorkspace::create(&base, request.native_search, &prompt) {
            Ok(workspace) => workspace,
            Err(_) => return Box::new(Scripted::failed(NO_WORKSPACE)),
        };

        let new_conversation = request.conversation_id.is_none();
        let conversation_id = request.conversation_id.unwrap_or_else(new_conversation_id);
        let args = grok_args(&workspace, request.native_search, request.model.as_deref());
        let expected_cwd = workspace.path().to_path_buf();
        let spec = self
            .launch
            .command(workspace.path(), workspace.grok_home(), &executable, args);
        let Ok(mut process) = Process::spawn(&spec) else {
            return Box::new(Scripted::failed(START_FAILED));
        };
        process.close_stdin();

        Box::new(Turn {
            stream: LineStream::new(process, MAX_LINE_BYTES).keeping_stderr_tail(STDERR_TAIL_BYTES),
            workspace: Some(workspace),
            queue: VecDeque::new(),
            conversation_id,
            announce_conversation: new_conversation,
            expected_cwd,
            requested_model: request.model,
            initialized: false,
            cancelled: false,
            native_search: request.native_search,
            searched: false,
            saw_text: false,
            sources: SourceCollector::new(ID),
            outcome: None,
            finish_by: None,
            done: false,
        })
    }
}

fn status_update(availability: Availability, authentication: Authentication) -> Update {
    Update::Status {
        provider_id: ID.to_owned(),
        status: ProviderState {
            availability,
            authentication,
            capabilities: CAPABILITIES,
            // Grok's signed-in model catalog changes frequently. Pervue accepts
            // valid grok-* IDs and deliberately does not freeze suggestions.
            models: &[],
        },
    }
}

enum StatusCheck {
    Probing {
        process: Box<Process>,
        workspace: Option<ProbeWorkspace>,
        give_up: Instant,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    Done(VecDeque<Update>),
}

impl Exchange for StatusCheck {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        let busy_until = deadline.max(after(BUSY_LIMIT));
        loop {
            match self {
                Self::Done(queue) => return queue.pop_front(),
                Self::Probing {
                    process,
                    workspace,
                    give_up,
                    stdout,
                    stderr,
                } => {
                    let poll_until = deadline.min(*give_up);
                    match process.next_event(poll_until) {
                        Some(Event::Stdout(bytes)) => {
                            keep_head(stdout, &bytes, STATUS_OUTPUT_BYTES)
                        }
                        Some(Event::Stderr(bytes)) => keep_tail(stderr, &bytes, STDERR_TAIL_BYTES),
                        Some(Event::Exited(exit)) => {
                            let text = String::from_utf8_lossy(stdout);
                            let error = String::from_utf8_lossy(stderr);
                            let authenticated = text.contains("You are logged in with ");
                            let unauthenticated = text.contains("You are not authenticated.")
                                || text.contains("You are using XAI_API_KEY.")
                                || text.contains("using its own API key")
                                || text.contains("deployment key")
                                || output::authentication_failure(&error);
                            let success = exit.status.is_some_and(|status| status.success());
                            let authentication = if authenticated {
                                Authentication::Authenticated
                            } else if unauthenticated {
                                Authentication::Unauthenticated
                            } else {
                                Authentication::Unknown
                            };
                            let availability = if success || unauthenticated {
                                Availability::Available
                            } else {
                                Availability::Unavailable
                            };
                            workspace.take();
                            *self = Self::Done(VecDeque::from([
                                status_update(availability, authentication),
                                Update::Completed,
                            ]));
                        }
                        None if Instant::now() >= *give_up => {
                            process.kill();
                            workspace.take();
                            *self = Self::Done(VecDeque::from([
                                status_update(Availability::Unavailable, Authentication::Unknown),
                                Update::Completed,
                            ]));
                        }
                        None => return None,
                    }
                }
            }
            if Instant::now() >= busy_until {
                return None;
            }
        }
    }

    fn cancel(&mut self, _grace: Duration) {
        if let Self::Probing {
            process, workspace, ..
        } = self
        {
            process.kill();
            workspace.take();
        }
        *self = Self::Done(VecDeque::from([Update::Stopped]));
    }
}

struct ProbeWorkspace {
    path: PathBuf,
    grok_home: PathBuf,
}

impl ProbeWorkspace {
    fn create(base: &Path) -> io::Result<Self> {
        let path = unique_child(base, "status");
        let grok_home = path.join("grok-home");
        create_private_dir(&grok_home)?;
        Ok(Self { path, grok_home })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn grok_home(&self) -> &Path {
        &self.grok_home
    }
}

impl Drop for ProbeWorkspace {
    fn drop(&mut self) {
        let _ = forget::remove(&self.path);
    }
}

struct TurnWorkspace {
    path: PathBuf,
    grok_home: PathBuf,
    prompt: PathBuf,
    agent: PathBuf,
}

impl TurnWorkspace {
    fn create(base: &Path, search: bool, prompt: &str) -> io::Result<Self> {
        let path = unique_child(base, "turn");
        let grok_home = path.join("grok-home");
        create_private_dir(&grok_home)?;
        let prompt_path = path.join("prompt.txt");
        write_private_file(&prompt_path, prompt.as_bytes())?;
        let agent = path.join("agent.md");
        write_private_file(
            &agent,
            if search {
                SEARCH_AGENT.as_bytes()
            } else {
                PLAIN_AGENT.as_bytes()
            },
        )?;
        Ok(Self {
            path,
            grok_home,
            prompt: prompt_path,
            agent,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn grok_home(&self) -> &Path {
        &self.grok_home
    }
}

impl Drop for TurnWorkspace {
    fn drop(&mut self) {
        let _ = forget::remove(&self.path);
    }
}

fn grok_args(workspace: &TurnWorkspace, search: bool, model: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--prompt-file"),
        workspace.prompt.as_os_str().to_os_string(),
        OsString::from("--verbatim"),
        OsString::from("--output-format"),
        OsString::from("streaming-messages-json"),
        OsString::from("--agent-profile"),
        workspace.agent.as_os_str().to_os_string(),
        OsString::from("--no-subagents"),
        OsString::from("--no-auto-update"),
        OsString::from("--max-turns"),
        OsString::from("8"),
        OsString::from("--permission-mode"),
        OsString::from("dontAsk"),
        OsString::from("--disallowed-tools"),
        OsString::from("Agent"),
    ];
    // --tools is a session-level final clamp in current Grok Build. Keep it
    // on both modes so profile/default-tool drift cannot re-enable local tools.
    // Plain mode then disables its only allowed server tool, yielding no tools.
    args.extend([OsString::from("--tools"), OsString::from("web_search")]);
    if !search {
        args.push(OsString::from("--disable-web-search"));
    }
    if let Some(model) = model {
        args.extend([OsString::from("--model"), OsString::from(model)]);
    }
    args
}

struct Turn {
    // Process first: on Drop it dies before the private cwd is removed.
    stream: LineStream,
    workspace: Option<TurnWorkspace>,
    queue: VecDeque<Update>,
    conversation_id: String,
    announce_conversation: bool,
    expected_cwd: PathBuf,
    requested_model: Option<String>,
    initialized: bool,
    cancelled: bool,
    native_search: bool,
    searched: bool,
    saw_text: bool,
    sources: SourceCollector,
    outcome: Option<Result<(), ErrorBody<'static>>>,
    finish_by: Option<Instant>,
    done: bool,
}

impl Turn {
    fn fail(&mut self, error: ErrorBody<'static>) {
        self.queue.clear();
        self.outcome = Some(Err(error));
        self.finish_by = None;
        self.stream.cancel(Duration::ZERO);
    }

    fn on_line(&mut self, line: &str) {
        if self.outcome.is_some() || self.cancelled {
            return;
        }
        match output::parse(line) {
            Err(_) => self.fail(MALFORMED_OUTPUT),
            Ok(Line::Init {
                api_key_source,
                model,
                cwd,
                tools,
                skills,
                active_mcp_servers,
            }) => {
                if self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                let model_ok = model.starts_with("grok-")
                    && self
                        .requested_model
                        .as_deref()
                        .is_none_or(|requested| requested == model);
                let tools_ok = if self.native_search {
                    tools.len() == 1 && tools[0] == "web_search"
                } else {
                    tools.is_empty()
                };
                if api_key_source != "oauth"
                    || !model_ok
                    || Path::new(&cwd) != self.expected_cwd
                    || !tools_ok
                    || !skills.is_empty()
                    || active_mcp_servers != 0
                {
                    return self.fail(if api_key_source != "oauth" {
                        AUTH_MODE_REJECTED
                    } else {
                        BOUNDARY_VIOLATION
                    });
                }
                self.initialized = true;
                if self.announce_conversation {
                    self.queue
                        .push_back(Update::ConversationCreated(self.conversation_id.clone()));
                }
                self.queue.push_back(Update::Started {
                    conversation_id: Some(self.conversation_id.clone()),
                });
            }
            Ok(Line::Assistant {
                text,
                searched,
                sources,
                forbidden_tool,
            }) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if forbidden_tool || (searched && !self.native_search) {
                    return self.fail(BOUNDARY_VIOLATION);
                }
                self.searched |= searched;
                for source in sources {
                    if let Some(source) = self.sources.push(source) {
                        self.queue.push_back(Update::Source(source));
                    }
                }
                // Search frames can contain "I will search..." narration in
                // the same assistant message as the server tool. The terminal
                // result is the authoritative final answer, so hold all search
                // text until then.
                if !self.native_search && !text.is_empty() {
                    self.saw_text = true;
                    self.queue.push_back(Update::Delta(text));
                }
            }
            Ok(Line::ResultSuccess { text }) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search {
                    if !text.is_empty() {
                        self.saw_text = true;
                        self.queue.push_back(Update::Delta(text));
                    }
                    self.outcome = Some(if self.searched && self.sources.count() > 0 {
                        Ok(())
                    } else {
                        Err(NATIVE_SEARCH_NO_SOURCES)
                    });
                } else {
                    if !self.saw_text && !text.is_empty() {
                        self.queue.push_back(Update::Delta(text));
                    }
                    self.outcome = Some(Ok(()));
                }
                self.finish_by = Some(after(FINISH_GRACE));
            }
            Ok(Line::ResultFailed(error)) => self.fail(error),
            Ok(Line::Ignored) => {}
        }
    }

    fn ended(&mut self, exit: &Exit) -> Update {
        self.workspace.take();
        if self.cancelled {
            return Update::Stopped;
        }
        match self.outcome.take() {
            Some(Ok(())) => Update::Completed,
            Some(Err(error)) => Update::Failed(error),
            None if exit.status.is_some_and(|status| status.success()) => {
                Update::Failed(MALFORMED_OUTPUT)
            }
            None => {
                let failure =
                    output::provider_failure(&String::from_utf8_lossy(self.stream.stderr_tail()));
                if failure.reason == "PROVIDER_UNAVAILABLE" {
                    Update::Failed(PROCESS_EXITED)
                } else {
                    Update::Failed(failure)
                }
            }
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
            if self.done || Instant::now() >= busy_until {
                return None;
            }
            if self
                .finish_by
                .is_some_and(|finish_by| Instant::now() >= finish_by)
            {
                self.finish_by = None;
                self.stream.cancel(Duration::ZERO);
            }
            let wait = self
                .finish_by
                .map_or(deadline, |finish_by| deadline.min(finish_by));
            match self.stream.next(wait) {
                Some(Output::Line(line)) => self.on_line(&line),
                Some(Output::Final(exit) | Output::Stopped(exit)) => {
                    let update = self.ended(&exit);
                    self.done = true;
                    return Some(update);
                }
                Some(Output::Error(_)) => {
                    self.workspace.take();
                    self.done = true;
                    return Some(if self.cancelled {
                        Update::Stopped
                    } else {
                        Update::Failed(MALFORMED_OUTPUT)
                    });
                }
                None if self
                    .finish_by
                    .is_some_and(|finish_by| Instant::now() >= finish_by) => {}
                None => return None,
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        if self.cancelled || self.done {
            return;
        }
        self.cancelled = true;
        self.queue.clear();
        self.finish_by = None;
        self.stream.cancel(grace);
    }
}

fn sweep_stale_workspaces(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if (name.starts_with("turn-") || name.starts_with("status-"))
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            let _ = forget::remove(&entry.path());
        }
    }
}

fn unique_child(base: &Path, prefix: &str) -> PathBuf {
    base.join(format!(
        "{prefix}-{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    ))
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(path)
    }
}

fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn is_pervue_conversation_id(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("conv_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn new_conversation_id() -> String {
    format!(
        "conv_{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    )
}

fn keep_head(head: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    if head.len() >= limit {
        return;
    }
    let remaining = limit - head.len();
    head.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let bytes = &bytes[bytes.len().saturating_sub(limit)..];
    let excess = (tail.len() + bytes.len()).saturating_sub(limit);
    tail.drain(..excess);
    tail.extend_from_slice(bytes);
}

fn after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_arguments_keep_prompt_text_out_of_argv() {
        let workspace = TurnWorkspace {
            path: PathBuf::from("/tmp/pervue-grok"),
            grok_home: PathBuf::from("/tmp/pervue-grok/grok-home"),
            prompt: PathBuf::from("/tmp/pervue-grok/prompt.txt"),
            agent: PathBuf::from("/tmp/pervue-grok/agent.md"),
        };
        let args = grok_args(&workspace, true, Some("grok-4.6"));
        let rendered: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert!(
            rendered
                .windows(2)
                .any(|pair| pair == ["--tools", "web_search"])
        );
        assert!(
            rendered
                .windows(2)
                .any(|pair| pair == ["--model", "grok-4.6"])
        );
        assert!(rendered.contains(&std::borrow::Cow::Borrowed("streaming-messages-json")));
        assert!(
            !rendered
                .iter()
                .any(|arg| arg.contains("Current user question"))
        );
        std::mem::forget(workspace);
    }

    #[test]
    fn capabilities_match_the_provider_contract() {
        assert_eq!(CAPABILITIES.streaming, Capability::Supported);
        assert_eq!(CAPABILITIES.continuation, Capability::Supported);
        assert_eq!(CAPABILITIES.web_search, Capability::Supported);
        assert_eq!(CAPABILITIES.page_context, Capability::Supported);
        assert_eq!(CAPABILITIES.model_selection, Capability::Supported);
        assert_eq!(CAPABILITIES.cancellation, Capability::Supported);
    }

    #[test]
    fn agent_profiles_are_closed_except_for_native_search() {
        assert!(PLAIN_AGENT.contains("tools: []"));
        assert!(SEARCH_AGENT.contains("  - web_search"));
        for profile in [PLAIN_AGENT, SEARCH_AGENT] {
            assert!(profile.contains("promptMode: full"));
            assert!(profile.contains("discoverSkills: false"));
            assert!(profile.contains("inheritSkills: false"));
            assert!(profile.contains("agentsMd: false"));
            assert!(profile.contains("mcpInheritance: none"));
            assert!(profile.contains("permissionMode: dontAsk"));
            assert!(profile.contains("  - Agent"));
        }
    }
}
