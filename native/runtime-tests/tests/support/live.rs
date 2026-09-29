//! What the opt-in live smoke tests share: whether they run at all, and small
//! checks that keep credentials and the prompt out of anywhere they shouldn't be.
//!
//! A live test asks the real provider CLI installed on the machine. It is
//! switched on by an environment variable of its own. Unset (or `0`), as in
//! normal CI runs, the test passes at once. Set to `1`, it runs when the CLI is
//! installed and signed in, and is skipped, passing, when it isn't. Set to
//! `required`, as in a job set up with a sign-in, a missing or signed-out CLI
//! fails it instead.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use runtime_core::exchange::{Exchange, Update};
use runtime_core::protocol::Availability;
use runtime_providers::Provider;

/// How long a real model may take to answer.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a status check may take.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Run when the CLI is installed and signed in; skip otherwise.
    IfReady,
    /// Run, and fail when the CLI isn't installed or signed in.
    Required,
}

/// The mode the environment variable `variable` asks for.
pub fn mode(variable: &str) -> Mode {
    match std::env::var(variable).as_deref() {
        Err(_) | Ok("" | "0") => Mode::Off,
        Ok("required") => Mode::Required,
        Ok(_) => Mode::IfReady,
    }
}

/// Why the test can't run here, when the CLI isn't ready: a failure in
/// `required` mode, a note otherwise.
pub fn skip_or_fail(variable: &str, mode: Mode, reason: &str) {
    assert_ne!(mode, Mode::Required, "{variable}=required: {reason}");
    eprintln!("skipped: {reason}");
}

/// Pulls updates until a terminal one, allowing `timeout` for all of them.
pub fn run_within(exchange: &mut dyn Exchange, timeout: Duration) -> Vec<Update> {
    let deadline = Instant::now() + timeout;
    let mut updates = Vec::new();
    loop {
        let update = exchange
            .next(deadline)
            .unwrap_or_else(|| panic!("the exchange should end within {timeout:?}: {updates:?}"));
        let terminal = update.is_terminal();
        updates.push(update);
        if terminal {
            return updates;
        }
    }
}

/// The provider's status, asked again after a moment when it says the CLI is
/// unavailable: a status probe can need the network (Antigravity's fetches its
/// models), and one blip must not fail a `required` run.
pub fn status_of(provider: &dyn Provider) -> Vec<Update> {
    const ATTEMPTS: u32 = 3;
    let mut updates = Vec::new();
    for attempt in 1..=ATTEMPTS {
        updates = run_within(provider.status().as_mut(), STATUS_TIMEOUT);
        let unavailable = matches!(
            updates.first(),
            Some(Update::Status { status, .. }) if status.availability == Availability::Unavailable
        );
        if !unavailable || attempt == ATTEMPTS {
            break;
        }
        eprintln!("the status probe said unavailable (attempt {attempt}); asking again");
        std::thread::sleep(Duration::from_secs(3));
    }
    updates
}

/// Panics unless the turn ended with `Completed`, and says how it did end: the
/// terminal update and how many updates came before it, not all of them.
pub fn assert_completed(what: &str, updates: &[Update]) {
    match updates.last() {
        Some(Update::Completed) => {}
        Some(other) => panic!(
            "{what} ended with {other:?} after {} updates",
            updates.len()
        ),
        None => panic!("{what} gave no updates"),
    }
}

/// A scratch directory of this test run, under cargo's temporary directory,
/// removed when it goes out of scope, a panic included.
pub struct Scratch(PathBuf);

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn scratch(name: &str) -> Scratch {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a scratch directory");
    Scratch(dir)
}

/// The user's home directory, where the provider CLIs keep their own files.
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(unix) { "HOME" } else { "USERPROFILE" }).map(PathBuf::from)
}

/// A string no earlier run could have written, to look for afterwards.
pub fn marker() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("marker-{}-{nanos}", std::process::id())
}

/// What a search for a marker under a directory found.
#[derive(Debug, PartialEq, Eq)]
pub struct Scan {
    /// The first file that holds it.
    pub found: Option<PathBuf>,
    /// Whether the search left files unread, because there were too many or
    /// they were too large: not finding it then proves less.
    pub incomplete: bool,
}

/// Looks for `marker` in the files under `dir`, without following links, and
/// reads at most `MAX_FILES` files of at most `MAX_FILE_BYTES` each. It looks
/// everywhere, not only where a provider is known to keep its files, because
/// what it is for is finding where a provider keeps them that nobody knew.
pub fn find_marker(dir: &Path, marker: &str) -> Scan {
    const MAX_FILES: usize = 20_000;
    const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

    let mut scan = Scan {
        found: None,
        incomplete: false,
    };
    let mut pending = vec![dir.to_path_buf()];
    let mut looked_at = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                looked_at += 1;
                if looked_at > MAX_FILES {
                    scan.incomplete = true;
                    return scan;
                }
                if entry
                    .metadata()
                    .is_ok_and(|meta| meta.len() > MAX_FILE_BYTES)
                {
                    scan.incomplete = true;
                    continue;
                }
                if std::fs::read(&path).is_ok_and(|bytes| contains(&bytes, marker.as_bytes())) {
                    scan.found = Some(path);
                    return scan;
                }
            }
        }
    }
    scan
}

/// Panics if `marker` is anywhere under `dir`, and says so when it could not
/// look everywhere.
pub fn assert_no_marker(what: &str, dir: &Path, marker: &str) {
    let scan = find_marker(dir, marker);
    assert_eq!(scan.found, None, "{what}: a file holds the prompt");
    if scan.incomplete {
        eprintln!(
            "warning: {what}: {} is too large to be searched in full, so the prompt may be in a file that was not read",
            dir.display()
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Panics, without printing it, if `text` holds a credential: the value of one
/// of `variables` in this environment, or anything shaped like an API key.
pub fn assert_no_credentials(what: &str, text: &str, variables: &[&str]) {
    for name in variables {
        if let Ok(value) = std::env::var(name) {
            assert!(
                value.len() < 8 || !text.contains(&value),
                "{what}: the value of {name} appears"
            );
        }
    }
    // A run of key characters after a prefix that keys of the providers use.
    for prefix in ["sk-", "xai-", "AIza"] {
        let shaped_like_a_key = text.match_indices(prefix).any(|(start, _)| {
            text[start + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .count()
                >= 16
        });
        assert!(
            !shaped_like_a_key,
            "{what}: something shaped like an API key appears"
        );
    }
}

/// The text of the answer in `updates`, checked for credentials before anyone
/// prints it: a model's words are the one thing a test prints that no test
/// wrote.
pub fn answer_of(what: &str, updates: &[Update], variables: &[&str]) -> String {
    assert_no_credentials(what, &format!("{updates:?}"), variables);
    updates
        .iter()
        .filter_map(|update| match update {
            Update::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
