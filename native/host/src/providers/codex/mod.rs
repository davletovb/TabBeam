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

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::discovery::SearchPath;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::conversation::normalized_prompt;
use crate::process::{Event, Exit, Process, ProcessSpec};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::stream::{LineStream, Output};

pub mod output;

use output::Line;

pub const ID: &str = "codex";

/// The executable the adapter looks for.
const EXECUTABLE: &str = "codex";

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

/// What this adapter supports. Answers arrive a message at a time, not
/// token by token; page context, attachments, and model selection are not
/// passed to Codex yet.
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
    message: "Codex isn't installed. Install the Codex CLI, then try again.",
    retryable: false,
};

const NOT_SIGNED_IN: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::ProviderNotAuthenticated,
    reason: "LOGIN_REQUIRED",
    message: "Codex isn't signed in. Run \"codex login\" in a terminal, then try again.",
    retryable: false,
};

const PAGE_CONTEXT_UNSUPPORTED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InvalidRequest,
    reason: "PAGE_CONTEXT_UNSUPPORTED",
    message: "Codex can't use page context yet. Choose No context, then ask again.",
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
    message: "Codex couldn't be started.",
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
    message: "Codex answered in a form Pervue doesn't understand.",
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

/// The Codex CLI adapter.
pub struct Codex {
    search: SearchPath,
    work_dir: PathBuf,
    session_dir: PathBuf,
    limits: Limits,
    conversations: Conversations,
}

impl Codex {
    /// The adapter of an installed host: the platform lookup rules, and an
    /// empty working directory under the system temporary directory.
    pub fn installed() -> Self {
        let mut codex = Self::new(
            SearchPath::from_env(),
            std::env::temp_dir().join("pervue-codex"),
        );
        codex.session_dir = installed_session_dir();
        codex
    }

    /// Looks for `codex` in `search`, and runs it from `work_dir`.
    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        Self {
            search,
            session_dir: work_dir.join("sessions"),
            work_dir,
            limits: LIMITS,
            conversations: Rc::default(),
        }
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
        self.session_dir = session_dir;
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
        Box::new(match probe(&executable) {
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
        // Codex doesn't receive browser context yet (`page_context: false`).
        // Answering without it would silently ignore what the user attached.
        if request.has_context {
            return Box::new(Scripted::failed(PAGE_CONTEXT_UNSUPPORTED));
        }
        let mut conversation_id = request.conversation_id;
        let mut prompt = request.text;
        let resume = match &conversation_id {
            None => None,
            Some(conversation_id) => match self
                .conversations
                .borrow()
                .get(conversation_id)
                .cloned()
                .or_else(|| read_thread(&self.session_dir, conversation_id))
            {
                Some(thread_id) => Some(thread_id),
                None if !request.history.is_empty() => None,
                None => return Box::new(Scripted::failed(UNKNOWN_CONVERSATION)),
            },
        };
        if resume.is_none() && !request.history.is_empty() {
            // If a provider has no native session (or its mapping was lost),
            // the ordered, bounded dialogue still reaches the new turn.
            prompt = normalized_prompt(&request.history, &prompt);
            conversation_id = None;
        }
        let mut turn = Turn {
            stage: Stage::Done,
            executable,
            work_dir: self.work_dir.clone(),
            session_dir: self.session_dir.clone(),
            prompt,
            resume,
            conversation_id,
            conversations: Rc::clone(&self.conversations),
            finish_grace: self.limits.finish,
            queue: VecDeque::new(),
            cancelled: false,
            thread_id: None,
            started: false,
            messages: 0,
            outcome: None,
            finish_by: None,
        };
        match probe(&turn.executable) {
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
fn probe(executable: &Path) -> std::io::Result<Process> {
    let spec = ProcessSpec::new(executable)
        .args(["login", "status"])
        .env("PATH", search_path_for(executable));
    let mut process = Process::spawn(&spec)?;
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
fn search_path_for(executable: &Path) -> OsString {
    let inherited = std::env::var_os("PATH");
    let dirs = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(inherited.iter().flat_map(std::env::split_paths));
    std::env::join_paths(dirs).unwrap_or_else(|_| inherited.unwrap_or_default())
}

fn after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}

fn installed_session_dir() -> PathBuf {
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
        .unwrap_or_else(|| std::env::temp_dir().join("pervue-data"))
        .join("pervue/codex-sessions")
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
        loop {
            let authentication = match self {
                Self::Done(updates) => return updates.pop_front(),
                Self::Probing { process, give_up } => {
                    match process.next_event(deadline.min(*give_up)) {
                        Some(Event::Exited(exit)) => signed_in(&exit),
                        // The probe's output names the account: never read.
                        Some(Event::Stdout(_) | Event::Stderr(_)) => continue,
                        None if Instant::now() >= *give_up => {
                            process.kill();
                            Authentication::Unknown
                        }
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
    work_dir: PathBuf,
    session_dir: PathBuf,
    prompt: String,
    /// The Codex thread to resume, when continuing a conversation.
    resume: Option<String>,
    conversation_id: Option<String>,
    conversations: Conversations,
    finish_grace: Duration,
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
        let mut spec = ProcessSpec::new(&self.executable)
            .args([
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "-C",
            ])
            .arg(&self.work_dir);
        if let Some(thread_id) = &self.resume {
            spec = spec.args(["resume", thread_id]);
        }
        let spec = spec.arg("-").env("PATH", search_path_for(&self.executable));
        let _ = std::fs::create_dir_all(&self.work_dir);

        match Process::spawn(&spec) {
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
                if save_thread(&self.session_dir, &conversation_id, &thread_id).is_err() {
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
        loop {
            if let Some(update) = self.queue.pop_front() {
                return Some(update);
            }
            match &mut self.stage {
                Stage::Done => return None,
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
                        None if Instant::now() >= *give_up => {
                            process.kill();
                            self.start();
                        }
                        None => return None,
                    }
                }
                Stage::Running(stream) => {
                    let wait = self
                        .finish_by
                        .map_or(deadline, |finish_by| deadline.min(finish_by));
                    match stream.next(wait) {
                        Some(Output::Line(line)) => self.on_line(&line),
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
