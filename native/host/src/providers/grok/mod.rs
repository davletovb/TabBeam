//! xAI Grok adapter through one-shot Grok Build headless mode (PRO-09).
//!
//! Pervue intentionally does not run Grok as a persistent ACP/app server.
//! Every turn gets a fresh headless `grok` process, a private working
//! directory and GROK_HOME, and the protocol's bounded history. Only the
//! existing Grok/X OAuth file is referenced outside that directory.
//!
//! Ordinary turns expose no tools. Grok web search is deliberately not
//! advertised yet because the currently shipped headless CLI does not expose
//! the backend search surface that upstream main documents.
//! The headless stream's init event is verified before any answer text is
//! forwarded, and the private workspace (including Grok's session files and
//! the prompt file) is removed after the child has exited.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::codex::workspace;
use super::discovery;
use super::environment;
use super::forget;
use super::private_fs;
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

pub const ID: &str = "grok";
const EXECUTABLE: &str = "grok";
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;
const STATUS_OUTPUT_BYTES: usize = 16 * 1024;
const STATUS_PROBE: Duration = Duration::from_secs(10);
const FINISH_GRACE: Duration = Duration::from_secs(5);
const OWNER_FILE: &str = ".pervue-owner";
const STALE_WORKSPACE_AFTER: Duration = Duration::from_secs(15 * 60);
const LEGACY_STALE_WORKSPACE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

pub const CAPABILITIES: Capabilities = Capabilities {
    streaming: Capability::Unsupported,
    continuation: Capability::Supported,
    web_search: Capability::Unsupported,
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
const SEARCH_UNSUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "SEARCH_UNSUPPORTED",
    message: "Grok Build's shipped headless CLI doesn't expose a Pervue-safe native web-search surface yet. Turn Web off and try again.",
    retryable: false,
};
const MODEL_MISMATCH: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MODEL_MISMATCH",
    message: "Grok started a different model than Pervue requested. Update Grok Build or choose its default model, then try again.",
    retryable: false,
};
const WORKSPACE_MISMATCH: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_MISMATCH",
    message: "Grok didn't stay in Pervue's private workspace, so the turn was stopped.",
    retryable: false,
};
const TOOLSET_MISMATCH: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "TOOLSET_MISMATCH",
    message: "Grok exposed tools in a text-only Pervue turn, so the turn was stopped.",
    retryable: false,
};
const SKILLS_MISMATCH: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "SKILLS_MISMATCH",
    message: "Grok loaded skills in Pervue's isolated turn, so the turn was stopped.",
    retryable: false,
};
const MCP_MISMATCH: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MCP_MISMATCH",
    message: "Grok connected an MCP server in Pervue's isolated turn, so the turn was stopped.",
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

#[derive(Debug, Clone)]
struct Launch {
    work_dir: PathBuf,
    inherited: Vec<(OsString, OsString)>,
    path: Option<OsString>,
    auth_path: Option<PathBuf>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        let auth_path = environment::lookup(&host, "GROK_AUTH_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                environment::lookup(&host, "GROK_HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("auth.json"))
            })
            .or_else(|| environment::home_dir(&host).map(|home| home.join(".grok/auth.json")));
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
                    give_up: private_fs::after(STATUS_PROBE),
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
        if request.native_search {
            return Box::new(Scripted::failed(SEARCH_UNSUPPORTED));
        }
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
            .is_some_and(|id| !private_fs::is_conversation_id(id))
        {
            return Box::new(Scripted::failed(UNKNOWN_CONVERSATION));
        }
        let Some(executable) = self.executable() else {
            return Box::new(Scripted::failed(NOT_INSTALLED));
        };
        let Ok(base) = self.launch.base_workspace() else {
            return Box::new(Scripted::failed(NO_WORKSPACE));
        };

        let prompt = provider_prompt(&request.history, request.context.as_ref(), &request.text);
        let workspace = match TurnWorkspace::create(&base, &prompt) {
            Ok(workspace) => workspace,
            Err(_) => return Box::new(Scripted::failed(NO_WORKSPACE)),
        };

        let new_conversation = request.conversation_id.is_none();
        let conversation_id = request.conversation_id.unwrap_or_else(new_conversation_id);
        let args = grok_args(&workspace, request.model.as_deref());
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
            saw_text: false,
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
        if let Some(workspace) = self.workspace.as_ref() {
            let _ = workspace.touch();
        }
        let busy_until = deadline.max(private_fs::after(BUSY_LIMIT));
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
                    if let Some(workspace) = workspace.as_ref() {
                        let _ = workspace.touch();
                    }
                    // A status-of-all request starts every probe up front and
                    // polls providers in sequence. If this probe already
                    // finished while another provider was being polled, drain
                    // that queued exit before applying the timeout.
                    let timed_out = Instant::now() >= *give_up;
                    let poll_until = if timed_out {
                        Instant::now()
                    } else {
                        deadline.min(*give_up)
                    };
                    match process.next_event(poll_until) {
                        Some(Event::Stdout(bytes)) => {
                            keep_head(stdout, &bytes, STATUS_OUTPUT_BYTES)
                        }
                        Some(Event::Stderr(bytes)) => private_fs::keep_tail(stderr, &bytes, STDERR_TAIL_BYTES),
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
        let path = private_fs::unique_child(base, "status");
        private_fs::create_private_dir(&path)?;
        private_fs::write_private_file(&path.join(OWNER_FILE), b"live")?;
        let grok_home = path.join("grok-home");
        private_fs::create_private_dir(&grok_home)?;
        Ok(Self { path, grok_home })
    }

    fn touch(&self) -> io::Result<()> {
        fs::write(self.path.join(OWNER_FILE), b"live")
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
        forget::remove_in_background(self.path.clone());
    }
}

