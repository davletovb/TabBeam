//! Where an application's provider runtime keeps its files.
//!
//! Every directory the runtime chooses for itself is under the application's
//! [`Namespace`], which is fixed when the adapters are built: workspaces in the
//! user's cache, and conversation mappings and cleanup records in the user's
//! data directory. Two applications that link the runtime therefore never
//! share a workspace or a record, so a mistake in one can't touch the other's
//! files. This protects against accidents, not attacks: any process running as
//! the user can already write to both applications' directories.
//!
//! The `pervue` namespace resolves to the paths Pervue has always used, so
//! installed hosts keep finding their existing files.

use std::ffi::OsString;
use std::hash::{BuildHasher, RandomState};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use runtime_core::turn::Namespace;

use crate::environment;

/// The directories one application's adapters use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    namespace: Namespace,
}

impl Layout {
    pub fn new(namespace: Namespace) -> Self {
        Self { namespace }
    }

    pub fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    /// The environment variable that replaces the executable lookup for this
    /// application: the namespace in capitals with `_` for `-`, then
    /// `_PROVIDER_PATH` (`PERVUE_PROVIDER_PATH` for `pervue`). Set to a list
    /// of directories in `PATH` form, it names the only places providers are
    /// looked for, for unusual installs and hermetic tests.
    pub fn search_path_variable(&self) -> String {
        format!(
            "{}_PROVIDER_PATH",
            self.namespace
                .as_str()
                .to_ascii_uppercase()
                .replace('-', "_")
        )
    }

    /// A provider's private workspace: `<provider>-workspace` in the
    /// application's cache directory. Without a cache directory, a new
    /// directory with a random name in the temporary directory, which
    /// [`crate::workspace::prepare`] accepts only where the temporary
    /// directory is private to the user, as it is on macOS and Windows.
    pub fn workspace(&self, host: &[(OsString, OsString)], provider: &str) -> PathBuf {
        self.cache_dir(host).map_or_else(
            || {
                std::env::temp_dir().join(format!(
                    "{}-{provider}-{:016x}",
                    self.namespace.as_str(),
                    RandomState::new().hash_one(SystemTime::now())
                ))
            },
            |cache| cache.join(format!("{provider}-workspace")),
        )
    }

    /// The application's directory in the user's cache.
    ///
    /// macOS and Windows name application directories with a capital letter
    /// (`Pervue`); other Unix systems use the namespace as it is.
    #[cfg(target_vendor = "apple")]
    pub fn cache_dir(&self, host: &[(OsString, OsString)]) -> Option<PathBuf> {
        absolute(host, "HOME").map(|home| home.join("Library/Caches").join(self.title()))
    }

    /// The application's directory in the user's cache.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    pub fn cache_dir(&self, host: &[(OsString, OsString)]) -> Option<PathBuf> {
        absolute(host, "XDG_CACHE_HOME")
            .map(|cache| cache.join(self.namespace.as_str()))
            .or_else(|| {
                absolute(host, "HOME").map(|home| home.join(".cache").join(self.namespace.as_str()))
            })
    }

    /// The application's directory in the user's cache.
    #[cfg(not(unix))]
    pub fn cache_dir(&self, host: &[(OsString, OsString)]) -> Option<PathBuf> {
        absolute(host, "LOCALAPPDATA").map(|local| local.join(self.title()))
    }

    /// The application's directory in the user's data directory, where
    /// conversation mappings and cleanup records live: `$XDG_DATA_HOME` or
    /// `~/.local/share` on Unix, `%LOCALAPPDATA%` (or `%APPDATA%`) on Windows.
    pub fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir_in(&std::env::vars_os().collect::<Vec<_>>())
    }

    /// [`Layout::data_dir`] for the environment `host`.
    pub fn data_dir_in(&self, host: &[(OsString, OsString)]) -> Option<PathBuf> {
        #[cfg(windows)]
        let base = environment::lookup(host, "LOCALAPPDATA")
            .or_else(|| environment::lookup(host, "APPDATA"))
            .map(PathBuf::from);
        #[cfg(not(windows))]
        let base = absolute(host, "XDG_DATA_HOME").or_else(|| {
            environment::lookup(host, "HOME").map(|home| PathBuf::from(home).join(".local/share"))
        });
        base.map(|base| base.join(self.namespace.as_str()))
    }

    /// The namespace with a capital first letter.
    #[cfg(any(target_vendor = "apple", not(unix)))]
    fn title(&self) -> String {
        let name = self.namespace.as_str();
        let mut title = name[..1].to_ascii_uppercase();
        title.push_str(&name[1..]);
        title
    }
}

