//! Removing what a provider keeps for a deleted conversation
//! (`conversation.forget`, v1 §5.4).
//!
//! Only files a provider wrote for Pervue are removed: a transcript counts as
//! Pervue's when it names the session being forgotten and records Pervue's
//! private working directory as the place it ran. Nothing is followed through
//! a symbolic link: a link is removed itself, never what it points to.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::protocol::events::{ErrorBody, ErrorCode};

pub const SESSION_FORGET_FAILED: ErrorBody<'static> = ErrorBody {
    code: ErrorCode::InternalError,
    reason: "SESSION_FORGET_FAILED",
    message: "Pervue couldn't remove everything this conversation left behind. Delete it again to retry.",
    retryable: true,
};

/// How much of a transcript is read to find where it ran.
const HEAD_BYTES: u64 = 1024 * 1024;

/// The complete lines within the first megabyte of `path`, or `None` if it
/// can't be read as a regular file.
pub(crate) fn head_lines(path: &Path) -> Option<Vec<String>> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut head = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    let complete = head
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |end| end + 1);
    Some(
        String::from_utf8_lossy(&head[..complete])
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

/// Whether the working directory a provider `recorded` is `workspace`.
pub(crate) fn same_directory(recorded: &str, workspace: &Path) -> bool {
    let recorded = Path::new(recorded);
    match (fs::canonicalize(recorded), fs::canonicalize(workspace)) {
        (Ok(recorded), Ok(workspace)) => recorded == workspace,
        _ => recorded == workspace,
    }
}

/// Removes `path`: a file, a symbolic link (not its target), or a directory
/// tree. A path that doesn't exist is already removed.
pub(crate) fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pervue-forget-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removes_files_directories_and_missing_paths() {
        let dir = scratch("remove");
        fs::write(dir.join("file"), "x").unwrap();
        fs::create_dir_all(dir.join("tree/inner")).unwrap();
        fs::write(dir.join("tree/inner/file"), "x").unwrap();
        remove(&dir.join("file")).unwrap();
        remove(&dir.join("tree")).unwrap();
        remove(&dir.join("missing")).unwrap();
        assert!(!dir.join("file").exists());
        assert!(!dir.join("tree").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_removed_but_not_followed() {
        let dir = scratch("link");
        fs::create_dir_all(dir.join("target")).unwrap();
        fs::write(dir.join("target/keep"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("target"), dir.join("link")).unwrap();
        remove(&dir.join("link")).unwrap();
        assert!(!dir.join("link").exists());
        assert!(dir.join("target/keep").exists());
        assert_eq!(head_lines(&dir.join("link")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_lines_keeps_only_complete_lines() {
        let dir = scratch("head");
        fs::write(dir.join("t"), "one\ntwo\npartial").unwrap();
        assert_eq!(
            head_lines(&dir.join("t")),
            Some(vec!["one".to_owned(), "two".to_owned()])
        );
        assert_eq!(head_lines(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
