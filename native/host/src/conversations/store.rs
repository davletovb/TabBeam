//! Where Pervue keeps which native session each of its conversations uses.
//!
//! One file per conversation, named by its `conv_…` ID and holding the
//! provider's opaque session handle, and a `superseded/<conversation>/`
//! directory of markers for sessions a conversation left behind, whose saved
//! transcripts still have to be removed. Files are private to the user on
//! Unix. This is the layout the adapters kept before the mapping moved out of
//! them, so a host that already has files here keeps finding them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use runtime_platform::{forget, private_fs};

use super::is_conversation_id;
use runtime_core::turn::{MAX_CONTINUATION_BYTES, is_session_handle};

/// What a new conversation needs of its mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// A mapping goes to disk when there is a directory for it, and stays in
    /// memory for the host's lifetime when there isn't.
    BestEffort,
    /// A new conversation is refused unless its mapping can go to disk, so a
    /// conversation that couldn't be resumed after a restart never starts.
    Required,
}

/// The conversation→session mappings of one provider.
pub struct SessionStore {
    dir: Option<PathBuf>,
    durability: Durability,
    memory: RefCell<HashMap<String, String>>,
}

impl SessionStore {
    /// A store in `dir`, or in memory only without one.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            durability: Durability::BestEffort,
            memory: RefCell::default(),
        }
    }

    #[must_use]
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// The session `conversation` currently uses, from memory or from disk.
    pub fn get(&self, conversation: &str) -> Option<String> {
        self.memory
            .borrow()
            .get(conversation)
            .cloned()
            .or_else(|| self.dir.as_deref().and_then(|dir| read(dir, conversation)))
    }

    /// A conversation ID nobody has used.
    pub fn new_id(&self) -> String {
        loop {
            let id = format!(
                "conv_{:016x}",
                RandomState::new().hash_one((SystemTime::now(), self.memory.borrow().len()))
            );
            let taken = self.memory.borrow().contains_key(&id)
                || self
                    .dir
                    .as_deref()
                    .is_some_and(|dir| dir.join(&id).exists());
            if !taken {
                return id;
            }
        }
    }

    /// Records the first session of a new conversation.
    pub fn remember_new(&self, conversation: &str, session: &str) -> io::Result<()> {
        match (&self.dir, self.durability) {
            (Some(dir), _) => write(dir, conversation, session, true)?,
            (None, Durability::Required) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "no directory for session mappings",
                ));
            }
            (None, Durability::BestEffort) => {}
        }
        self.insert(conversation, session);
        Ok(())
    }

    /// Replaces the session of `conversation` with `session`.
    pub fn remember(&self, conversation: &str, session: &str) -> io::Result<()> {
        if let Some(dir) = &self.dir {
            write(dir, conversation, session, false)?;
        }
        self.insert(conversation, session);
        Ok(())
    }

    /// Replaces the session of `conversation` after a turn ended, when the
    /// answer is already out: a failed write doesn't matter to it. The new
    /// session is kept in memory, and the stale file is dropped, so a
    /// restarted host rebuilds from history instead of resuming the wrong
    /// session.
    pub fn follow(&self, conversation: &str, session: &str) {
        if self.remember(conversation, session).is_err() {
            if let Some(dir) = &self.dir {
                let _ = forget_file(dir, conversation);
            }
            self.insert(conversation, session);
        }
    }

    /// Drops a mapping the provider says no longer exists. Best effort: a
    /// mapping that can't be removed is replaced by the next one.
    pub fn drop_mapping(&self, conversation: &str) {
        self.memory.borrow_mut().remove(conversation);
        if let Some(dir) = &self.dir {
            let _ = forget_file(dir, conversation);
        }
    }

    /// Records `old` as a session `conversation` left, before its replacement
    /// is written, so its transcript can be removed even after a restart.
    /// Returns the marker to delete once that succeeded, or `None` when there
    /// is no directory to keep one in.
    pub fn record_superseded(&self, conversation: &str, old: &str) -> io::Result<Option<PathBuf>> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        if !is_conversation_id(conversation) || !is_session_handle(old) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid superseded session",
            ));
        }
        let base = superseded_dir(dir, conversation);
        private_fs::create_private_dir(&base)?;
        private_fs::write_private_file(&base.join(old), b"pending\n")?;
        Ok(Some(base.join(old)))
    }

    /// The sessions `conversation` left whose transcripts are still recorded
    /// as pending.
    pub fn superseded(&self, conversation: &str) -> Vec<String> {
        // A conversation ID is joined into a path: only Pervue's own shape.
        let Some(dir) = self
            .dir
            .as_ref()
            .filter(|_| is_conversation_id(conversation))
        else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(superseded_dir(dir, conversation)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|session| is_session_handle(session))
            .collect()
    }

    /// Every session `conversation` has used: the current one, then those it
    /// left.
    pub fn sessions_of(&self, conversation: &str) -> Vec<String> {
        let mut sessions: Vec<String> = self.get(conversation).into_iter().collect();
        sessions.extend(self.superseded(conversation));
        sessions
    }

    /// Removes what is kept on disk for `conversation`: its mapping and its
    /// pending markers. Its in-memory mapping stays until [`forget_memory`].
    ///
    /// [`forget_memory`]: SessionStore::forget_memory
    pub fn files_to_remove(&self, conversation: &str) -> Option<Removal> {
        // A request can name any conversation, and this removes a directory
        // named after it: only Pervue's own IDs may.
        if !is_conversation_id(conversation) {
            return None;
        }
        let dir = self.dir.clone()?;
        Some(Removal {
            dir,
            conversation: conversation.to_owned(),
        })
    }

    /// Drops the in-memory mapping, once everything else about the
    /// conversation is gone: if a provider's files couldn't all be removed, a
    /// retry can still find them.
    pub fn forget_memory(&self, conversation: &str) {
        self.memory.borrow_mut().remove(conversation);
    }

    fn insert(&self, conversation: &str, session: &str) {
        self.memory
            .borrow_mut()
            .insert(conversation.to_owned(), session.to_owned());
    }
}

