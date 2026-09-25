#![no_main]

use libfuzzer_sys::fuzz_target;
use pervue_host::framing;
use pervue_host::host;
use pervue_host::limits::MAX_FRAME_SIZE;

// Treats the input as one request frame and runs it through the host: request
// validation, routing, and event emission. Whatever the request contains, every
// frame the host writes must be a single JSON object.
fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_FRAME_SIZE {
        return;
    }

    let mut input = Vec::new();
    framing::write_frame(&mut input, data).expect("frame the request");
    let mut output = Vec::new();
    host::run(&mut input.as_slice(), &mut output).expect("host handles one in-memory frame");

    let mut frames = output.as_slice();
    while let Some(frame) = framing::read_frame(&mut frames).expect("host output is framed") {
        let event: serde_json::Value =
            serde_json::from_slice(&frame).expect("host emitted invalid JSON");
        assert!(event.is_object(), "host emitted a non-object event");
    }
});
