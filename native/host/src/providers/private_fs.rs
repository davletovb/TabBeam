use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
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

pub(crate) fn write_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) fn is_conversation_id(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("conv_")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn new_conversation_id() -> String {
    format!(
        "conv_{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    )
}

pub(crate) fn unique_child(base: &Path, prefix: &str) -> PathBuf {
    base.join(format!(
        "{prefix}-{:016x}",
        RandomState::new().hash_one((SystemTime::now(), std::process::id()))
    ))
}

pub(crate) fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8], limit: usize) {
    let bytes = &bytes[bytes.len().saturating_sub(limit)..];
    let excess = (tail.len() + bytes.len()).saturating_sub(limit);
    tail.drain(..excess);
    tail.extend_from_slice(bytes);
}

pub(crate) fn after(duration: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(duration).unwrap_or(now)
}
