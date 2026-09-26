//! Exercises only the standalone crate's public surface (no host or browser).

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

use pervue_core::discovery::SearchPath;
use pervue_core::exchange::{Exchange, Scripted, Update};
use pervue_core::framing::{self, FrameError, MAX_FRAME_SIZE};
use pervue_core::process::{Process, ProcessSpec};
use pervue_core::protocol::{
    Authentication, Availability, Capabilities, Capability, ProviderState,
};
use pervue_core::stream::{LineSplitter, Output, StreamError, split_text};

#[test]
fn child_fixture() {
    match std::env::var("PERVUE_CORE_CHILD").as_deref() {
        Ok("echo") => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).unwrap();
            println!("echo:{input}");
            eprintln!("child-stderr");
        }
        Ok("hang") => std::thread::sleep(Duration::from_secs(30)),
        _ => {}
    }
}

#[test]
fn process_and_stream_lifecycle_work_without_host() {
    let self_exe = std::env::current_exe().unwrap();
    let spec = ProcessSpec::new(&self_exe)
        .args(["--exact", "child_fixture", "--nocapture"])
        .env("PERVUE_CORE_CHILD", "echo");
    let mut process = Process::spawn(&spec).unwrap();
    process.write(b"ping").unwrap();
    process.close_stdin();
    let mut lines = pervue_core::stream::LineStream::new(process, 4096).keeping_stderr_tail(64);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_echo = false;
    loop {
        match lines.next(deadline) {
            Some(Output::Line(line)) => saw_echo |= line.contains("echo:ping"),
            Some(Output::Final(exit)) => {
                assert!(exit.status.unwrap().success());
                break;
            }
            other => panic!("unexpected stream state: {other:?}"),
        }
    }
    assert!(saw_echo);
    assert!(lines.stderr_tail().ends_with(b"child-stderr\n"));

    let mut hanging = Process::spawn(
        &ProcessSpec::new(self_exe)
            .args(["--exact", "child_fixture", "--nocapture"])
            .env("PERVUE_CORE_CHILD", "hang"),
    )
    .unwrap();
    let exit = hanging.kill();
    assert!(!exit.status.unwrap().success());
}

#[test]
fn bounded_wire_and_utf8_chunks_work_without_host() {
    let mut output = Cursor::new(Vec::new());
    framing::write_frame(&mut output, b"ok").unwrap();
    output.set_position(0);
    assert_eq!(
        framing::read_frame(&mut output).unwrap(),
        Some(b"ok".to_vec())
    );

    let mut too_large = Cursor::new((MAX_FRAME_SIZE as u32 + 1).to_ne_bytes());
    assert_eq!(
        framing::read_frame(&mut too_large),
        Err(FrameError::TooLarge)
    );

    let mut splitter = LineSplitter::new(4);
    let mut lines = VecDeque::new();
    splitter.push(&[0xc3], &mut lines).unwrap();
    splitter.push(&[0xa9, b'\r', b'\n'], &mut lines).unwrap();
    assert_eq!(lines.pop_front().as_deref(), Some("é"));
    assert_eq!(splitter.finish().unwrap(), None);
    let chunks: Vec<_> = split_text("ééé", 2).collect();
    assert_eq!(chunks.concat(), "ééé");
    assert!(chunks.iter().all(|chunk| chunk.len() <= 4));

    let mut too_long = LineSplitter::new(1);
    assert_eq!(
        too_long.push(b"ab\n", &mut lines),
        Err(StreamError::LineTooLong)
    );
}

#[test]
fn exchange_and_platform_types_do_not_depend_on_host() {
    let status = ProviderState {
        availability: Availability::Available,
        authentication: Authentication::Authenticated,
        capabilities: Capabilities {
            streaming: Capability::Supported,
            continuation: Capability::Unknown,
            web_search: Capability::Unsupported,
            page_context: Capability::Supported,
            attachments: Capability::Unsupported,
            model_selection: Capability::Unknown,
            cancellation: Capability::Supported,
        },
        models: &[],
    };
    let mut exchange: Box<dyn Exchange> = Box::new(Scripted::new([
        Update::Status {
            provider_id: "example".into(),
            status,
        },
        Update::Completed,
    ]));
    assert!(matches!(
        exchange.next(Instant::now()),
        Some(Update::Status { .. })
    ));
    assert!(exchange.next(Instant::now()).unwrap().is_terminal());
    exchange.cancel(Duration::ZERO);
    assert!(exchange.next(Instant::now()).is_none());

    let mut pending = Scripted::new([
        Update::Started {
            conversation_id: None,
        },
        Update::Completed,
    ]);
    pending.cancel(Duration::ZERO);
    assert_eq!(pending.next(Instant::now()), Some(Update::Stopped));
    assert_eq!(pending.next(Instant::now()), None);

    assert!(SearchPath::new(["relative".into()]).dirs().is_empty());
    assert!(Process::spawn(&ProcessSpec::new("relative-program")).is_err());
}
