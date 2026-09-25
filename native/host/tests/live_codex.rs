//! Opt-in live smoke test for the Codex adapter (PRO-03): with
//! `PERVUE_LIVE_CODEX=1`, the built host asks the installed, signed-in Codex
//! CLI one question and streams back its answer. Without it the test passes
//! at once, so CI never needs Codex or its credentials.
//!
//! ```bash
//! cd native
//! PERVUE_LIVE_CODEX=1 cargo test -p pervue-host --test live_codex -- --nocapture
//! ```

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use pervue_host::framing;
use serde_json::Value;

const HOST: &str = env!("CARGO_BIN_EXE_pervue-host");
const ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop/";
/// A real model can take a while.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);

/// The host under test. It is killed and reaped however the test ends, even
/// on a timeout, so a stuck Codex never leaves it running.
struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn frame(payload: &str) -> Vec<u8> {
    let mut wire = Vec::new();
    framing::write_frame(&mut wire, payload.as_bytes()).unwrap();
    wire
}

#[allow(
    clippy::disallowed_methods,
    reason = "this test starts the built host binary; the spawn guard is for the host itself"
)]
#[test]
fn live_codex_answers_a_question() {
    if std::env::var_os("PERVUE_LIVE_CODEX").is_none() {
        eprintln!("skipped: set PERVUE_LIVE_CODEX=1 to ask the installed Codex");
        return;
    }

    let mut host = Host(
        Command::new(HOST)
            .arg(ORIGIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the host"),
    );
    let mut stdin = host.0.stdin.take().expect("stdin");
    let mut stdout = host.0.stdout.take().expect("stdout");

    let (sender, events) = mpsc::channel::<Value>();
    let reader = thread::spawn(move || {
        while let Ok(Some(frame)) = framing::read_frame(&mut stdout) {
            let event = serde_json::from_slice(&frame).expect("a JSON event");
            if sender.send(event).is_err() {
                return;
            }
        }
    });

    stdin
        .write_all(&frame(
            r#"{"version":1,"type":"request","request_id":"req_status","method":"provider.status","payload":{"provider_id":"codex"}}"#,
        ))
        .unwrap();
    stdin
        .write_all(&frame(
            r#"{"version":1,"type":"request","request_id":"req_live","method":"conversation.send","payload":{"provider_id":"codex","input":{"text":"Reply with the single word: pong"}}}"#,
        ))
        .unwrap();
    stdin.flush().unwrap();

    // Keep stdin open until the answer ends: closing it cancels the request.
    let mut received = Vec::new();
    loop {
        let event = events
            .recv_timeout(ANSWER_TIMEOUT)
            .expect("Codex should answer before the timeout");
        eprintln!("{event}");
        let done = event["request_id"] == "req_live"
            && matches!(
                event["event"].as_str(),
                Some("response.completed" | "response.failed")
            );
        received.push(event);
        if done {
            break;
        }
    }
    drop(stdin);
    assert!(host.0.wait().expect("the host exits").success());
    reader.join().unwrap();

    let status = received
        .iter()
        .find(|event| event["event"] == "provider.status")
        .expect("a provider.status event");
    assert_eq!(status["payload"]["status"]["availability"], "available");
    assert_eq!(
        status["payload"]["status"]["authentication"],
        "authenticated"
    );

    let answer: Vec<&str> = received
        .iter()
        .filter(|event| event["request_id"] == "req_live")
        .map(|event| event["event"].as_str().unwrap())
        .collect();
    assert_eq!(answer.first(), Some(&"conversation.created"), "{answer:?}");
    assert_eq!(answer.get(1), Some(&"response.started"), "{answer:?}");
    assert_eq!(answer.last(), Some(&"response.completed"), "{answer:?}");
    let text: String = received
        .iter()
        .filter(|event| event["event"] == "response.delta")
        .map(|event| event["payload"]["text"].as_str().unwrap())
        .collect();
    assert!(!text.trim().is_empty());
    eprintln!("Codex answered: {text}");
}
