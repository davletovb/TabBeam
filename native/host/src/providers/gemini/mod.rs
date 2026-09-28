//! Google Gemini adapter through Antigravity CLI ('agy') (PRO-08).
//!
//! Each Pervue turn is a one-shot Antigravity run in a private workspace with
//! a workspace-local agent. Ordinary turns have no tools; Web turns allow only
//! 'search_web'. Pervue sends bounded conversation history every turn instead
//! of depending on Antigravity's native continuation state, then removes the
//! Antigravity transcript once the child has exited.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::codex::workspace;
use super::discovery;
use super::environment;
use super::forget;
use super::private_fs;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::{SEARCH_INSTRUCTIONS, provider_prompt};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::search::{NATIVE_SEARCH_NO_SOURCES, SourceCollector, codex_message_sources};
use pervue_core::discovery::SearchPath;
use pervue_core::process::{Event, Exit, Process, ProcessSpec};
use pervue_core::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "gemini";
const EXECUTABLE: &str = "agy";
const PLAIN_AGENT: &str = "pervue-text";
const SEARCH_AGENT: &str = "pervue-search";
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;
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
    message: "Antigravity CLI isn't installed. Install Antigravity CLI, then try again.",
    retryable: false,
};
const START_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Antigravity CLI couldn't start. Reinstall it, then try again.",
    retryable: false,
};
const NO_WORKSPACE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_UNAVAILABLE",
    message: "Pervue couldn't prepare a private folder for Antigravity. Check your cache folder, then try again.",
    retryable: false,
};
const PROCESS_EXITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROCESS_EXITED",
    message: "Antigravity stopped unexpectedly. Try again.",
    retryable: true,
};
const MALFORMED_OUTPUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MALFORMED_PROVIDER_OUTPUT",
    message: "Antigravity answered in a way Pervue doesn't understand. Update Antigravity CLI and Pervue, then try again.",
    retryable: false,
};
const BOUNDARY_VIOLATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_BOUNDARY_VIOLATION",
    message: "Antigravity tried to use a tool or step Pervue doesn't allow, so the turn was stopped.",
    retryable: false,
};
const AGENT_NOT_USED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_AGENT_NOT_USED",
    message: "Antigravity didn't use Pervue's restricted agent, so the turn was stopped. Update Antigravity CLI, then try again.",
    retryable: false,
};
const PERMISSIONS_TOO_OPEN: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_PERMISSIONS_TOO_OPEN",
    message: "Antigravity is set to run tools without asking, so Pervue stopped the turn. Set Antigravity's tool permission to review requests, then try again.",
    retryable: false,
};
const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "This Gemini conversation ID is not one Pervue issued. Start a new conversation.",
    retryable: false,
};
const MODEL_NOT_SUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "MODEL_NOT_SUPPORTED",
    message: "The Gemini provider accepts Gemini model IDs only.",
    retryable: false,
};

const PLAIN_AGENT_DEFINITION: &str = r#"---
name: pervue-text
description: Text-only Pervue Gemini responder with no local or external tools.
tools: []
mainAgent: true
subagent: false
inheritCustomizations: false
inheritMcp: false
commandExecutionPolicy: "off"
mcpServers: []
skills: []
plugins: []
rules: []
agents: []
hooks: []
---
# System Prompt
Answer the user's request directly as text. Do not use tools, subagents, files, commands, browsers, MCP servers, skills, plugins, hooks, or external side effects.
"#;

const SEARCH_AGENT_DEFINITION: &str = r#"---
name: pervue-search
description: Pervue Gemini web responder allowed to use only Antigravity's native web search.
tools:
  - search_web
mainAgent: true
subagent: false
inheritCustomizations: false
inheritMcp: false
commandExecutionPolicy: "off"
mcpServers: []
skills: []
plugins: []
rules: []
agents: []
hooks: []
---
# System Prompt
Use search_web when answering. Do not use any other tool, subagent, file, command, browser automation, MCP server, skill, plugin, hook, or external side effect. Cite the pages you use as Markdown links.
"#;

type PendingCleanups = Rc<RefCell<HashMap<String, Vec<String>>>>;

