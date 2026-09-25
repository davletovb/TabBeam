//! Provider process manager (NAT-04).
//!
//! [`Process`] starts a provider executable from an absolute path and an
//! argument array, with no shell and no `PATH` search, and supervises it until
//! it has exited and been reaped:
//!
//! - stdin, stdout, and stderr are three separate pipes, so a provider never
//!   shares the host's own Native Messaging streams;
//! - output arrives in chunks of at most [`MAX_CHUNK_BYTES`], in the order each
//!   stream produced it, with at most [`MAX_QUEUED_CHUNKS`] read ahead of the
//!   caller;
//! - input is written by a helper thread, so a provider that isn't reading
//!   can't block the caller;
//! - [`Process::terminate`] asks the process to stop and kills it when the
//!   grace period runs out, and [`Process::kill`] kills it at once;
//! - dropping a [`Process`] kills and reaps it.
//!
//! On POSIX the child leads a new process group. Stopping it signals the whole
//! group, so the processes it started stop too, and when it exits on its own
//! the host kills any it left behind. On Windows only the child itself is
//! stopped so far; stopping its whole tree needs a Job Object (ADR-0001).

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Largest chunk of output read from a provider at once.
pub const MAX_CHUNK_BYTES: usize = 8 * 1024;

/// Output chunks read ahead of the caller, across stdout and stderr. Beyond
/// them, the reader threads wait, and so does a provider that keeps writing:
/// at most 128 KiB of a provider's output waits in the host.
pub const MAX_QUEUED_CHUNKS: usize = 16;

/// How often a waiting call checks whether the process has exited.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How long the host keeps reading after the process exited and its process
/// group was stopped. Only a descendant that left the group can hold the
/// output open that long.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// How long cleanup waits for helper threads to finish before leaving them to
/// end on their own.
const JOIN_TIMEOUT: Duration = Duration::from_millis(100);

/// An executable to start, the arguments to pass it, and any environment
/// variables to set for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

impl ProcessSpec {
    /// Starts `program`, which must be an absolute path. Finding a provider's
    /// executable is the provider registry's job (PRO-02), not the process
    /// manager's.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Appends one argument. It is passed as its own argv element: no shell
    /// parses or splits it.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends each of `args`, as [`ProcessSpec::arg`] does.
    #[must_use]
    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets an environment variable for the process. Everything else is
    /// inherited from the host until SEC-02 narrows it.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

/// Something a process did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Bytes the process wrote to stdout: at most [`MAX_CHUNK_BYTES`], cut
    /// wherever a read happened to end.
    Stdout(Vec<u8>),
    /// Bytes the process wrote to stderr, chunked the same way.
    Stderr(Vec<u8>),
    /// The process exited and was reaped. Always the last event.
    Exited(Exit),
}

/// How a process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit status, or `None` if something outside the host reaped the
    /// process first.
    pub status: Option<ExitStatus>,
    pub ending: Ending,
    /// Whether stdout and stderr both reached end of file. `false` means a
    /// descendant outside the process group still held them open when the
    /// host stopped waiting, so output may be missing.
    pub output_closed: bool,
}

/// Who ended a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It exited on its own.
    Natural,
    /// It exited within the grace period after [`Process::terminate`] asked
    /// it to stop.
    Stopped,
    /// It was killed: by [`Process::kill`], by a drop, or when a grace period
    /// ran out.
    Killed,
}

/// A running provider process. See the module documentation.
pub struct Process {
    child: Child,
    /// Queues input for the stdin writer thread; `None` once stdin is closed.
    stdin: Option<Sender<Vec<u8>>>,
    output: Receiver<Output>,
    /// Output streams that haven't reached end of file.
    open_streams: usize,
    threads: Vec<JoinHandle<()>>,
    ending: Ending,
    state: State,
}

#[derive(Clone, Copy)]
enum State {
    Running,
    /// Reaped; the rest of its output is read until `drain_until`.
    Reaped {
        status: Option<ExitStatus>,
        drain_until: Instant,
    },
    Finished(Exit),
}