struct TurnWorkspace {
    path: PathBuf,
    grok_home: PathBuf,
    prompt: PathBuf,
    agent: PathBuf,
}

impl TurnWorkspace {
    fn create(base: &Path, prompt: &str) -> io::Result<Self> {
        let path = private_fs::unique_child(base, "turn");
        private_fs::create_private_dir(&path)?;
        // Construct the guard before any sensitive file is written so every
        // later error path removes the partial workspace.
        let workspace = Self {
            grok_home: path.join("grok-home"),
            prompt: path.join("prompt.txt"),
            agent: path.join("agent.md"),
            path,
        };
        workspace.initialize(prompt)?;
        Ok(workspace)
    }

    fn initialize(&self, prompt: &str) -> io::Result<()> {
        private_fs::write_private_file(&self.path.join(OWNER_FILE), b"live")?;
        private_fs::create_private_dir(&self.grok_home)?;
        private_fs::write_private_file(&self.prompt, prompt.as_bytes())?;
        private_fs::write_private_file(&self.agent, PLAIN_AGENT.as_bytes())
    }

    fn touch(&self) -> io::Result<()> {
        fs::write(self.path.join(OWNER_FILE), b"live")
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
        if !self.path.as_os_str().is_empty() {
            forget::remove_in_background(self.path.clone());
        }
    }
}

fn grok_args(workspace: &TurnWorkspace, model: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--prompt-file"),
        workspace.prompt.as_os_str().to_os_string(),
        OsString::from("--verbatim"),
        OsString::from("--output-format"),
        OsString::from("streaming-messages-json"),
        OsString::from("--include-partial-messages"),
        OsString::from("--agent"),
        workspace.agent.as_os_str().to_os_string(),
        OsString::from("--no-subagents"),
        OsString::from("--no-auto-update"),
        OsString::from("--max-turns"),
        OsString::from("8"),
        OsString::from("--permission-mode"),
        OsString::from("dontAsk"),
        OsString::from("--disallowed-tools"),
        OsString::from("Agent,search_tool,use_tool"),
        // Shipped Grok always offers the MCP search/use umbrellas unless
        // explicitly denied. Clamp the built-in surface to web_search, then
        // disable that hosted tool too: the resulting turn is text-only.
        OsString::from("--tools"),
        OsString::from("web_search"),
        OsString::from("--disable-web-search"),
    ];
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
    saw_text: bool,
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
                all_mcp_disabled,
            }) => {
                if self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if api_key_source != "oauth" {
                    return self.fail(AUTH_MODE_REJECTED);
                }
                if !model_matches(self.requested_model.as_deref(), &model) {
                    return self.fail(MODEL_MISMATCH);
                }
                if !forget::same_directory(&cwd, &self.expected_cwd) {
                    return self.fail(WORKSPACE_MISMATCH);
                }
                if !tools.is_empty() {
                    return self.fail(TOOLSET_MISMATCH);
                }
                if !skills.is_empty() {
                    return self.fail(SKILLS_MISMATCH);
                }
                if !all_mcp_disabled {
                    return self.fail(MCP_MISMATCH);
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
                activity,
                forbidden_tool,
            }) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if forbidden_tool {
                    return self.fail(BOUNDARY_VIOLATION);
                }
                if !text.is_empty() {
                    self.saw_text = true;
                    self.queue.push_back(Update::Delta(text));
                } else if activity {
                    self.queue.push_back(Update::Activity);
                }
            }
            Ok(Line::ResultSuccess { text }) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if !self.saw_text && !text.is_empty() {
                    self.queue.push_back(Update::Delta(text));
                }
                self.outcome = Some(Ok(()));
                self.finish_by = Some(private_fs::after(FINISH_GRACE));
            }
            Ok(Line::ResultFailed(error)) => self.fail(error),
            Ok(Line::Activity) if self.initialized => self.queue.push_back(Update::Activity),
            Ok(Line::Activity | Line::Ignored) => {}
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
        let busy_until = deadline.max(private_fs::after(BUSY_LIMIT));
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