#[derive(Debug, Clone)]
struct Launch {
    work_dir: PathBuf,
    inherited: Vec<(OsString, OsString)>,
    path: Option<OsString>,
    home: Option<PathBuf>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        Self {
            work_dir,
            inherited: environment::inherit(host.clone(), &[]),
            path: environment::lookup(&host, "PATH").map(OsStr::to_os_string),
            home: environment::home_dir(&host),
        }
    }

    fn base_workspace(&self) -> io::Result<PathBuf> {
        workspace::prepare(&self.work_dir)
    }

    fn command(&self, cwd: &Path, executable: &Path, args: Vec<OsString>) -> ProcessSpec {
        ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env(
                "PATH",
                environment::search_path_for(executable, self.path.as_deref()),
            )
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .current_dir(cwd)
    }
}

pub struct Gemini {
    search: SearchPath,
    launch: Rc<Launch>,
    /// Pervue-owned cleanup records kept across host restarts.
    cleanup_dir: Option<PathBuf>,
    timeouts: Timeouts,
    pending_cleanups: PendingCleanups,
}

impl Gemini {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        let mut gemini = Self::new(
            discovery::installed(),
            workspace::default_for(&host, "antigravity"),
        );
        gemini.cleanup_dir = installed_cleanup_dir();
        gemini
    }

    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        let cleanup_dir = Some(work_dir.with_extension("cleanups"));
        Self {
            search,
            launch: Rc::new(Launch::new(work_dir, std::env::vars_os().collect())),
            cleanup_dir,
            timeouts: TIMEOUTS,
            pending_cleanups: Rc::new(RefCell::new(HashMap::new())),
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

    /// Overrides the durable cleanup-record directory (used by isolated tests).
    #[must_use]
    pub fn with_cleanup_dir(mut self, cleanup_dir: PathBuf) -> Self {
        self.cleanup_dir = Some(cleanup_dir);
        self
    }

    fn executable(&self) -> Option<PathBuf> {
        self.search.find(EXECUTABLE)
    }
}

impl Provider for Gemini {
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
        let Ok(workspace) = self.launch.base_workspace() else {
            return Box::new(Scripted::new([
                status_update(Availability::Unavailable, Authentication::Unknown),
                Update::Completed,
            ]));
        };
        let spec = self
            .launch
            .command(&workspace, &executable, vec![OsString::from("models")]);
        Box::new(match Process::spawn(&spec) {
            Ok(mut process) => {
                process.close_stdin();
                StatusCheck::Probing {
                    process,
                    give_up: private_fs::after(STATUS_PROBE),
                    stderr_tail: Vec::new(),
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
            .is_some_and(|model| !model.starts_with("gemini-"))
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
        let workspace = match TurnWorkspace::create(&base, request.native_search) {
            Ok(workspace) => workspace,
            Err(_) => return Box::new(Scripted::failed(NO_WORKSPACE)),
        };

        let new_conversation = request.conversation_id.is_none();
        let conversation_id = request.conversation_id.unwrap_or_else(new_conversation_id);
        let agent = if request.native_search {
            SEARCH_AGENT
        } else {
            PLAIN_AGENT
        };
        let mut prompt = provider_prompt(&request.history, request.context.as_ref(), &request.text);
        if request.native_search {
            prompt.insert_str(0, SEARCH_INSTRUCTIONS);
        }
        let input = serde_json::json!({
            "event": "user",
            "message": { "content": prompt }
        })
        .to_string()
            + "
";
        let args = agy_args(agent, request.model.as_deref());
        let spec = self.launch.command(workspace.path(), &executable, args);
        let Ok(mut process) = Process::spawn(&spec) else {
            return Box::new(Scripted::failed(START_FAILED));
        };
        if process.write(input.as_bytes()).is_err() {
            process.kill();
            return Box::new(Scripted::failed(START_FAILED));
        }
        process.close_stdin();

        Box::new(Turn {
            stream: LineStream::new(process, MAX_LINE_BYTES).keeping_stderr_tail(STDERR_TAIL_BYTES),
            workspace: Some(workspace),
            home: self.launch.home.clone(),
            cleanup_dir: self.cleanup_dir.clone(),
            pending_cleanups: Rc::clone(&self.pending_cleanups),
            queue: VecDeque::new(),
            conversation_id,
            announce_conversation: new_conversation,
            expected_agent: agent,
            initialized: false,
            antigravity_conversation: None,
            cancelled: false,
            native_search: request.native_search,
            searched: false,
            saw_delta: false,
            answer: String::new(),
            held: String::new(),
            held_step_done: false,
            answer_steps: HashSet::new(),
            answer_step: None,
            step_text: String::new(),
            messages: 0,
            sources: SourceCollector::new(ID),
            outcome: None,
            finish_by: None,
            done: false,
        })
    }

    fn forget(&self, conversation_id: &str) -> Box<dyn Exchange> {
        if !private_fs::is_conversation_id(conversation_id) {
            return Box::new(Scripted::new([Update::Completed]));
        }
        let memory_ids = self
            .pending_cleanups
            .borrow()
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        let home = self.launch.home.clone();
        let cleanup_dir = self.cleanup_dir.clone();
        let pending = Rc::clone(&self.pending_cleanups);
        let conversation = conversation_id.to_owned();
        let work_conversation = conversation.clone();
        forget::in_background(
            move || {
                let mut ids = memory_ids;
                if let Some(dir) = cleanup_dir.as_deref() {
                    ids.extend(read_pending_cleanup_ids(dir, &work_conversation)?);
                }
                ids.sort();
                ids.dedup();
                if !ids.is_empty() {
                    let home = home.as_deref().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            "home directory unavailable for Antigravity transcript cleanup",
                        )
                    })?;
                    for id in &ids {
                        remove_antigravity_transcript(home, id)?;
                    }
                }
                if let Some(dir) = cleanup_dir.as_deref() {
                    forget_cleanup_record(dir, &work_conversation)?;
                }
                Ok(())
            },
            move || {
                pending.borrow_mut().remove(&conversation);
            },
        )
    }
}