/// What the reader threads send.
enum Output {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// One stream reached end of file, or failed.
    Closed,
}

impl Process {
    /// Starts `spec` with piped stdin, stdout, and stderr.
    ///
    /// Fails with [`io::ErrorKind::InvalidInput`] if the program path isn't
    /// absolute, and with the operating system's error if the process can't
    /// start.
    pub fn spawn(spec: &ProcessSpec) -> io::Result<Self> {
        if !spec.program.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a provider executable path must be absolute",
            ));
        }

        // The one place the host starts a process (SEC-01).
        #[allow(clippy::disallowed_methods)]
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .envs(spec.env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tree::configure(&mut command);
        let mut child = command.spawn()?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (sender, output) = mpsc::sync_channel(MAX_QUEUED_CHUNKS);
        let mut process = Self {
            child,
            stdin: None,
            output,
            open_streams: 0,
            threads: Vec::new(),
            ending: Ending::Natural,
            state: State::Running,
        };

        // If a helper thread can't start, dropping `process` kills the child.
        if let Some(stdout) = stdout {
            process.start_reader("provider-stdout", stdout, Output::Stdout, sender.clone())?;
        }
        if let Some(stderr) = stderr {
            process.start_reader("provider-stderr", stderr, Output::Stderr, sender)?;
        }
        if let Some(stdin) = stdin {
            process.start_writer(stdin)?;
        }
        Ok(process)
    }

    /// The operating system's process ID. On POSIX it is also the ID of the
    /// process group the process leads.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Queues `bytes` for the process's stdin and returns without waiting for
    /// the process to read them.
    ///
    /// Fails with [`io::ErrorKind::BrokenPipe`] once stdin is closed: after
    /// [`Process::close_stdin`], after a stop, or once the process has stopped
    /// reading.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &self.stdin {
            Some(stdin) if stdin.send(bytes.to_vec()).is_ok() => Ok(()),
            _ => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// Closes stdin once the queued input is written, so the process reads end
    /// of file.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Returns the process's next event, waiting until `deadline` at most.
    ///
    /// All output comes before [`Event::Exited`], which is the last event:
    /// once it is returned, every later call returns it again. Returns `None`
    /// if the deadline passes first; that is how a caller times a process out
    /// before stopping it. A process whose output nobody pulls eventually
    /// blocks on its own writes.
    pub fn next_event(&mut self, deadline: Instant) -> Option<Event> {
        loop {
            if let State::Finished(exit) = self.state {
                return Some(Event::Exited(exit));
            }

            let now = Instant::now();
            if self.open_streams > 0 {
                let until = match self.state {
                    State::Reaped { drain_until, .. } => drain_until,
                    _ => now + POLL_INTERVAL,
                };
                let timeout = until.min(deadline).saturating_duration_since(now);
                match self.output.recv_timeout(timeout) {
                    Ok(Output::Stdout(bytes)) => return Some(Event::Stdout(bytes)),
                    Ok(Output::Stderr(bytes)) => return Some(Event::Stderr(bytes)),
                    Ok(Output::Closed) => self.open_streams = self.open_streams.saturating_sub(1),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => self.open_streams = 0,
                }
            } else if matches!(self.state, State::Running) {
                thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
            }

            self.poll_exit();
            if let State::Reaped {
                status,
                drain_until,
            } = self.state
            {
                if self.open_streams == 0 || Instant::now() >= drain_until {
                    self.finish(status);
                    continue;
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
        }
    }

    /// Asks the process to stop without waiting: closes stdin and, on POSIX,
    /// sends SIGTERM to the process group. Keep pulling events to see it exit,
    /// and call [`Process::kill`] if it outlives your grace period. A process
    /// that exits after this is reported as [`Ending::Stopped`].
    pub fn request_stop(&mut self) {
        self.stdin = None;
        self.poll_exit();
        if matches!(self.state, State::Running) {
            self.ending = Ending::Stopped;
            tree::request_stop(&self.child);
        }
    }

    /// Asks the process to stop, waits up to `grace` for it to exit, and then
    /// kills it. Output it writes after the request is discarded.
    ///
    /// The request closes stdin and, on POSIX, sends SIGTERM to the process
    /// group. On Windows, closing stdin is the only request.
    pub fn terminate(&mut self, grace: Duration) -> Exit {
        self.stop(Some(grace))
    }

    /// Kills the process, and on POSIX its process group, without asking
    /// first. Output it hasn't been pulled yet is discarded.
    pub fn kill(&mut self) -> Exit {
        self.stop(None)
    }

    fn stop(&mut self, grace: Option<Duration>) -> Exit {
        if let State::Finished(exit) = self.state {
            return exit;
        }
        self.stdin = None;
        self.poll_exit();

        if let (State::Running, Some(grace)) = (self.state, grace) {
            self.ending = Ending::Stopped;
            tree::request_stop(&self.child);
            let give_up = Instant::now().checked_add(grace);
            while matches!(self.state, State::Running) {
                let now = Instant::now();
                let remaining = match give_up {
                    Some(give_up) if now >= give_up => break,
                    Some(give_up) => give_up - now,
                    None => POLL_INTERVAL,
                };
                self.discard_output(remaining.min(POLL_INTERVAL));
                self.poll_exit();
            }
        }

        let (status, drain_until) = match self.state {
            State::Finished(exit) => return exit,
            State::Reaped {
                status,
                drain_until,
            } => (status, drain_until),
            State::Running => {
                self.ending = Ending::Killed;
                tree::force_stop(&mut self.child);
                let status = self.child.wait().ok();
                self.reaped(status)
            }
        };
        while self.open_streams > 0 && Instant::now() < drain_until {
            self.discard_output(drain_until.saturating_duration_since(Instant::now()));
        }
        self.finish(status)
    }

    fn start_reader<R: Read + Send + 'static>(
        &mut self,
        name: &str,
        stream: R,
        wrap: fn(Vec<u8>) -> Output,
        sender: SyncSender<Output>,
    ) -> io::Result<()> {
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || read_output(stream, wrap, &sender))?;
        self.threads.push(thread);
        self.open_streams += 1;
        Ok(())
    }

    fn start_writer(&mut self, mut stdin: ChildStdin) -> io::Result<()> {
        let (sender, inputs) = mpsc::channel::<Vec<u8>>();
        let thread = thread::Builder::new()
            .name("provider-stdin".to_owned())
            .spawn(move || {
                // Runs until stdin is closed or the process stops reading;
                // dropping `stdin` then closes the pipe.
                for bytes in inputs {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            })?;
        self.threads.push(thread);
        self.stdin = Some(sender);
        Ok(())
    }

    /// Reaps the process if it has exited.
    fn poll_exit(&mut self) {
        if !matches!(self.state, State::Running) {
            return;
        }
        match self.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                self.reaped(Some(status));
            }
            // Only something outside the host reaping the child fails this.
            Err(_) => {
                self.reaped(None);
            }
        }
    }

    /// Records that the process was reaped and stops whatever it left behind.
    /// Returns its status and how long to keep reading its output.
    fn reaped(&mut self, status: Option<ExitStatus>) -> (Option<ExitStatus>, Instant) {
        // Descendants left in the group would outlive the process as orphans,
        // and could hold its output open.
        tree::stop_leftovers(&self.child);
        let drain_until = Instant::now() + DRAIN_TIMEOUT;
        self.state = State::Reaped {
            status,
            drain_until,
        };
        (status, drain_until)
    }

    /// Waits up to `timeout` for output and throws it away.
    fn discard_output(&mut self, timeout: Duration) {
        if self.open_streams == 0 {
            thread::sleep(timeout);
            return;
        }
        match self.output.recv_timeout(timeout) {
            Ok(Output::Closed) => self.open_streams = self.open_streams.saturating_sub(1),
            Ok(Output::Stdout(_) | Output::Stderr(_)) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => self.open_streams = 0,
        }
    }

    fn finish(&mut self, status: Option<ExitStatus>) -> Exit {
        let exit = Exit {
            status,
            ending: self.ending,
            output_closed: self.open_streams == 0,
        };
        self.stdin = None;
        join_threads(&mut self.threads);
        self.state = State::Finished(exit);
        exit
    }
}