/// The variable `name` as a path, if it is set to an absolute one.
fn absolute(host: &[(OsString, OsString)], name: &str) -> Option<PathBuf> {
    environment::lookup(host, name)
        .map(PathBuf::from)
        .filter(|path| Path::new(path).is_absolute())
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

    fn layout(name: &str) -> Layout {
        Layout::new(Namespace::fixed(name).unwrap())
    }

    /// Installed hosts keep finding the directories Pervue has always used.
    #[cfg(target_vendor = "apple")]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        assert_eq!(
            layout("pervue").workspace(&vars(&[("HOME", "/Users/me")]), "codex"),
            PathBuf::from("/Users/me/Library/Caches/Pervue/codex-workspace")
        );
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        let pervue = layout("pervue");
        assert_eq!(
            pervue.workspace(&vars(&[("HOME", "/home/me")]), "codex"),
            PathBuf::from("/home/me/.cache/pervue/codex-workspace")
        );
        assert_eq!(
            pervue.workspace(
                &vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")]),
                "claude"
            ),
            PathBuf::from("/cache/pervue/claude-workspace")
        );
        // A relative cache directory would depend on the working directory.
        assert_eq!(
            pervue.workspace(
                &vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "cache")]),
                "codex"
            ),
            PathBuf::from("/home/me/.cache/pervue/codex-workspace")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        assert_eq!(
            layout("pervue").workspace(
                &vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")]),
                "codex"
            ),
            PathBuf::from(r"C:\Users\me\AppData\Local\Pervue\codex-workspace")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn pervue_keeps_its_data_directory() {
        let pervue = layout("pervue");
        assert_eq!(
            pervue.data_dir_in(&vars(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.local/share/pervue"))
        );
        assert_eq!(
            pervue.data_dir_in(&vars(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])),
            Some(PathBuf::from("/data/pervue"))
        );
        // A relative data directory would depend on the working directory.
        assert_eq!(
            pervue.data_dir_in(&vars(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "data")])),
            Some(PathBuf::from("/home/me/.local/share/pervue"))
        );
        assert_eq!(pervue.data_dir_in(&[]), None);
    }

    #[cfg(windows)]
    #[test]
    fn pervue_keeps_its_data_directory() {
        let pervue = layout("pervue");
        assert_eq!(
            pervue.data_dir_in(&vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")])),
            Some(PathBuf::from(r"C:\Users\me\AppData\Local\pervue"))
        );
        assert_eq!(
            pervue.data_dir_in(&vars(&[("APPDATA", r"C:\Users\me\AppData\Roaming")])),
            Some(PathBuf::from(r"C:\Users\me\AppData\Roaming\pervue"))
        );
    }

    #[test]
    fn a_namespace_separates_every_directory() {
        let host = vars(&[("HOME", "/home/me"), ("LOCALAPPDATA", r"C:\Local")]);
        let (pervue, conclave) = (layout("pervue"), layout("conclave"));
        assert_ne!(
            pervue.workspace(&host, "claude"),
            conclave.workspace(&host, "claude")
        );
        assert_ne!(pervue.data_dir_in(&host), conclave.data_dir_in(&host));
        for layout in [&pervue, &conclave] {
            let name = layout.namespace().as_str();
            let contains = |path: PathBuf| path.to_string_lossy().to_lowercase().contains(name);
            assert!(contains(layout.workspace(&host, "claude")));
            assert!(contains(layout.data_dir_in(&host).unwrap()));
        }
    }

    #[test]
    fn without_a_cache_the_workspace_gets_a_new_name() {
        let conclave = layout("conclave");
        let first = conclave.workspace(&vars(&[("HOME", "relative")]), "codex");
        let second = conclave.workspace(&[], "codex");
        for dir in [&first, &second] {
            assert_eq!(dir.parent(), Some(std::env::temp_dir().as_path()));
            let name = dir.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("conclave-codex-"), "{name}");
        }
        assert_ne!(first, second);
    }
}
