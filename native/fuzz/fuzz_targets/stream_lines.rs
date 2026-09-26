#![no_main]

use std::collections::VecDeque;

use libfuzzer_sys::fuzz_target;
use pervue_core::stream::LineSplitter;

fuzz_target!(|data: &[u8]| {
    let mut lines = VecDeque::new();
    let mut splitter = LineSplitter::new(4096);
    for chunk in data.chunks(7) {
        if splitter.push(chunk, &mut lines).is_err() {
            return;
        }
        lines.clear();
    }
    let _ = splitter.finish();
});
