//! The Codex CLI adapter: discovery and sign-in status (PRO-02), requests and
//! streaming (PRO-03), and cancellation, timeouts, and failures (PRO-04).
//!
//! A request runs `codex exec --json` in a read-only sandbox, from an empty
//! working directory, with the question on stdin. Its JSON lines become
//! protocol updates (see [`output`]). Codex's own session IDs stay inside the
//! adapter: each conversation gets a Pervue ID, mapped to the Codex thread it
//! continues with `codex exec resume`. The map lives in the host process, so
//! a conversation can be continued only while the host that started it runs;
//! conversation continuity is Milestone C.
//!
//! Before each request, `codex login status` checks the sign-in, because a
//! signed-out `codex exec` retries the network instead of failing. Only its
//! exit status is read: its output names the account and a masked key.
//!
//! Codex runs in a workspace only its user can reach, with a minimal
//! environment (SEC-02): the variables every provider gets
//! ([`environment::INHERITED`]), Codex's own settings, and a `PATH` that
//! starts with Codex's directory.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use super::discovery::SearchPath;
use super::environment;
use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::process::{Event, Exit, Process, ProcessSpec};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ErrorBody, ErrorCode, ProviderState,
};
use crate::stream::{BUSY_LIMIT, LineStream, Output};

pub mod output;

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
    message: "Codex couldn't start. Reinstall the Codex CLI, then try again.",
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

    /// `codex` with `args`, in the workspace, with only its environment. The
    /// workspace is created first, so no process starts without it.
    fn command<I>(&self, executable: &Path, args: I) -> std::io::Result<ProcessSpec>
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        prepare_workspace(&self.work_dir)?;
        Ok(ProcessSpec::new(executable)
            .args(args)
            .envs(self.inherited.iter().cloned())
            .env("PATH", search_path_for(executable, self.path.as_deref()))
            .current_dir(&self.work_dir))
    }
}

/// The Codex CLI adapter.
pub struct Codex {
    search: SearchPath,
    launch: Rc<Launch>,
    limits: Limits,
    conversations: Conversations,
}

impl Codex {
    /// The adapter of an installed host: the platform lookup rules, and an
    /// empty workspace in the user's own cache directory.
    pub fn installed() -> Self {
        let host: Vec<_> = std::env::vars_os().collect();
        Self::new(SearchPath::from_env(), workspace(&host))
    }

