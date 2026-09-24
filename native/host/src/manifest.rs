//! Chrome's Native Messaging host manifest and caller identity (SEC-01).
//!
//! Chrome starts the host only for the extensions whose origins the manifest
//! lists in `allowed_origins`, so the manifest is the identity check at the
//! browser/native boundary. The host generates it (`--print-manifest`) rather
//! than leaving it to hand-edited JSON, so it can only list exact extension
//! origins: never a wildcard, a pattern, or another scheme.

use std::fmt;
use std::path::Path;

use serde::Serialize;

/// The name the extension connects to and the manifest registers.
pub const HOST_NAME: &str = "com.pervue.host";

const DESCRIPTION: &str = "Pervue native host";
const EXTENSION_ID_LENGTH: usize = 32;
const ORIGIN_SCHEME: &[u8] = b"chrome-extension://";

/// Whether `id` is a Chrome extension ID: 32 characters from `a` to `p`.
pub fn is_extension_id(id: &[u8]) -> bool {
    id.len() == EXTENSION_ID_LENGTH && id.iter().all(|byte| (b'a'..=b'p').contains(byte))
}

/// Whether `origin` is exactly the caller origin Chrome passes to the host:
/// `chrome-extension://<extension ID>/`.
pub fn is_extension_origin(origin: &[u8]) -> bool {
    origin
        .strip_prefix(ORIGIN_SCHEME)
        .and_then(|rest| rest.strip_suffix(b"/"))
        .is_some_and(is_extension_id)
}

/// Why a manifest could not be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// No extension ID was given, so no extension could use the host.
    NoExtensionIds,
    /// An argument is not a Chrome extension ID.
    InvalidExtensionId(String),
    /// Chrome requires an absolute host path on macOS and Linux.
    RelativeHostPath,
    /// The host path cannot be written as a JSON string.
    NonUnicodeHostPath,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoExtensionIds => write!(formatter, "at least one extension ID is required"),
            Self::InvalidExtensionId(id) => write!(
                formatter,
                "not a Chrome extension ID (32 characters a-p): {id:?}"
            ),
            Self::RelativeHostPath => write!(formatter, "the host path must be absolute"),
            Self::NonUnicodeHostPath => write!(formatter, "the host path is not valid Unicode"),
        }
    }
}

impl std::error::Error for ManifestError {}

#[derive(Serialize)]
struct Manifest<'a> {
    name: &'a str,
    description: &'a str,
    path: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    allowed_origins: Vec<String>,
}

/// The manifest that registers `host_path` for exactly `extension_ids`, as
/// pretty-printed JSON. Repeated IDs are listed once.
pub fn manifest_json(host_path: &Path, extension_ids: &[&str]) -> Result<String, ManifestError> {
    if extension_ids.is_empty() {
        return Err(ManifestError::NoExtensionIds);
    }
    if !host_path.is_absolute() {
        return Err(ManifestError::RelativeHostPath);
    }
    let path = host_path
        .to_str()
        .ok_or(ManifestError::NonUnicodeHostPath)?;

    let mut allowed_origins = Vec::with_capacity(extension_ids.len());
    for id in extension_ids {
        if !is_extension_id(id.as_bytes()) {
            return Err(ManifestError::InvalidExtensionId((*id).to_owned()));
        }
        let origin = format!("chrome-extension://{id}/");
        if !allowed_origins.contains(&origin) {
            allowed_origins.push(origin);
        }
    }

    let manifest = Manifest {
        name: HOST_NAME,
        description: DESCRIPTION,
        path,
        kind: "stdio",
        allowed_origins,
    };
    Ok(serde_json::to_string_pretty(&manifest).expect("a manifest of strings serializes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "abcdefghijklmnopabcdefghijklmnop";
    const OTHER_ID: &str = "ponmlkjihgfedcbaponmlkjihgfedcba";

    fn absolute_host_path() -> std::path::PathBuf {
        std::env::temp_dir().join("pervue-host")
    }

    #[test]
    fn extension_ids_are_32_letters_from_a_to_p() {
        assert!(is_extension_id(ID.as_bytes()));
        assert!(is_extension_id(OTHER_ID.as_bytes()));

        for invalid in [
            "",
            "abcdefghijklmnopabcdefghijklmno",
            "abcdefghijklmnopabcdefghijklmnopa",
            "abcdefghijklmnopabcdefghijklmnoq",
            "Abcdefghijklmnopabcdefghijklmnop",
            "abcdefghijklmnopabcdefghijklmn0p",
            "abcdefghijklmnop*bcdefghijklmnop",
            "abcdefghijklmnop/bcdefghijklmnop",
            "abcdefghijklmnop bcdefghijklmnop",
        ] {
            assert!(!is_extension_id(invalid.as_bytes()), "{invalid:?}");
        }
    }

    #[test]
    fn caller_origins_must_be_exact() {
        assert!(is_extension_origin(
            format!("chrome-extension://{ID}/").as_bytes()
        ));

        for invalid in [
            format!("chrome-extension://{ID}"),
            format!("chrome-extension://{ID}//"),
            format!("chrome-extension://{ID}/popup.html"),
            format!("chrome-extension://{ID}/?x"),
            format!("Chrome-Extension://{ID}/"),
            format!("moz-extension://{ID}/"),
            format!("https://{ID}/"),
            format!(" chrome-extension://{ID}/"),
            "chrome-extension://*/".to_owned(),
            "chrome-extension://pervue/".to_owned(),
            "chrome-extension:///".to_owned(),
        ] {
            assert!(!is_extension_origin(invalid.as_bytes()), "{invalid:?}");
        }
    }

    #[test]
    fn manifest_lists_exactly_the_given_origins() {
        let path = absolute_host_path();
        let json = manifest_json(&path, &[ID, OTHER_ID, ID]).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            manifest,
            serde_json::json!({
                "name": "com.pervue.host",
                "description": "Pervue native host",
                "path": path.to_str().unwrap(),
                "type": "stdio",
                "allowed_origins": [
                    format!("chrome-extension://{ID}/"),
                    format!("chrome-extension://{OTHER_ID}/")
                ]
            })
        );
    }

    #[test]
    fn manifest_rejects_anything_but_extension_ids() {
        let path = absolute_host_path();
        assert_eq!(
            manifest_json(&path, &[]),
            Err(ManifestError::NoExtensionIds)
        );
        for invalid in [
            "*",
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/",
            "",
        ] {
            assert_eq!(
                manifest_json(&path, &[ID, invalid]),
                Err(ManifestError::InvalidExtensionId(invalid.to_owned()))
            );
        }
        assert_eq!(
            manifest_json(Path::new("pervue-host"), &[ID]),
            Err(ManifestError::RelativeHostPath)
        );
    }
}