fn model_matches(requested: Option<&str>, actual: &str) -> bool {
    if !actual.starts_with("grok-") {
        return false;
    }
    requested.is_none_or(|requested| {
        actual == requested
            || actual
                .strip_prefix(requested)
                .and_then(|suffix| suffix.as_bytes().first())
                .is_some_and(|byte| matches!(*byte, b'-' | b'.' | b'@' | b':'))
    })
}

fn sweep_stale_workspaces(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !(name.starts_with("turn-") || name.starts_with("status-"))
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let path = entry.path();
        let owner = path.join(OWNER_FILE);
        let stale = age_at_least(&owner, STALE_WORKSPACE_AFTER)
            || (!owner.exists() && age_at_least(&path, LEGACY_STALE_WORKSPACE_AFTER));
        if stale {
            forget::remove_in_background(path);
        }
    }
}

fn age_at_least(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed >= age)
}


fn keep_head(head: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    if head.len() >= limit {
        return;
    }
    let remaining = limit - head.len();
    head.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
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
        let args = grok_args(&workspace, Some("grok-4.6"));
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
        assert_eq!(CAPABILITIES.streaming, Capability::Unsupported);
        assert_eq!(CAPABILITIES.continuation, Capability::Supported);
        assert_eq!(CAPABILITIES.web_search, Capability::Unsupported);
        assert_eq!(CAPABILITIES.page_context, Capability::Supported);
        assert_eq!(CAPABILITIES.model_selection, Capability::Supported);
        assert_eq!(CAPABILITIES.cancellation, Capability::Supported);
    }

    #[test]
    fn agent_profile_and_cli_clamps_are_text_only() {
        assert!(PLAIN_AGENT.contains("tools: []"));
        assert!(PLAIN_AGENT.contains("promptMode: full"));
        assert!(PLAIN_AGENT.contains("discoverSkills: false"));
        assert!(PLAIN_AGENT.contains("inheritSkills: false"));
        assert!(PLAIN_AGENT.contains("agentsMd: false"));
        assert!(PLAIN_AGENT.contains("mcpInheritance: none"));
        assert!(PLAIN_AGENT.contains("permissionMode: dontAsk"));
        assert!(PLAIN_AGENT.contains("  - Agent"));

        let workspace = TurnWorkspace {
            path: PathBuf::from("/tmp/pervue-grok"),
            grok_home: PathBuf::from("/tmp/pervue-grok/grok-home"),
            prompt: PathBuf::from("/tmp/pervue-grok/prompt.txt"),
            agent: PathBuf::from("/tmp/pervue-grok/agent.md"),
        };
        let args = grok_args(&workspace, None);
        let rendered: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert!(rendered.windows(2).any(|pair| pair == ["--agent", "/tmp/pervue-grok/agent.md"]));
        assert!(rendered.windows(2).any(|pair| pair == ["--disallowed-tools", "Agent,search_tool,use_tool"]));
        assert!(rendered.contains(&std::borrow::Cow::Borrowed("--disable-web-search")));
        std::mem::forget(workspace);
    }

    #[test]
    fn model_aliases_may_resolve_to_versioned_ids() {
        assert!(model_matches(Some("grok-4"), "grok-4-0709"));
        assert!(model_matches(Some("grok-4.6"), "grok-4.6"));
        assert!(!model_matches(Some("grok-4"), "grok-40"));
        assert!(!model_matches(Some("grok-4"), "claude-grok-4"));
    }
}
