//! Google Gemini adapter through Antigravity CLI ('agy') (PRO-08).
//!
//! Each Pervue turn is a one-shot Antigravity run in a private workspace with
//! a workspace-local agent. Ordinary turns have no tools; Web turns allow only
//! 'search_web'. Pervue sends bounded conversation history every turn instead
//! of depending on Antigravity's native continuation state, then removes the
//! Antigravity transcript once the child has exited.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
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
    message: "Antigravity tried to use a capability Pervue did not allow. The turn was stopped.",
    retryable: false,
};
const CLEANUP_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InternalError,
    reason: "PROVIDER_TRANSCRIPT_CLEANUP_FAILED",
    message: "Pervue couldn't remove Antigravity's temporary conversation data. Delete the conversation to retry cleanup.",
    retryable: true,
};
const UNKNOWN_CONVERSATION: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "UNKNOWN_CONVERSATION",
    message: "This conversation can't be continued because its bounded history is unavailable. Start a new conversation.",
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
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
            .current_dir(cwd)
    }
}

pub struct Gemini {
    search: SearchPath,
    launch: Rc<Launch>,
    timeouts: Timeouts,
    pending_cleanups: PendingCleanups,
}

impl Gemini {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        Self::new(
            discovery::installed(),
            workspace::default_for(&host, "antigravity"),
        )
    }

    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        Self {
            search,
            launch: Rc::new(Launch::new(work_dir, std::env::vars_os().collect())),
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
            Ok(process) => StatusCheck::Probing {
                process,
                give_up: after(STATUS_PROBE),
                stderr_tail: Vec::new(),
            },
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
        if request.conversation_id.is_some() && request.history.is_empty() {
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
            sources: SourceCollector::new(ID),
            outcome: None,
            finish_by: None,
            done: false,
        })
    }

    fn forget(&self, conversation_id: &str) -> Box<dyn Exchange> {
        let ids = self
            .pending_cleanups
            .borrow()
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        if ids.is_empty() {
            return Box::new(Scripted::new([Update::Completed]));
        }
        let Some(home) = self.launch.home.clone() else {
            return Box::new(Scripted::failed(forget::SESSION_FORGET_FAILED));
        };
        let pending = Rc::clone(&self.pending_cleanups);
        let conversation = conversation_id.to_owned();
        forget::in_background(
            move || {
                for id in &ids {
                    remove_antigravity_transcript(&home, id)?;
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
        let busy_until = deadline.max(after(BUSY_LIMIT));
        loop {
            match self {
                Self::Done(queue) => return queue.pop_front(),
                Self::Probing {
                    process,
                    give_up,
                    stderr_tail: _,
                } if Instant::now() >= *give_up => {
                    process.kill();
                    *self = Self::Done(VecDeque::from([
                        status_update(Availability::Unavailable, Authentication::Unknown),
                        Update::Completed,
                    ]));
                }
                Self::Probing {
                    process,
                    give_up,
                    stderr_tail,
                } => match process.next_event(deadline.min(*give_up)) {
                    Some(Event::Stdout(_)) => {}
                    Some(Event::Stderr(bytes)) => keep_tail(stderr_tail, &bytes, STDERR_TAIL_BYTES),
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
                    None if Instant::now() >= *give_up => {}
                    None => return None,
                },
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
        create_private_dir(&path)?;
        let agent = if search { SEARCH_AGENT } else { PLAIN_AGENT };
        let definition = if search {
            SEARCH_AGENT_DEFINITION
        } else {
            PLAIN_AGENT_DEFINITION
        };
        let agent_dir = path.join(".agents/agents").join(agent);
        create_private_dir(&agent_dir)?;
        write_private_file(&agent_dir.join("agent.md"), definition.as_bytes())?;
        let hooks_dir = path.join(".agents");
        create_private_dir(&hooks_dir)?;
        write_private_file(
            &hooks_dir.join("hooks.json"),
            b"{}\n",
        )?;
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

struct Turn {
    // Keep the process before the workspace so dropping a live turn stops the
    // child before its cwd is removed.
    stream: LineStream,
    workspace: Option<TurnWorkspace>,
    home: Option<PathBuf>,
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
                if self.initialized
                    || agent != self.expected_agent
                    || !matches!(
                        permission_mode.as_str(),
                        "request-review" | "proceed-in-sandbox" | "strict"
                    )
                {
                    return self.fail(BOUNDARY_VIOLATION);
                }
                self.initialized = true;
                self.antigravity_conversation = Some(conversation_id.clone());
                self.pending_cleanups
                    .borrow_mut()
                    .entry(self.conversation_id.clone())
                    .or_default()
                    .push(conversation_id);
                if self.announce_conversation {
                    self.queue
                        .push_back(Update::ConversationCreated(self.conversation_id.clone()));
                }
                self.queue.push_back(Update::Started {
                    conversation_id: Some(self.conversation_id.clone()),
                });
            }
            Ok(Line::AgentDelta(text)) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                // Search narration emitted before the first actual search is
                // intentionally dropped; it is neither an answer nor a source.
                if !self.native_search || self.searched {
                    self.show_text(text);
                }
            }
            Ok(Line::Tool(tool)) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search && tool == "search_web" {
                    self.searched = true;
                    self.queue.push_back(Update::Activity);
                } else {
                    self.fail(BOUNDARY_VIOLATION);
                }
            }
            Ok(Line::Subagent) => self.fail(BOUNDARY_VIOLATION),
            Ok(Line::Progress) => {
                if self.initialized {
                    self.queue.push_back(Update::Activity);
                }
            }
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
                if !self.saw_delta && (!self.native_search || self.searched) && !response.is_empty()
                {
                    self.show_text(response);
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
                self.finish_by = Some(after(FINISH_GRACE));
            }
            Ok(Line::ResultFailed(error)) => self.fail(error),
            Ok(Line::Ignored) => {}
        }
    }

    fn show_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.saw_delta = true;
        self.answer.push_str(&text);
        self.queue.push_back(Update::Delta(text));
    }

    fn cleanup_runtime(&mut self) -> io::Result<()> {
        let result = match (&self.home, &self.antigravity_conversation) {
            (_, None) => Ok(()),
            (Some(home), Some(id)) => remove_antigravity_transcript(home, id),
            (None, Some(_)) => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "home directory unavailable for Antigravity transcript cleanup",
            )),
        };
        if result.is_ok() {
            if let Some(id) = &self.antigravity_conversation {
                let mut pending = self.pending_cleanups.borrow_mut();
                let remove_entry = if let Some(ids) = pending.get_mut(&self.conversation_id) {
                    ids.retain(|candidate| candidate != id);
                    ids.is_empty()
                } else {
                    false
                };
                if remove_entry {
                    pending.remove(&self.conversation_id);
                }
            }
        }
        self.workspace.take();
        result
    }

    fn ended(&mut self, exit: &Exit) -> Update {
        if self.cancelled {
            let _ = self.cleanup_runtime();
            return Update::Stopped;
        }
        if self.cleanup_runtime().is_err() {
            return Update::Failed(CLEANUP_FAILED);
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
                    self.outcome = Some(Err(MALFORMED_OUTPUT));
                    let update = if self.cleanup_runtime().is_err() {
                        Update::Failed(CLEANUP_FAILED)
                    } else if self.cancelled {
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

fn new_conversation_id() -> String {
    format!(
        "conv_{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    )
}

fn search_path_for(executable: &Path, inherited: Option<&OsStr>) -> OsString {
    let mut paths = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(inherited) = inherited {
        paths.extend(std::env::split_paths(inherited));
    }
    std::env::join_paths(paths).unwrap_or_default()
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
