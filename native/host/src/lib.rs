//! TabBeam native host.
//!
//! The host owns Chrome Native Messaging framing and protocol-v1 encoding.
//! It validates browser requests, applies product policy, and routes provider
//! work through the reusable `seatline-core` primitives. Native Messaging is
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

/// TabBeam's provider-runtime namespace (ADR-0002): its workspaces, conversation
/// mappings and cleanup records live under this name. This clean-break namespace
/// is established before the first release and should remain stable afterward.
pub const NAMESPACE: &str = "tabbeam";

/// The directories TabBeam's provider adapters use.
pub fn layout() -> providers::Layout {
    providers::Layout::with_cache_title(
        seatline_core::turn::Namespace::fixed(NAMESPACE).expect("TabBeam's namespace is valid"),
        "TabBeam",
    )
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use seatline_providers::{gemini, grok};

    use super::{NAMESPACE, layout};

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn tabbeams_namespace_is_valid() {
        assert_eq!(layout().namespace().as_str(), "tabbeam");
        assert_eq!(NAMESPACE, "tabbeam");
    }

    /// The clean-break rename establishes TabBeam's provider override and
    /// application-owned runtime names before the first release.
    #[test]
    fn tabbeam_uses_its_provider_override_and_runtime_names() {
        assert_eq!(layout().search_path_variable(), "TABBEAM_PROVIDER_PATH");
        // A killed host's leftover Grok workspaces are found by this file.
        assert_eq!(grok::owner_file(layout().namespace()), ".tabbeam-owner");
        assert_eq!(
            gemini::agent_name(layout().namespace(), false),
            "tabbeam-text"
        );
        assert_eq!(
            gemini::agent_name(layout().namespace(), true),
            "tabbeam-search"
        );
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn tabbeam_uses_its_cache_directory() {
        assert_eq!(
            layout().workspace(&vars(&[("HOME", "/Users/me")]), "codex"),
            PathBuf::from("/Users/me/Library/Caches/TabBeam/codex-workspace")
        );
    }

    #[cfg(all(unix, not(target_vendor = "apple")))]
    #[test]
    fn tabbeam_uses_its_cache_directory() {
        assert_eq!(
            layout().workspace(&vars(&[("HOME", "/home/me")]), "codex"),
            PathBuf::from("/home/me/.cache/tabbeam/codex-workspace")
        );
        assert_eq!(
            layout().workspace(
                &vars(&[("HOME", "/home/me"), ("XDG_CACHE_HOME", "/cache")]),
                "claude"
            ),
            PathBuf::from("/cache/tabbeam/claude-workspace")
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn tabbeam_uses_its_cache_directory() {
        assert_eq!(
            layout().workspace(
                &vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")]),
                "codex"
            ),
            PathBuf::from(r"C:\Users\me\AppData\Local\TabBeam\codex-workspace")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn tabbeam_uses_its_data_directory() {
        assert_eq!(
            layout().data_dir_in(&vars(&[("HOME", "/home/me")])),
            Some(PathBuf::from("/home/me/.local/share/tabbeam"))
        );
        assert_eq!(
            layout().data_dir_in(&vars(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")])),
            Some(PathBuf::from("/data/tabbeam"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn tabbeam_uses_its_data_directory() {
        assert_eq!(
            layout().data_dir_in(&vars(&[("LOCALAPPDATA", r"C:\Users\me\AppData\Local")])),
            Some(PathBuf::from(r"C:\Users\me\AppData\Local\tabbeam"))
        );
    }
}