/// The on-disk part of forgetting a conversation, ready to run on any thread.
pub struct Removal {
    dir: PathBuf,
    conversation: String,
}

impl Removal {
    pub fn run(self) -> io::Result<()> {
        forget_file(&self.dir, &self.conversation)?;
        forget::remove(&superseded_dir(&self.dir, &self.conversation))
    }
}

fn superseded_dir(dir: &Path, conversation: &str) -> PathBuf {
    dir.join("superseded").join(conversation)
}

fn read(dir: &Path, conversation: &str) -> Option<String> {
    if !is_conversation_id(conversation) {
        return None;
    }
    let mut content = String::new();
    std::fs::File::open(dir.join(conversation))
        .ok()?
        .take(MAX_CONTINUATION_BYTES as u64 + 1)
        .read_to_string(&mut content)
        .ok()?;
    is_session_handle(&content).then_some(content)
}

fn write(dir: &Path, conversation: &str, session: &str, only_if_new: bool) -> io::Result<()> {
    if !is_conversation_id(conversation) || !is_session_handle(session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid session mapping",
        ));
    }
    private_fs::create_private_dir(dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if only_if_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(dir.join(conversation))?;
    file.write_all(session.as_bytes())?;
    file.sync_all()
}

/// Removes a stored mapping, if any.
fn forget_file(dir: &Path, conversation: &str) -> io::Result<()> {
    if is_conversation_id(conversation) {
        forget::remove(&dir.join(conversation))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pervue-store-{name}-{}-{:x}",
                std::process::id(),
                RandomState::new().hash_one(SystemTime::now())
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const CONVERSATION: &str = "conv_0123456789abcdef";

    #[test]
    fn a_mapping_survives_a_restart_in_the_layout_adapters_used() {
        let scratch = Scratch::new("restart");
        let store = SessionStore::new(Some(scratch.0.clone()));
        store.remember_new(CONVERSATION, "session-1").unwrap();
        // One file named by the conversation, holding the bare handle.
        assert_eq!(
            std::fs::read_to_string(scratch.0.join(CONVERSATION)).unwrap(),
            "session-1"
        );
        let restarted = SessionStore::new(Some(scratch.0.clone()));
        assert_eq!(restarted.get(CONVERSATION).as_deref(), Some("session-1"));
    }

    #[test]
    fn a_new_conversation_never_replaces_an_existing_mapping() {
        let scratch = Scratch::new("create-new");
        let store = SessionStore::new(Some(scratch.0.clone()));
        store.remember_new(CONVERSATION, "session-1").unwrap();
        assert!(store.remember_new(CONVERSATION, "session-2").is_err());
        assert_eq!(store.get(CONVERSATION).as_deref(), Some("session-1"));
    }

    #[test]
    fn superseded_sessions_are_recorded_beside_the_mappings() {
        let scratch = Scratch::new("superseded");
        let store = SessionStore::new(Some(scratch.0.clone()));
        let marker = store
            .record_superseded(CONVERSATION, "old-session")
            .unwrap()
            .unwrap();
        assert_eq!(
            marker,
            scratch
                .0
                .join("superseded")
                .join(CONVERSATION)
                .join("old-session")
        );
        assert_eq!(store.superseded(CONVERSATION), ["old-session"]);
        store.remember_new(CONVERSATION, "new-session").unwrap();
        assert_eq!(
            store.sessions_of(CONVERSATION),
            ["new-session", "old-session"]
        );
    }

    #[test]
    fn removing_a_conversation_takes_its_mapping_and_markers() {
        let scratch = Scratch::new("remove");
        let store = SessionStore::new(Some(scratch.0.clone()));
        store.remember_new(CONVERSATION, "session-1").unwrap();
        store
            .record_superseded(CONVERSATION, "old-session")
            .unwrap();
        store.files_to_remove(CONVERSATION).unwrap().run().unwrap();
        // The in-memory mapping stays until the caller says everything else
        // is gone.
        assert_eq!(store.get(CONVERSATION).as_deref(), Some("session-1"));
        store.forget_memory(CONVERSATION);
        assert_eq!(store.get(CONVERSATION), None);
        assert!(store.superseded(CONVERSATION).is_empty());
    }

    #[test]
    fn without_a_directory_mappings_live_in_memory_unless_durability_is_required() {
        let best_effort = SessionStore::new(None);
        best_effort.remember_new(CONVERSATION, "session-1").unwrap();
        assert_eq!(best_effort.get(CONVERSATION).as_deref(), Some("session-1"));
        let required = SessionStore::new(None).with_durability(Durability::Required);
        assert!(required.remember_new(CONVERSATION, "session-1").is_err());
        assert_eq!(required.get(CONVERSATION), None);
    }

    #[test]
    fn only_well_formed_conversations_and_sessions_reach_the_disk() {
        let scratch = Scratch::new("names");
        let store = SessionStore::new(Some(scratch.0.clone()));
        for (conversation, session) in [
            ("../escape", "session"),
            ("conv_short", "session"),
            (CONVERSATION, "--resume"),
            (CONVERSATION, ""),
            (CONVERSATION, "has space"),
        ] {
            assert!(
                store.remember_new(conversation, session).is_err(),
                "{conversation} {session}"
            );
        }
        assert!(!scratch.0.exists() || std::fs::read_dir(&scratch.0).unwrap().count() == 0);
        assert_eq!(store.get("../escape"), None);
    }

    #[test]
    fn following_a_new_session_survives_a_failed_write() {
        let scratch = Scratch::new("follow");
        let store = SessionStore::new(Some(scratch.0.join("blocked")));
        // A file where the directory should be: every write fails.
        std::fs::create_dir_all(&scratch.0).unwrap();
        std::fs::write(scratch.0.join("blocked"), b"not a directory").unwrap();
        store.follow(CONVERSATION, "session-2");
        assert_eq!(store.get(CONVERSATION).as_deref(), Some("session-2"));
    }

    /// A `conversation.forget` names whatever conversation it likes. It must
    /// never reach outside the store, however the ID is spelled.
    #[test]
    fn a_conversation_id_can_never_name_a_path_outside_the_store() {
        let scratch = Scratch::new("traversal");
        let store_dir = scratch.0.join("pervue").join("claude-sessions");
        let store = SessionStore::new(Some(store_dir.clone()));
        store.remember_new(CONVERSATION, "session-1").unwrap();
        store
            .record_superseded(CONVERSATION, "old-session")
            .unwrap();
        // Something a traversal would delete: the store's own directory, its
        // parent, and a sibling.
        let sibling = scratch.0.join("pervue").join("important");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("data"), b"keep").unwrap();

        for id in [
            "..",
            "../..",
            "../../important",
            "/",
            "superseded/..",
            "conv_../../..",
            ".",
            "conv_0123456789abcde/",
        ] {
            assert!(store.files_to_remove(id).is_none(), "{id}");
            assert!(store.superseded(id).is_empty(), "{id}");
            assert!(store.sessions_of(id).is_empty(), "{id}");
            store.drop_mapping(id);
        }
        assert!(store_dir.join(CONVERSATION).exists());
        assert!(store_dir.join("superseded").join(CONVERSATION).exists());
        assert_eq!(std::fs::read(sibling.join("data")).unwrap(), b"keep");
    }

    #[test]
    fn generated_ids_are_conversation_ids() {
        let store = SessionStore::new(None);
        assert!(is_conversation_id(&store.new_id()));
    }
}
