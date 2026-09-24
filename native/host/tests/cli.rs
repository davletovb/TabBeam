//! Launch-shape checks for the `pervue-host` binary.

use std::io::Write;
use std::process::{Command, Output, Stdio};

const HOST: &str = env!("CARGO_BIN_EXE_pervue-host");

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

#[test]
fn chrome_launch_shape_runs_the_host() {
    let output = run_host(&["chrome-extension://pervue-test-extension/"], b"");
    assert_eq!(output.status.code(), Some(0));

    let host_ready = r#"{"version":1,"type":"event","request_id":null,"event":"host.ready","payload":{"host_version":"0.1.0-dev","protocol_versions":[1]}}"#;
    assert_eq!(output.stdout, frame(host_ready));
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
        &["chrome-extension://id/", "extra"],
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