fn status_update(availability: Availability, authentication: Authentication) -> Update {
    Update::Status {
        provider_id: ID.to_owned(),
        status: ProviderState {
            availability,
            authentication,
            capabilities: CAPABILITIES,
            models: &[],
        },
    }
}

enum StatusCheck {
    Probing {
        process: Process,
        give_up: Instant,
        stderr_tail: Vec<u8>,
    },
    Done(VecDeque<Update>),
}

impl Exchange for StatusCheck {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        let busy_until = deadline.max(private_fs::after(BUSY_LIMIT));
        loop {
            match self {
                Self::Done(queue) => return queue.pop_front(),
                Self::Probing {
                    process,
                    give_up,
                    stderr_tail,
                } => {
                    // Even after the nominal deadline, first consume an exit
                    // that is already queued. This avoids killing a probe that
                    // finished while another provider status check was polled.
                    let timed_out = Instant::now() >= *give_up;
                    let poll_until = if timed_out {
                        Instant::now()
                    } else {
                        deadline.min(*give_up)
                    };
                    match process.next_event(poll_until) {
                        Some(Event::Stdout(_)) => {}
                        Some(Event::Stderr(bytes)) => {
                            private_fs::keep_tail(stderr_tail, &bytes, STDERR_TAIL_BYTES);
                        }
                        Some(Event::Exited(exit)) => {
                            let success = exit.status.is_some_and(|status| status.success());
                            let authentication = if success {
                                Authentication::Authenticated
                            } else if output::authentication_failure(&String::from_utf8_lossy(
                                stderr_tail,
                            )) {
                                Authentication::Unauthenticated
                            } else {
                                Authentication::Unknown
                            };
                            let availability =
                                if success || authentication == Authentication::Unauthenticated {
                                    Availability::Available
                                } else {
                                    Availability::Unavailable
                                };
                            *self = Self::Done(VecDeque::from([
                                status_update(availability, authentication),
                                Update::Completed,
                            ]));
                        }
                        None if Instant::now() >= *give_up => {
                            process.kill();
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
        if let Self::Probing { process, .. } = self {
            process.kill();
        }
        *self = Self::Done(VecDeque::from([Update::Stopped]));
    }
}

struct TurnWorkspace {
    path: PathBuf,
}

impl TurnWorkspace {
    fn create(base: &Path, search: bool) -> io::Result<Self> {
        let path = base.join(format!(
            "turn-{:016x}",
            RandomState::new().hash_one((SystemTime::now(), std::process::id()))
        ));
        private_fs::create_private_dir(&path)?;
        let agent = if search { SEARCH_AGENT } else { PLAIN_AGENT };
        let definition = if search {
            SEARCH_AGENT_DEFINITION
        } else {
            PLAIN_AGENT_DEFINITION
        };
        let agent_dir = path.join(".agents/agents").join(agent);
        private_fs::create_private_dir(&agent_dir)?;
        private_fs::write_private_file(&agent_dir.join("agent.md"), definition.as_bytes())?;
        let hooks_dir = path.join(".agents");
        private_fs::create_private_dir(&hooks_dir)?;
        private_fs::write_private_file(&hooks_dir.join("hooks.json"), b"{}\n")?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TurnWorkspace {
    fn drop(&mut self) {
        let _ = forget::remove(&self.path);
    }
}

fn private_fs::create_private_dir(path: &Path) -> io::Result<()> {
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

fn private_fs::write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

struct Turn {
    // Keep the process before the workspace so dropping a live turn stops the
    // child before its cwd is removed.
    stream: LineStream,
    workspace: Option<TurnWorkspace>,
    home: Option<PathBuf>,
    cleanup_dir: Option<PathBuf>,
    pending_cleanups: PendingCleanups,
    queue: VecDeque<Update>,
    conversation_id: String,
    announce_conversation: bool,
    expected_agent: &'static str,
    initialized: bool,
    antigravity_conversation: Option<String>,
    cancelled: bool,
    native_search: bool,
    searched: bool,
    saw_delta: bool,
    answer: String,
    /// One Antigravity agent-response step held until the next step reveals
    /// whether it was search narration.
    held: String,
    held_step_done: bool,
    /// Indices of the agent-response steps seen, so a later update that
    /// doesn't name its type still counts as answer text only for them.
    answer_steps: HashSet<u64>,
    /// The agent-response step being written, and its text so far.
    answer_step: Option<u64>,
    step_text: String,
    messages: usize,
    sources: SourceCollector,
    outcome: Option<Result<(), ErrorBody<'static>>>,
    finish_by: Option<Instant>,
    done: bool,
}

impl Turn {
    fn fail(&mut self, error: ErrorBody<'static>) {
        self.queue.clear();
        self.outcome = Some(Err(error));
        self.finish_by = Some(Instant::now());
    }

    fn register_cleanup_id(&mut self, id: String) {
        {
            let mut pending = self.pending_cleanups.borrow_mut();
            let ids = pending.entry(self.conversation_id.clone()).or_default();
            if !ids.iter().any(|candidate| candidate == &id) {
                ids.push(id.clone());
            }
        }
        if let Some(dir) = self.cleanup_dir.as_deref() {
            let _ = record_pending_cleanup_id(dir, &self.conversation_id, &id);
        }
    }

    fn clear_cleanup_ids(&mut self, removed: &HashSet<String>) {
        if removed.is_empty() {
            return;
        }
        let empty = {
            let mut pending = self.pending_cleanups.borrow_mut();
            let empty = if let Some(ids) = pending.get_mut(&self.conversation_id) {
                ids.retain(|candidate| !removed.contains(candidate));
                ids.is_empty()
            } else {
                true
            };
            if empty {
                pending.remove(&self.conversation_id);
            }
            empty
        };
        if let Some(dir) = self.cleanup_dir.as_deref() {
            for id in removed {
                let _ = forget_cleanup_id_record(dir, &self.conversation_id, id);
            }
            if empty {
                let _ = forget_cleanup_record(dir, &self.conversation_id);
            }
        }
    }

    fn on_line(&mut self, line: &str) {
        if self.outcome.is_some() || self.cancelled {
            return;
        }
        match output::parse(line) {
            Err(_) => self.fail(MALFORMED_OUTPUT),
            Ok(Line::Init {
                conversation_id,
                permission_mode,
                agent,
            }) => {
                if self.initialized {
                    return self.fail(BOUNDARY_VIOLATION);
                }

                // Capture and persist the provider transcript ID before any
                // boundary check. Even an unsafe init can already have written
                // the prompt/page context to brain/<id>.
                self.antigravity_conversation = Some(conversation_id.clone());
                self.register_cleanup_id(conversation_id);

                if agent != self.expected_agent {
                    return self.fail(AGENT_NOT_USED);
                }
                if !matches!(
                    permission_mode.as_str(),
                    "request-review" | "proceed-in-sandbox" | "strict"
                ) {
                    return self.fail(PERMISSIONS_TOO_OPEN);
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
            Ok(Line::AgentDelta { index, text, done }) => {
                if let Some(index) = index {
                    self.answer_steps.insert(index);
                }
                self.answer_delta(index, text, done);
            }
            Ok(Line::Untyped { index, text, done }) => {
                if index.is_some_and(|index| self.answer_steps.contains(&index)) {
                    self.answer_delta(index, text, done);
                } else if !self.initialized {
                    self.fail(MALFORMED_OUTPUT);
                } else {
                    self.queue.push_back(Update::Activity);
                }
            }
            Ok(Line::OtherStep { .. }) => {
                // The prompt echoed back, a system message, or an unclassified
                // step: not answer text, and nothing Pervue lets act. The
                // prompt is echoed before or after `init`, so either is fine.
                self.queue.push_back(Update::Activity);
            }
            Ok(Line::Tool(tool)) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search && tool == "search_web" {
                    // Text immediately before any search is narration, not
                    // durable answer/history.
                    self.held.clear();
                    self.held_step_done = false;
                    self.searched = true;
                    self.queue.push_back(Update::Activity);
                } else {
                    self.fail(BOUNDARY_VIOLATION);
                }
            }
            Ok(Line::Subagent | Line::UnexpectedStep) => self.fail(BOUNDARY_VIOLATION),
            Ok(Line::ResultSuccess {
                conversation_id,
                response,
            }) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if conversation_id
                    .as_deref()
                    .is_some_and(|id| self.antigravity_conversation.as_deref() != Some(id))
                {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search {
                    self.flush_held();
                }
                if !self.saw_delta && (!self.native_search || self.searched) && !response.is_empty()
                {
                    if self.native_search {
                        self.show_search_message(response);
                    } else {
                        self.show_text(response);
                    }
                }
                if self.native_search && self.searched {
                    for result in codex_message_sources(&self.answer) {
                        if let Some(source) = self.sources.push(result) {
                            self.queue.push_back(Update::Source(source));
                        }
                    }
                }
                let outcome = if self.native_search && (!self.searched || self.sources.count() == 0)
                {
                    Err(NATIVE_SEARCH_NO_SOURCES)
                } else {
                    Ok(())
                };
                self.outcome = Some(outcome);
                self.finish_by = Some(private_fs::after(FINISH_GRACE));
            }
            Ok(Line::ResultFailed(error)) => self.fail(error),
            Ok(Line::Ignored) => {}
        }
    }

    /// Answer text from agent-response step `index`.
    fn answer_delta(&mut self, index: Option<u64>, text: String, done: bool) {
        if !self.initialized {
            return self.fail(MALFORMED_OUTPUT);
        }
        if index != self.answer_step {
            self.answer_step = index;
            self.step_text.clear();
        }
        // A DONE update normally carries the last fragment; one that repeats
        // the step's whole text shows only what's new, never a second copy.
        let text = if done && !self.step_text.is_empty() && text.starts_with(&self.step_text) {
            text[self.step_text.len()..].to_owned()
        } else {
            text
        };
        if done {
            self.answer_step = None;
            self.step_text.clear();
        } else {
            self.step_text.push_str(&text);
        }
        if !self.native_search {
            self.show_text(text);
        } else {
            // Each response step is held until the next step. If a search
            // follows, it was narration and is dropped. If a second response
            // starts, the previous one was answer text.
            if self.held_step_done {
                self.flush_held();
            }
            self.held.push_str(&text);
            self.held_step_done = done;
        }
    }

    fn flush_held(&mut self) {
        self.held_step_done = false;
        if self.held.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.held);
        self.show_search_message(text);
    }

    fn show_search_message(&mut self, mut text: String) {
        if text.is_empty() {
            return;
        }
        if self.messages > 0 && !self.answer.is_empty() {
            text.insert_str(0, "\n\n");
        }
        self.messages += 1;
        self.show_text(text);
    }

    fn show_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.saw_delta = true;
        self.answer.push_str(&text);
        self.queue.push_back(Update::Delta(text));
    }

    /// Removes provider transcripts only after the child is gone. When init
    /// was never consumed (stop/malformed output), identify Pervue-owned
    /// transcripts by the unique private workspace path recorded in them.
    fn cleanup_runtime(&mut self) {
        let mut ids = Vec::new();
        if let Some(id) = self.antigravity_conversation.clone() {
            ids.push(id);
        } else if let (Some(home), Some(workspace)) = (&self.home, &self.workspace) {
            if let Ok(found) = antigravity_transcripts_for_workspace(home, workspace.path()) {
                ids.extend(found);
            }
        }

        ids.sort();
        ids.dedup();
        for id in &ids {
            self.register_cleanup_id(id.clone());
        }

        let mut removed = HashSet::new();
        if let Some(home) = &self.home {
            for id in &ids {
                if remove_antigravity_transcript(home, id).is_ok() {
                    removed.insert(id.clone());
                }
            }
        }
        self.clear_cleanup_ids(&removed);
        self.workspace.take();
    }

    fn ended(&mut self, exit: &Exit) -> Update {
        self.cleanup_runtime();
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
                    self.outcome = Some(Err(MALFORMED_OUTPUT));
                    self.cleanup_runtime();
                    let update = if self.cancelled {
                        Update::Stopped
                    } else {
                        Update::Failed(MALFORMED_OUTPUT)
                    };
                    self.done = true;
                    return Some(update);
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

fn agy_args(agent: &str, model: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--input-format"),
        OsString::from("stream-json"),
        OsString::from("--output-format"),
        OsString::from("stream-json"),
        OsString::from("--print-timeout"),
        OsString::from("300s"),
        OsString::from("--sandbox"),
        OsString::from("--agent"),
        OsString::from(agent),
    ];
    if let Some(model) = model {
        args.extend([OsString::from("--model"), OsString::from(model)]);
    }
    args
}

fn remove_antigravity_transcript(home: &Path, id: &str) -> io::Result<()> {
    if !output::is_conversation_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe Antigravity conversation id",
        ));
    }
    forget::remove(
        &home
            .join(".gemini")
            .join("antigravity-cli")
            .join("brain")
            .join(id),
    )
}

const MAX_PENDING_CLEANUPS: usize = 256;
const TRANSCRIPT_SCAN_BUDGET: usize = 512;
const TRANSCRIPT_SCAN_BYTES: u64 = 1024 * 1024;

fn private_fs::is_conversation_id(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("conv_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn installed_cleanup_dir() -> Option<PathBuf> {
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
        .map(|path| path.join("pervue/gemini-cleanups"))
}

fn cleanup_conversation_dir(base: &Path, conversation_id: &str) -> io::Result<PathBuf> {
    if !private_fs::is_conversation_id(conversation_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe Pervue conversation id",
        ));
    }
    Ok(base.join(conversation_id))
}

fn record_pending_cleanup_id(base: &Path, conversation_id: &str, id: &str) -> io::Result<()> {
    if !output::is_conversation_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe Antigravity conversation id",
        ));
    }
    let dir = cleanup_conversation_dir(base, conversation_id)?;
    private_fs::create_private_dir(&dir)?;
    private_fs::write_private_file(&dir.join(id), b"pending\n")
}

