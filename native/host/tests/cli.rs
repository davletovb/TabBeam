//! Launch-shape checks for the `pervue-host` binary.

use std::io::Write;
use std::process::{Command, Output, Stdio};

const HOST: &str = env!("CARGO_BIN_EXE_pervue-host");
const EXTENSION_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/";

#[allow(
    clippy::disallowed_methods,
    reason = "these tests start the built host binary; the spawn guard is for the host itself"
)]
fn run_host(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(HOST)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pervue-host");
    // The host may exit before reading stdin (for example on a usage error).
    let _ = child.stdin.take().expect("stdin").write_all(stdin);
    child.wait_with_output().expect("wait for pervue-host")
}

fn frame(payload: &str) -> Vec<u8> {
    let length = u32::try_from(payload.len()).unwrap();
    let mut wire = length.to_ne_bytes().to_vec();
    wire.extend_from_slice(payload.as_bytes());
    wire
}

#[test]
fn version_flag_prints_the_host_version() {
    let output = run_host(&["--version"], b"");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0.1.0-dev");
}

const HOST_READY: &str = r#"{"version":1,"type":"event","request_id":null,"event":"host.ready","payload":{"host_version":"0.1.0-dev","protocol_versions":[1]}}"#;

#[test]
fn chrome_launch_shape_runs_the_host() {
    let output = run_host(&[ORIGIN], b"");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, frame(HOST_READY));
}

#[test]
fn chrome_windows_launch_shape_runs_the_host() {
    // Chrome on Windows appends the calling window's handle; it is 0 when the
    // caller is a service worker.
    for parent_window in [
        "--parent-window=0",
        "--parent-window=132658",
        "--parent-window=-2147483648",
    ] {
        let output = run_host(&[ORIGIN, parent_window], b"");
        assert_eq!(output.status.code(), Some(0), "{parent_window}");
        assert_eq!(output.stdout, frame(HOST_READY), "{parent_window}");
    }
}

#[test]
fn no_arguments_run_the_host() {
    let request = r#"{"version":1,"type":"request","request_id":"req_cli","method":"provider.status","payload":{"provider_id":"codex"}}"#;
    let output = run_host(&[], &frame(request));
    assert_eq!(output.status.code(), Some(0));

    let failure = r#"{"version":1,"type":"event","request_id":"req_cli","event":"response.failed","payload":{"error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED","message":"The selected provider runtime is not installed.","retryable":false}}}"#;
    assert!(
        output.stdout.ends_with(&frame(failure)),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn framing_failures_set_the_exit_status() {
    assert_eq!(run_host(&[], &[0x01, 0x00]).status.code(), Some(3));

    let oversized = (1024_u32 * 1024 + 1).to_ne_bytes();
    assert_eq!(run_host(&[], &oversized).status.code(), Some(4));
}

#[test]
fn unexpected_arguments_print_usage() {
    for args in [
        &["--bogus"][..],
        &["chrome-extension://"],
        &["https://example.com/"],
        &["--version", "extra"],
        &["--print-manifest"],
        &[ORIGIN, "extra"],
        &["--parent-window=0"],
        &["--parent-window=0", ORIGIN],
        &[ORIGIN, "--parent-window="],
        &[ORIGIN, "--parent-window=-"],
        &[ORIGIN, "--parent-window=12ab"],
        &[ORIGIN, "--parent-window", "0"],
        &[ORIGIN, "--parent-window=0", "extra"],
    ] {
        let output = run_host(args, b"");
        assert_eq!(output.status.code(), Some(64), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage:"),
            "{args:?}"
        );
    }
}

#[test]
fn only_exact_extension_origins_start_the_host() {
    // Chrome passes `chrome-extension://<32 characters a-p>/`; anything else is
    // refused before the host reads a frame.
    for origin in [
        "chrome-extension://pervue-test-extension/",
        "chrome-extension://abcdefghijklmnopabcdefghijklmno/",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnopa/",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnoq/",
        "chrome-extension://ABCDEFGHIJKLMNOPABCDEFGHIJKLMNOP/",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/popup.html",
        "chrome-extension://*/",
        "moz-extension://abcdefghijklmnopabcdefghijklmnop/",
    ] {
        let output = run_host(&[origin], &frame("{}"));
        assert_eq!(output.status.code(), Some(64), "{origin}");
        assert!(output.stdout.is_empty(), "{origin}");
    }
}

#[test]
fn print_manifest_registers_this_binary_for_exact_origins() {
    let other_id = "ponmlkjihgfedcbaponmlkjihgfedcba";
    let output = run_host(&["--print-manifest", EXTENSION_ID, other_id], b"");
    assert_eq!(output.status.code(), Some(0));

    let manifest: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the manifest is JSON");
    assert_eq!(manifest["name"], "com.pervue.host");
    assert_eq!(manifest["type"], "stdio");
    assert_eq!(
        manifest["allowed_origins"],
        serde_json::json!([ORIGIN, format!("chrome-extension://{other_id}/")])
    );

    let path = manifest["path"].as_str().expect("path is a string");
    assert!(std::path::Path::new(path).is_absolute(), "{path}");
    assert_eq!(
        std::fs::canonicalize(path).unwrap(),
        std::fs::canonicalize(HOST).unwrap()
    );
}

#[test]
fn print_manifest_rejects_anything_but_extension_ids() {
    for id in ["*", ORIGIN, "abcdefghijklmnopabcdefghijklmnoq", ""] {
        let output = run_host(&["--print-manifest", EXTENSION_ID, id], b"");
        assert_eq!(output.status.code(), Some(64), "{id:?}");
        assert!(output.stdout.is_empty(), "{id:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not a Chrome extension ID"),
            "{id:?}"
        );
    }
}
