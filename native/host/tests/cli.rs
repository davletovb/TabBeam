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
    // An empty provider search path: whatever is installed on this machine,
    // the host finds no provider executables.
    let no_providers = std::env::temp_dir().join("pervue-cli-tests-no-providers");
    let mut child = Command::new(HOST)
        .args(args)
        .env("PERVUE_PROVIDER_PATH", no_providers)
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
fn the_chrome_launch_shape_serves_requests() {
    let request = r#"{"version":1,"type":"request","request_id":"req_cli","method":"provider.status","payload":{"provider_id":"missing"}}"#;
    let output = run_host(&[ORIGIN], &frame(request));
    assert_eq!(output.status.code(), Some(0));

    let failure = r#"{"version":1,"type":"event","request_id":"req_cli","event":"response.failed","payload":{"error":{"code":"PROVIDER_NOT_FOUND","reason":"PROVIDER_NOT_INSTALLED","message":"The selected provider runtime is not installed.","retryable":false}}}"#;
    assert!(
        output.stdout.ends_with(&frame(failure)),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn the_installed_host_reports_a_missing_codex() {
    let request = r#"{"version":1,"type":"request","request_id":"req_codex","method":"provider.status","payload":{"provider_id":"codex"}}"#;
    let output = run_host(&[ORIGIN], &frame(request));
    assert_eq!(output.status.code(), Some(0));

    let frames = frames_only(&output.stdout);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1]["event"], "provider.status");
    assert_eq!(frames[1]["payload"]["provider_id"], "codex");
    assert_eq!(frames[1]["payload"]["status"]["availability"], "not_found");
    assert_eq!(frames[2]["event"], "response.completed");
}

#[test]
fn framing_failures_set_the_exit_status() {
    assert_eq!(run_host(&[ORIGIN], &[0x01, 0x00]).status.code(), Some(3));

    let oversized = (1024_u32 * 1024 + 1).to_ne_bytes();
    assert_eq!(run_host(&[ORIGIN], &oversized).status.code(), Some(4));
}

#[test]
fn unexpected_arguments_print_usage() {
    for args in [
        // Without a caller origin the host serves nothing (SEC-01).
        &[][..],
        &["--bogus"],
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

/// Splits stdout into frames, failing on any byte outside a whole frame.
fn frames_only(mut stdout: &[u8]) -> Vec<serde_json::Value> {
    let mut frames = Vec::new();
    while !stdout.is_empty() {
        let (prefix, rest) = stdout.split_at(4);
        let length = u32::from_ne_bytes(prefix.try_into().unwrap()) as usize;
        frames.push(serde_json::from_slice(&rest[..length]).expect("a frame is JSON"));
        stdout = &rest[length..];
    }
    frames
}

#[test]
fn diagnostics_go_to_stderr_and_never_into_the_frames() {
    let requests = [
        r#"{"version":1,"type":"request","request_id":"req_a","method":"conversation.send","payload":{"provider_id":"fake","input":{"text":"hi"}}}"#,
        "{not json",
        r#"{"version":1,"type":"request","request_id":"req_b","method":"provider.status","payload":{"provider_id":"missing"}}"#,
    ];
    let input: Vec<u8> = requests.iter().flat_map(|request| frame(request)).collect();
    let output = run_host(&[ORIGIN], &input);
    assert_eq!(output.status.code(), Some(0));

    // stdout: host.ready, four events for req_a, one failure each for the
    // malformed frame and req_b, and nothing else.
    let frames = frames_only(&output.stdout);
    assert_eq!(frames.len(), 7);

    // stderr: one JSON object per line, from host.started to host.stopped.
    let stderr = String::from_utf8(output.stderr).unwrap();
    let records: Vec<serde_json::Value> = stderr
        .lines()
        .map(|line| serde_json::from_str(line).expect("a diagnostics line is JSON"))
        .collect();
    let events: Vec<&str> = records
        .iter()
        .map(|record| record["event"].as_str().unwrap())
        .collect();
    assert_eq!(
        events,
        [
            "host.started",
            "request.completed",
            "request.rejected",
            "request.failed",
            "host.stopped"
        ]
    );
    // req_a started a conversation, so its record names the created one.
    assert_eq!(records[1]["conversation_id"], "fake-conversation");
    assert_eq!(records[4]["exit_code"], 0);
}

#[test]
fn the_logged_exit_code_matches_the_process() {
    let oversized = (1024_u32 * 1024 + 1).to_ne_bytes();
    let output = run_host(&[ORIGIN], &oversized);
    assert_eq!(output.status.code(), Some(4));

    let stderr = String::from_utf8(output.stderr).unwrap();
    let last: serde_json::Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "host.stopped");
    assert_eq!(last["reason"], "frame_too_large");
    assert_eq!(last["exit_code"], 4);
    assert!(output.stdout.len() > 4, "host.ready is still written");
}
