//! What the opt-in live smoke tests share: whether they run at all, and small
//! checks that keep credentials and the prompt out of anywhere they shouldn't be.
//!
//! A live test asks the real provider CLI installed on the machine. It is
//! switched on by an environment variable of its own. Unset (or `0`), as in
//! normal CI runs, the test passes at once. Set to `1`, it runs when the CLI is
//! installed and signed in, and is skipped, passing, when it isn't. Set to
//! `required`, as in a job set up with a sign-in, a missing or signed-out CLI
//! fails it instead.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use runtime_core::exchange::{Exchange, Update};

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

/// A scratch directory of this test run, under cargo's temporary directory.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a scratch directory");
    dir
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

/// The first file under `dir` that holds `marker`, looking at no more than
/// `MAX_FILES` files of at most `MAX_FILE_BYTES` each, and not following links.
pub fn find_marker(dir: &Path, marker: &str) -> Option<PathBuf> {
    const MAX_FILES: usize = 50_000;
    const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

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
                    return None;
                }
                if entry
                    .metadata()
                    .is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES)
                    && std::fs::read(&path).is_ok_and(|bytes| contains(&bytes, marker.as_bytes()))
                {
                    return Some(path);
                }
            }
        }
    }
    None
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