impl Drop for Process {
    /// Kills the process if it is still running and reaps it, so it never
    /// outlives its owner, even as a zombie.
    fn drop(&mut self) {
        self.kill();
    }
}

/// Forwards `stream` in chunks until end of file, then reports it closed.
/// Stops early once the [`Process`] is gone.
fn read_output(mut stream: impl Read, wrap: fn(Vec<u8>) -> Output, sender: &SyncSender<Output>) {
    let mut buffer = vec![0; MAX_CHUNK_BYTES];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if sender.send(wrap(buffer[..count].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let _ = sender.send(Output::Closed);
}

/// Joins the helper threads. A reader still blocked on output that an escaped
/// descendant holds open is left to finish when that output closes.
fn join_threads(threads: &mut Vec<JoinHandle<()>>) {
    let give_up = Instant::now() + JOIN_TIMEOUT;
    for handle in threads.drain(..) {
        while !handle.is_finished() && Instant::now() < give_up {
            thread::sleep(Duration::from_millis(1));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

/// Process-tree control through process groups (ADR-0001).
#[cfg(unix)]
mod tree {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    /// Makes the child lead a new process group, whose ID is its process ID.
    pub fn configure(command: &mut Command) {
        command.process_group(0);
    }

    /// Asks the whole group to stop.
    pub fn request_stop(child: &Child) {
        signal_group(child, Signal::SIGTERM);
    }

    /// Kills the whole group, the child included.
    pub fn force_stop(child: &mut Child) {
        signal_group(child, Signal::SIGKILL);
        let _ = child.kill();
    }

    /// Kills what is left of the group after the child was reaped. A process
    /// group ID stays reserved while any member is alive, so when the child
    /// left descendants, this reaches only them. When it left none, the ID is
    /// free again: a group that took it in the microseconds since the reaping
    /// would be hit instead, which takes process IDs wrapping around within
    /// that window. Signalling before reaping would close it, but seeing the
    /// exit without reaping needs `waitid` with `WNOWAIT`, which nix doesn't
    /// offer on macOS (SEC-02).
    pub fn stop_leftovers(child: &Child) {
        signal_group(child, Signal::SIGKILL);
    }

    fn signal_group(child: &Child, signal: Signal) {
        // Group 0 would be the host's own group, and 1 is init's.
        if let Ok(group) = i32::try_from(child.id()) {
            if group > 1 {
                let _ = killpg(Pid::from_raw(group), signal);
            }
        }
    }
}

/// On Windows only the child itself is controlled so far: stopping its whole
/// tree needs a Job Object (ADR-0001).
#[cfg(not(unix))]
mod tree {
    use std::process::{Child, Command};

    pub fn configure(_command: &mut Command) {}

    /// Closing stdin, which the caller does first, is the only request.
    pub fn request_stop(_child: &Child) {}

    pub fn force_stop(child: &mut Child) {
        let _ = child.kill();
    }

    pub fn stop_leftovers(_child: &Child) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_program_path_is_refused_before_anything_starts() {
        for program in ["pervue-fake-provider", "./pervue-fake-provider", "bin/sh"] {
            let error = Process::spawn(&ProcessSpec::new(program))
                .err()
                .expect("a relative path must be refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{program}");
        }
    }

    #[test]
    fn arguments_are_kept_as_separate_elements() {
        let spec =
            ProcessSpec::new("/opt/provider")
                .arg("--mode")
                .args(["a b", "$(id)", "; rm -rf /"]);
        assert_eq!(
            spec.args,
            ["--mode", "a b", "$(id)", "; rm -rf /"].map(OsString::from)
        );
    }
}
