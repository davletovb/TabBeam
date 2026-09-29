//! Pervue native host.
//!
//! The host owns Chrome Native Messaging framing and protocol-v1 encoding.
//! It validates browser requests, applies product policy, and routes provider
//! work through the reusable `runtime-core` primitives. Native Messaging is
//! deliberately host-owned under ADR-0002; it is not part of the shared
//! provider runtime boundary.

pub mod conversation;
pub mod conversations;
pub mod diagnostics;
pub mod framing;
pub mod host;
pub mod limits;
pub mod manifest;
pub mod protocol;
pub mod providers;
pub mod search;

/// Host version reported by `--version` and in the `host.ready` event.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Pervue's provider-runtime namespace (ADR-0002): its workspaces, conversation
/// mappings and cleanup records live under this name. Changing it would strand
/// installed hosts' files.
pub const NAMESPACE: &str = "pervue";

/// The directories Pervue's provider adapters use.
pub fn layout() -> providers::Layout {
    providers::Layout::new(
        runtime_core::turn::Namespace::fixed(NAMESPACE).expect("Pervue's namespace is valid"),
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use runtime_providers::{gemini, grok};

    use super::{NAMESPACE, layout};

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn pervues_namespace_is_valid_and_unchanged() {
        assert_eq!(layout().namespace().as_str(), "pervue");
        assert_eq!(NAMESPACE, "pervue");
    }

    /// Installed hosts keep finding the directories Pervue has always used, and
    /// the names it has always given what it leaves in them.
    #[test]
    fn installed_hosts_keep_their_override_and_the_names_they_look_for() {
        assert_eq!(layout().search_path_variable(), "PERVUE_PROVIDER_PATH");
        // A killed host's leftover Grok workspaces are found by this file.
        assert_eq!(grok::owner_file(layout().namespace()), ".pervue-owner");
        assert_eq!(
            gemini::agent_name(layout().namespace(), false),
            "pervue-text"
        );
        assert_eq!(
            gemini::agent_name(layout().namespace(), true),
            "pervue-search"
        );
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        assert_eq!(
            layout().workspace(&vars(&[("HOME", "/Users/me")]), "codex"),
            PathBuf::from("/Users/me/Library/Caches/Pervue/codex-workspace")
        );
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        assert_eq!(
            layout().workspace(&vars(&[("HOME", "/home/me")]), "codex"),
            PathBuf::from("/home/me/.cache/pervue/codex-workspace")
        );
        assert_eq!(
            layout().workspace(
                &vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")]),
                "claude"
            ),
            PathBuf::from("/cache/pervue/claude-workspace")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn pervue_keeps_its_cache_directory() {
        assert_eq!(
            layout().workspace(
                &vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")]),
                "codex"
            ),
            PathBuf::from(r"C:\Users\me\AppData\Local\Pervue\codex-workspace")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn pervue_keeps_its_data_directory() {
        assert_eq!(
            layout().data_dir_in(&vars(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.local/share/pervue"))
        );
        assert_eq!(
            layout().data_dir_in(&vars(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])),
            Some(PathBuf::from("/data/pervue"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn pervue_keeps_its_data_directory() {
        assert_eq!(
            layout().data_dir_in(&vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")])),
            Some(PathBuf::from(r"C:\Users\me\AppData\Local\pervue"))
        );
    }
}