    /// Looks for `codex` in `search`, and runs it in `work_dir`, which it
    /// creates when needed, with variables from the host's environment.
    pub fn new(search: SearchPath, work_dir: PathBuf) -> Self {
        Self {
            search,
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

    fn executable(&self) -> Option<PathBuf> {
        self.search.find(EXECUTABLE)
    }
}

/// Where an installed host runs Codex: an empty directory in the user's own
/// cache, which other users can't write to. In a shared temporary directory,
/// such as `/tmp` on Linux, another user could plant files there, such as
/// instructions for Codex to follow. Without a cache directory, a new one
/// with a random name in the temporary directory.
fn workspace(host: &[(OsString, OsString)]) -> PathBuf {
    cache_dir(host).map_or_else(
        || {
            std::env::temp_dir().join(format!(
                "pervue-codex-{:016x}",
                RandomState::new().hash_one(SystemTime::now())
            ))
        },
        |cache| cache.join("codex-workspace"),
    )
}

/// Pervue's directory in the user's cache.
#[cfg(target_vendor = "apple")]
fn cache_dir(host: &[(OsString, OsString)]) -> Option<PathBuf> {
    absolute(host, "HOME").map(|home| home.join("Library/Caches/Pervue"))
}

/// Pervue's directory in the user's cache.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn cache_dir(host: &[(OsString, OsString)]) -> Option<PathBuf> {
    absolute(host, "XDG_CACHE_HOME")
        .map(|cache| cache.join("pervue"))
        .or_else(|| absolute(host, "HOME").map(|home| home.join(".cache/pervue")))
}

/// Pervue's directory in the user's cache.
#[cfg(not(unix))]
fn cache_dir(host: &[(OsString, OsString)]) -> Option<PathBuf> {
    absolute(host, "LOCALAPPDATA").map(|local| local.join("Pervue"))
}

/// The variable `name` as a path, if it is set to an absolute one.
fn absolute(host: &[(OsString, OsString)], name: &str) -> Option<PathBuf> {
    environment::lookup(host, name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Creates the workspace if needed. On POSIX, the directories created are
/// open to their user alone.
fn prepare_workspace(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
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
        // Codex doesn't receive browser context yet (`page_context: false`).
        // Answering without it would silently ignore what the user attached.
        if request.has_context {
            return Box::new(Scripted::failed(PAGE_CONTEXT_UNSUPPORTED));
        }
        let resume = match &request.conversation_id {
            None => None,
            Some(conversation_id) => match self.conversations.borrow().get(conversation_id) {
                Some(thread_id) => Some(thread_id.clone()),
                None => return Box::new(Scripted::failed(UNKNOWN_CONVERSATION)),
            },
        };
        let mut turn = Turn {
            stage: Stage::Done,
            executable,
            launch: Rc::clone(&self.launch),
            prompt: request.text,
            resume,
            conversation_id: request.conversation_id,
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
    let mut process = Process::spawn(&launch.command(executable, ["login", "status"])?)?;
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
        let mut args: Vec<OsString> = [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "-C",
        ]
        .map(OsString::from)
        .into();
        args.push(self.launch.work_dir.clone().into());
        if let Some(thread_id) = &self.resume {
            args.extend(["resume", thread_id].map(OsString::from));
        }
        args.push("-".into());
        let spawned = self
            .launch
            .command(&self.executable, args)
            .and_then(|spec| Process::spawn(&spec));

        match spawned {
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
                let mut conversations = self.conversations.borrow_mut();
                let conversation_id = new_conversation_id(&conversations);
                conversations.insert(conversation_id.clone(), thread_id);
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

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn the_workspace_is_in_the_users_cache() {
        assert_eq!(
            workspace(&vars(&[("HOME", "/Users/me")])),
            PathBuf::from("/Users/me/Library/Caches/Pervue/codex-workspace")
        );
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn the_workspace_is_in_the_users_cache() {
        assert_eq!(
            workspace(&vars(&[("HOME", "/home/me")])),
            PathBuf::from("/home/me/.cache/pervue/codex-workspace")
        );
        assert_eq!(
            workspace(&vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")])),
            PathBuf::from("/cache/pervue/codex-workspace")
        );
        // A relative cache directory would depend on the working directory.
        assert_eq!(
            workspace(&vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "cache")])),
            PathBuf::from("/home/me/.cache/pervue/codex-workspace")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn the_workspace_is_in_the_users_cache() {
        assert_eq!(
            workspace(&vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")])),
            PathBuf::from(r"C:\Users\me\AppData\Local\Pervue\codex-workspace")
        );
    }

    #[test]
    fn without_a_cache_the_workspace_gets_a_new_name() {
        let first = workspace(&vars(&[("HOME", "relative"), ("LOCALAPPDATA", "relative")]));
        let second = workspace(&[]);
        for dir in [&first, &second] {
            assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
            let name = dir.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("pervue-codex-"), "{name}");
        }
        assert_ne!(first, second);
    }

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

    #[cfg(unix)]
    #[test]
    fn the_workspace_is_created_for_its_user_alone() {
        use std::os::unix::fs::PermissionsExt;

        let root =
            std::env::temp_dir().join(format!("pervue-workspace-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("cache/codex-workspace");
        prepare_workspace(&dir).unwrap();
        for created in [&root.join("cache"), &dir] {
            let mode = std::fs::metadata(created).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", created.display());
        }
        // Already there: nothing to do.
        prepare_workspace(&dir).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
