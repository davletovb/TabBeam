//! Gemini CLI adapter (PRO-08).
//!
//! Pervue invokes Gemini in headless `stream-json` mode and keeps browser
//! content out of argv by writing the prompt on stdin. Each Pervue conversation
//! uses an opaque, safe Gemini session ID; follow-ups use `--resume`.
//! Provider-specific output is normalized here before it reaches the host.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::codex::workspace;
use super::discovery;
use super::environment;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::{SEARCH_INSTRUCTIONS, provider_prompt};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::search::{NATIVE_SEARCH_NO_SOURCES, SourceCollector, codex_message_sources};
use pervue_core::discovery::SearchPath;
use pervue_core::process::{Process, ProcessSpec};
use pervue_core::stream::{LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "gemini";
const EXECUTABLE: &str = "gemini";
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const STDERR_TAIL_BYTES: usize = 8 * 1024;

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
    message: "Gemini isn't installed. Install Gemini CLI, then try again.",
    retryable: false,
};
const START_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROVIDER_UNAVAILABLE",
    message: "Gemini couldn't start. Reinstall Gemini CLI, then try again.",
    retryable: false,
};
const NO_WORKSPACE: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "WORKSPACE_UNAVAILABLE",
    message: "Pervue couldn't prepare a private folder for Gemini. Check your cache folder, then try again.",
    retryable: false,
};
const PROCESS_EXITED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "PROCESS_EXITED",
    message: "Gemini stopped unexpectedly. Try again.",
    retryable: true,
};
const MALFORMED_OUTPUT: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderFailed,
    reason: "MALFORMED_PROVIDER_OUTPUT",
    message: "Gemini answered in a way Pervue doesn't understand. Update Gemini CLI and Pervue, then try again.",
    retryable: false,
};

#[derive(Debug, Clone)]
struct Launch {
    work_dir: PathBuf,
    inherited: Vec<(OsString, OsString)>,
    path: Option<OsString>,
}

impl Launch {
    fn new(work_dir: PathBuf, host: Vec<(OsString, OsString)>) -> Self {
        Self {
            work_dir,
            inherited: environment::inherit(host.clone(), &[]),
            path: environment::lookup(&host, "PATH").map(OsStr::to_os_string),
        }
    }

    fn workspace(&self) -> std::io::Result<PathBuf> {
        workspace::prepare(&self.work_dir)
    }

    fn command(
        &self,
        workspace: &Path,
        settings: &Path,
        executable: &Path,
        args: Vec<OsString>,
    ) -> ProcessSpec {
        ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            // System settings override user/project settings for the controls
            // Pervue must own: built-in tools, MCP, extensions, skills, hooks.
            .env("GEMINI_CLI_SYSTEM_SETTINGS_PATH", settings.as_os_str())
            .current_dir(workspace)
    }
}

pub struct Gemini {
    search: SearchPath,
    launch: Rc<Launch>,
    timeouts: Timeouts,
}

impl Gemini {
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        Self::new(
            discovery::installed(),
            workspace::default_for(&host, "gemini"),
        )
    }

    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
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
        let availability = if self.executable().is_some() {
            Availability::Available
        } else {
            Availability::NotFound
        };
        Box::new(Scripted::new([
            Update::Status {
                provider_id: ID.to_owned(),
                status: ProviderState {
                    availability,
                    // Gemini CLI currently exposes no side-effect-free auth
                    // status command. Asking performs the definitive check.
                    authentication: Authentication::Unknown,
                    capabilities: CAPABILITIES,
                    models: &[],
                },
            },
            Update::Completed,
        ]))
    }

    fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
        let Some(executable) = self.executable() else {
            return Box::new(Scripted::failed(NOT_INSTALLED));
        };
        let Ok(workspace) = self.launch.workspace() else {
            return Box::new(Scripted::failed(NO_WORKSPACE));
        };
        let settings = match write_settings(&workspace, request.native_search) {
            Ok(path) => path,
            Err(_) => return Box::new(Scripted::failed(NO_WORKSPACE)),
        };

        let new_conversation = request.conversation_id.is_none();
        let conversation_id = request.conversation_id.unwrap_or_else(new_conversation_id);
        let history = if new_conversation {
            &request.history[..]
        } else {
            &[][..]
        };
        let mut prompt = provider_prompt(history, request.context.as_ref(), &request.text);
        if request.native_search {
            prompt.insert_str(0, SEARCH_INSTRUCTIONS);
        }
        let args = gemini_args(
            &conversation_id,
            !new_conversation,
            request.model.as_deref(),
        );
        let spec = self
            .launch
            .command(&workspace, &settings, &executable, args);
        let Ok(mut process) = Process::spawn(&spec) else {
            return Box::new(Scripted::failed(START_FAILED));
        };
        if process.write(prompt.as_bytes()).is_err() {
            process.kill();
            return Box::new(Scripted::failed(START_FAILED));
        }
        process.close_stdin();

        Box::new(Turn {
            stream: LineStream::new(process, MAX_LINE_BYTES).keeping_stderr_tail(STDERR_TAIL_BYTES),
            queue: VecDeque::new(),
            conversation_id,
            announce_conversation: new_conversation,
            initialized: false,
            announced: false,
            cancelled: false,
            native_search: request.native_search,
            answer: String::new(),
            sources: SourceCollector::new(ID),
            outcome: None,
        })
    }
}