fn read_pending_cleanup_ids(base: &Path, conversation_id: &str) -> io::Result<Vec<String>> {
    let dir = cleanup_conversation_dir(base, conversation_id)?;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    for entry in entries.take(MAX_PENDING_CLEANUPS) {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if output::is_conversation_id(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn forget_cleanup_id_record(base: &Path, conversation_id: &str, id: &str) -> io::Result<()> {
    if !output::is_conversation_id(id) {
        return Ok(());
    }
    let dir = cleanup_conversation_dir(base, conversation_id)?;
    forget::remove(&dir.join(id))
}

fn forget_cleanup_record(base: &Path, conversation_id: &str) -> io::Result<()> {
    let dir = cleanup_conversation_dir(base, conversation_id)?;
    forget::remove(&dir)
}

fn antigravity_brain(home: &Path) -> PathBuf {
    home.join(".gemini").join("antigravity-cli").join("brain")
}

/// Finds only transcripts that prove they came from this turn's unique
/// Pervue-owned workspace. This is the fallback when cancellation/malformed
/// output prevents the adapter from consuming Antigravity's init event.
fn antigravity_transcripts_for_workspace(home: &Path, workspace: &Path) -> io::Result<Vec<String>> {
    let brain = antigravity_brain(home);
    let entries = match fs::read_dir(&brain) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let raw = workspace.to_string_lossy().into_owned();
    let escaped = serde_json::to_string(&raw)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned();
    let mut ids = Vec::new();
    for entry in entries.take(MAX_PENDING_CLEANUPS) {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !output::is_conversation_id(&id) {
            continue;
        }
        let mut budget = TRANSCRIPT_SCAN_BUDGET;
        if transcript_tree_mentions(&entry.path(), &raw, &escaped, 6, &mut budget)? {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn transcript_tree_mentions(
    dir: &Path,
    raw: &str,
    escaped: &str,
    depth: usize,
    budget: &mut usize,
) -> io::Result<bool> {
    if depth == 0 || *budget == 0 {
        return Ok(false);
    }
    for entry in fs::read_dir(dir)? {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if transcript_tree_mentions(&entry.path(), raw, escaped, depth - 1, budget)? {
                return Ok(true);
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        fs::File::open(entry.path())?
            .take(TRANSCRIPT_SCAN_BYTES)
            .read_to_end(&mut bytes)?;
        let text = String::from_utf8_lossy(&bytes);
        if text.contains(raw) || (!escaped.is_empty() && text.contains(escaped)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn private_fs::new_conversation_id() -> String {
    format!(
        "conv_{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    )
}

fn private_fs::keep_tail(tail: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let bytes = &bytes[bytes.len().saturating_sub(limit)..];
    let excess = (tail.len() + bytes.len()).saturating_sub(limit);
    tail.drain(..excess);
    tail.extend_from_slice(bytes);
}

fn private_fs::after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_use_stream_json_a_private_agent_and_keep_prompt_out_of_argv() {
        let args = agy_args(SEARCH_AGENT, Some("gemini-3-pro"));
        let strings: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert!(
            strings
                .windows(2)
                .any(|pair| pair == ["--agent", SEARCH_AGENT])
        );
        assert!(
            strings
                .windows(2)
                .any(|pair| pair == ["--model", "gemini-3-pro"])
        );
        assert!(strings.contains(&std::borrow::Cow::Borrowed("--sandbox")));
        assert_eq!(
            strings.iter().filter(|arg| **arg == "stream-json").count(),
            2
        );
        assert!(
            !strings
                .iter()
                .any(|arg| arg.contains("Current user question"))
        );
    }

    #[test]
    fn capabilities_match_the_normalized_contract() {
        assert_eq!(CAPABILITIES.streaming, Capability::Supported);
        assert_eq!(CAPABILITIES.continuation, Capability::Supported);
        assert_eq!(CAPABILITIES.web_search, Capability::Supported);
        assert_eq!(CAPABILITIES.page_context, Capability::Supported);
        assert_eq!(CAPABILITIES.model_selection, Capability::Supported);
        assert_eq!(CAPABILITIES.cancellation, Capability::Supported);
    }

    #[test]
    fn agent_definitions_fail_closed_except_for_native_search() {
        assert!(PLAIN_AGENT_DEFINITION.contains("tools: []"));
        assert!(SEARCH_AGENT_DEFINITION.contains("  - search_web"));
        for definition in [PLAIN_AGENT_DEFINITION, SEARCH_AGENT_DEFINITION] {
            assert!(definition.contains("inheritCustomizations: false"));
            assert!(definition.contains("inheritMcp: false"));
            assert!(definition.contains("commandExecutionPolicy: \"off\""));
            assert!(definition.contains("mcpServers: []"));
            assert!(definition.contains("skills: []"));
            assert!(definition.contains("plugins: []"));
            assert!(definition.contains("rules: []"));
            assert!(definition.contains("agents: []"));
            assert!(definition.contains("hooks: []"));
        }
    }
}
