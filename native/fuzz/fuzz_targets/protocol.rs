#![no_main]

use libfuzzer_sys::fuzz_target;
use pervue_host::diagnostics::Diagnostics;
use pervue_host::framing;
use pervue_host::host;
use pervue_host::limits::MAX_FRAME_SIZE;

// Treats the input as one request frame and runs it through the host: request
// validation, routing, event emission, and diagnostics. Whatever the request
// contains, every frame the host writes must be a single JSON object, and the
// diagnostics must be three JSON lines: host.started, one record for the
// request, and host.stopped.
fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FRAME_SIZE {
        return;
    }

    let mut input = Vec::new();
    framing::write_frame(&mut input, data).expect("frame the request");
    let mut output = Vec::new();
    let mut log = Diagnostics::new(Vec::new());
    host::run(&mut input.as_slice(), &mut output, &mut log)
        .expect("host handles one in-memory frame");

    let mut frames = output.as_slice();
    while let Some(frame) = framing::read_frame(&mut frames).expect("host output is framed") {
        let event: serde_json::Value =
            serde_json::from_slice(&frame).expect("host emitted invalid JSON");
        assert!(event.is_object(), "host emitted a non-object event");
    }

    let log = String::from_utf8(log.into_inner()).expect("diagnostics are UTF-8");
    let events: Vec<String> = log
        .lines()
        .map(|line| {
            let record: serde_json::Value =
                serde_json::from_str(line).expect("a diagnostics line is JSON");
            record["event"]
                .as_str()
                .expect("a record names its event")
                .to_owned()
        })
        .collect();
    assert_eq!(events.len(), 3, "one record per lifecycle step: {log}");
    assert_eq!(events[0], "host.started");
    assert!(events[1].starts_with("request."), "{log}");
    assert_eq!(events[2], "host.stopped");
});