struct Turn {
    stream: LineStream,
    queue: VecDeque<Update>,
    conversation_id: String,
    announce_conversation: bool,
    initialized: bool,
    announced: bool,
    cancelled: bool,
    native_search: bool,
    answer: String,
    sources: SourceCollector,
    outcome: Option<Result<(), ErrorBody<'static>>>,
}

impl Turn {
    fn fail(&mut self, error: ErrorBody<'static>) {
        self.outcome = Some(Err(error));
    }

    fn on_line(&mut self, line: &str) {
        if self.outcome.is_some() || self.cancelled {
            return;
        }
        match output::parse(line) {
            Err(_) => self.fail(MALFORMED_OUTPUT),
            Ok(Line::Init(session)) => {
                if self.initialized || session != self.conversation_id {
                    return self.fail(MALFORMED_OUTPUT);
                }
                self.initialized = true;
                if self.announce_conversation {
                    self.queue
                        .push_back(Update::ConversationCreated(self.conversation_id.clone()));
                }
                self.announced = true;
                self.queue.push_back(Update::Started {
                    conversation_id: Some(self.conversation_id.clone()),
                });
            }
            Ok(Line::AssistantDelta(text)) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search {
                    self.answer.push_str(&text);
                }
                if !text.is_empty() {
                    self.queue.push_back(Update::Delta(text));
                }
            }
            Ok(Line::WebSearch) => self.queue.push_back(Update::Activity),
            Ok(Line::Progress) => self.queue.push_back(Update::Activity),
            Ok(Line::ResultSuccess) => {
                if !self.initialized {
                    return self.fail(MALFORMED_OUTPUT);
                }
                if self.native_search {
                    for result in codex_message_sources(&self.answer) {
                        if let Some(source) = self.sources.push(result) {
                            self.queue.push_back(Update::Source(source));
                        }
                    }
                    if self.sources.count() == 0 {
                        self.outcome = Some(Err(NATIVE_SEARCH_NO_SOURCES));
                        return;
                    }
                }
                self.outcome = Some(Ok(()));
            }
            Ok(Line::ResultFailed(error)) => self.fail(error),
            Ok(Line::Ignored) => {}
        }
    }

    fn finished(&mut self) -> Update {
        if self.cancelled {
            return Update::Stopped;
        }
        match self.outcome.take() {
            Some(Ok(())) => Update::Completed,
            Some(Err(error)) => Update::Failed(error),
            None if self.announced => Update::Failed(PROCESS_EXITED),
            None => Update::Failed(PROCESS_EXITED),
        }
    }
}

impl Exchange for Turn {
    fn next(&mut self, deadline: Instant) -> Option<Update> {
        loop {
            if let Some(update) = self.queue.pop_front() {
                return Some(update);
            }
            match self.stream.next(deadline)? {
                Output::Line(line) => self.on_line(&line),
                Output::Final(exit) => {
                    if !exit.status.is_some_and(|status| status.success()) && self.outcome.is_none()
                    {
                        self.fail(PROCESS_EXITED);
                    }
                    return Some(self.finished());
                }
                Output::Error(_) => return Some(Update::Failed(MALFORMED_OUTPUT)),
                Output::Stopped(_) => return Some(Update::Stopped),
            }
        }
    }

    fn cancel(&mut self, grace: Duration) {
        self.cancelled = true;
        self.queue.clear();
        self.stream.cancel(grace);
    }
}

fn gemini_args(session: &str, resume: bool, model: Option<&str>) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("--output-format"),
        OsString::from("stream-json"),
    ];
    if let Some(model) = model {
        args.push(format!("--model={model}").into());
    }
    if resume {
        args.extend([OsString::from("--resume"), OsString::from(session)]);
    } else {
        args.extend([OsString::from("--session-id"), OsString::from(session)]);
    }
    args
}

fn write_settings(workspace: &Path, search: bool) -> std::io::Result<PathBuf> {
    let name = if search {
        ".pervue-gemini-search.json"
    } else {
        ".pervue-gemini-plain.json"
    };
    let path = workspace.join(name);
    let tools = if search {
        r#"["google_web_search"]"#
    } else {
        "[]"
    };
    let content = format!(
        r#"{{"tools":{{"core":{tools}}},"hooksConfig":{{"enabled":false}},"admin":{{"mcp":{{"enabled":false}},"extensions":{{"enabled":false}},"skills":{{"enabled":false}}}}}}"#
    );
    // The workspace is private and these files are immutable in meaning. A
    // concurrent request writes identical bytes to the same selected file.
    std::fs::write(&path, content)?;
    Ok(path)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_keep_prompt_out_of_argv_and_session_safe() {
        let args = gemini_args("conv_123", false, Some("gemini-3.5-flash"));
        let strings: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
        assert!(strings.contains(&std::borrow::Cow::Borrowed("--session-id")));
        assert!(strings.contains(&std::borrow::Cow::Borrowed("conv_123")));
        assert!(strings.contains(&std::borrow::Cow::Borrowed("--model=gemini-3.5-flash")));
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
}
