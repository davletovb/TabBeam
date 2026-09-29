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
    #[test]
    fn pervues_namespace_is_valid_and_unchanged() {
        assert_eq!(super::layout().namespace().as_str(), "pervue");
    }
}
