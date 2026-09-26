#![no_main]

use libfuzzer_sys::fuzz_target;
use pervue_core::framing::{self, PREFIX_SIZE};

fuzz_target!(|data: &[u8]| {
    let mut input = data;
    // Bound work per input: an empty frame consumes four bytes, so a valid
    // stream cannot hold more than size / 4 + 1 frames.
    for _ in 0..=data.len() / PREFIX_SIZE {
        match framing::read_frame(&mut input) {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }
});
