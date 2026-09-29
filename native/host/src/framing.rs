//! Chrome Native Messaging framing.
//!
//! A frame is a 4-byte unsigned payload length in the platform's native byte
//! order followed by exactly that many payload bytes. Frames are capped at
//! [`MAX_FRAME_SIZE`] in both directions, and an oversized length is rejected
//! before any payload memory is allocated.

use std::io::{self, Read, Write};

/// Maximum bytes in one Native Messaging payload. Chrome caps host-to-browser
/// frames at 1 MiB; TabBeam applies that bound to reads as well.
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// Size of the length prefix that precedes every payload.
pub const PREFIX_SIZE: usize = 4;

/// Why a frame could not be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The stream reported an error, or a write made no progress.
    Io,
    /// The stream ended after a partial prefix or payload.
    Truncated,
    /// The payload length exceeds [`MAX_FRAME_SIZE`].
    TooLarge,
    /// The payload buffer could not be allocated.
    AllocationFailed,
}

enum ReadError {
    Io,
    Eof { bytes_read: usize },
}

/// Reads one frame.
///
/// Returns `Ok(None)` only when the stream ends before any prefix byte. An end
/// of stream after a partial prefix or payload is [`FrameError::Truncated`].
pub fn read_frame<R: Read + ?Sized>(input: &mut R) -> Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0_u8; PREFIX_SIZE];
    match read_exact(input, &mut prefix) {
        Ok(()) => {}
        Err(ReadError::Eof { bytes_read: 0 }) => return Ok(None),
        Err(ReadError::Eof { .. }) => return Err(FrameError::Truncated),
        Err(ReadError::Io) => return Err(FrameError::Io),
    }

    let length = usize::try_from(u32::from_ne_bytes(prefix)).map_err(|_| FrameError::TooLarge)?;
    if length > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge);
    }

    let mut payload = Vec::new();
    payload
        .try_reserve_exact(length)
        .map_err(|_| FrameError::AllocationFailed)?;
    payload.resize(length, 0);

    match read_exact(input, &mut payload) {
        Ok(()) => Ok(Some(payload)),
        Err(ReadError::Eof { .. }) => Err(FrameError::Truncated),
        Err(ReadError::Io) => Err(FrameError::Io),
    }
}

/// Writes one frame and flushes the stream.
///
/// Nothing is written when the payload exceeds [`MAX_FRAME_SIZE`].
pub fn write_frame<W: Write + ?Sized>(output: &mut W, payload: &[u8]) -> Result<(), FrameError> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge);
    }
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge)?;

    write_all(output, &length.to_ne_bytes())?;
    if !payload.is_empty() {
        write_all(output, payload)?;
    }
    output.flush().map_err(|_| FrameError::Io)
}

fn read_exact<R: Read + ?Sized>(input: &mut R, buffer: &mut [u8]) -> Result<(), ReadError> {
    let mut total = 0;
    while total < buffer.len() {
        let remaining = buffer.len() - total;
        match input.read(&mut buffer[total..]) {
            Ok(0) => return Err(ReadError::Eof { bytes_read: total }),
            Ok(count) if count <= remaining => total += count,
            // A reader that claims more bytes than it was offered is broken.
            Ok(_) => return Err(ReadError::Io),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ReadError::Io),
        }
    }
    Ok(())
}

fn write_all<W: Write + ?Sized>(output: &mut W, mut buffer: &[u8]) -> Result<(), FrameError> {
    while !buffer.is_empty() {
        match output.write(buffer) {
            Ok(count) if count > 0 && count <= buffer.len() => buffer = &buffer[count..],
            Ok(_) => return Err(FrameError::Io),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(FrameError::Io),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_limit_matches_the_shared_protocol_contract() {
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/protocol/native-messaging-v1.json"
        ))
        .expect("the shared contract is JSON");
        assert_eq!(contract["max_frame_bytes"], MAX_FRAME_SIZE);
    }

    /// Serves `data` in chunks of at most `chunk` bytes and counts read calls.
    struct ChunkedReader<'a> {
        data: &'a [u8],
        chunk: usize,
        calls: usize,
    }

    impl Read for ChunkedReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            let count = buffer.len().min(self.chunk).min(self.data.len());
            buffer[..count].copy_from_slice(&self.data[..count]);
            self.data = &self.data[count..];
            Ok(count)
        }
    }

    /// Accepts at most `chunk` bytes per write and counts write/flush calls.
    #[derive(Default)]
    struct ChunkedWriter {
        data: Vec<u8>,
        chunk: usize,
        writes: usize,
        flushes: usize,
    }

    impl Write for ChunkedWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            let count = buffer.len().min(self.chunk);
            self.data.extend_from_slice(&buffer[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("read failed"))
        }
    }

    /// Returns `Interrupted` once before every successful read.
    struct InterruptingReader<'a> {
        data: &'a [u8],
        interrupt_next: bool,
    }

    impl Read for InterruptingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.interrupt_next = !self.interrupt_next;
            if self.interrupt_next {
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.data.read(buffer)
        }
    }

    struct ZeroWriter;

    impl Write for ZeroWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Ok(0)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingFlushWriter(Vec<u8>);

    impl Write for FailingFlushWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    fn prefix(length: u32) -> [u8; PREFIX_SIZE] {
        length.to_ne_bytes()
    }

    #[test]
    fn round_trip_then_eof() {
        let payload = [0x00, 0x01, 0x0a, 0x1a, 0x7f, 0xff];
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).unwrap();

        let mut input = wire.as_slice();
        assert_eq!(read_frame(&mut input), Ok(Some(payload.to_vec())));
        assert_eq!(read_frame(&mut input), Ok(None));
    }

    #[test]
    fn empty_frame_round_trips() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &[]).unwrap();
        assert_eq!(wire, prefix(0));
        assert_eq!(read_frame(&mut wire.as_slice()), Ok(Some(Vec::new())));
    }

    #[test]
    fn maximum_frame_round_trips() {
        let payload: Vec<u8> = (0..MAX_FRAME_SIZE)
            .map(|index| (index % 251) as u8)
            .collect();
        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).unwrap();
        assert_eq!(read_frame(&mut wire.as_slice()), Ok(Some(payload)));
    }

    #[test]
    fn wire_format_uses_native_byte_order() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &[0xaa, 0xbb, 0xcc, 0xdd]).unwrap();

        let expected: [u8; 8] = if cfg!(target_endian = "little") {
            [0x04, 0x00, 0x00, 0x00, 0xaa, 0xbb, 0xcc, 0xdd]
        } else {
            [0x00, 0x00, 0x00, 0x04, 0xaa, 0xbb, 0xcc, 0xdd]
        };
        assert_eq!(wire, expected);
    }

    #[test]
    fn short_reads_are_retried_until_the_frame_is_complete() {
        let mut wire = prefix(3).to_vec();
        wire.extend_from_slice(b"abc");
        let mut reader = ChunkedReader {
            data: &wire,
            chunk: 1,
            calls: 0,
        };

        assert_eq!(read_frame(&mut reader), Ok(Some(b"abc".to_vec())));
        assert_eq!(reader.calls, wire.len());
    }

    #[test]
    fn short_writes_are_retried_and_flushed_once() {
        let mut expected = prefix(3).to_vec();
        expected.extend_from_slice(b"abc");
        let mut writer = ChunkedWriter {
            chunk: 1,
            ..ChunkedWriter::default()
        };

        assert_eq!(write_frame(&mut writer, b"abc"), Ok(()));
        assert_eq!(writer.writes, expected.len());
        assert_eq!(writer.flushes, 1);
        assert_eq!(writer.data, expected);
    }

    #[test]
    fn oversized_length_is_rejected_before_allocation() {
        let wire = prefix(MAX_FRAME_SIZE as u32 + 1);
        assert_eq!(read_frame(&mut wire.as_slice()), Err(FrameError::TooLarge));
    }

    #[test]
    fn oversized_payload_writes_nothing() {
        let payload = vec![0_u8; MAX_FRAME_SIZE + 1];
        let mut wire = Vec::new();
        assert_eq!(write_frame(&mut wire, &payload), Err(FrameError::TooLarge));
        assert!(wire.is_empty());
    }

    #[test]
    fn partial_prefix_is_truncated() {
        assert_eq!(
            read_frame(&mut [0x05_u8, 0x00].as_slice()),
            Err(FrameError::Truncated)
        );
    }

    #[test]
    fn partial_payload_is_truncated() {
        let mut wire = prefix(5).to_vec();
        wire.extend_from_slice(b"abc");
        assert_eq!(read_frame(&mut wire.as_slice()), Err(FrameError::Truncated));
    }

    #[test]
    fn empty_stream_is_clean_eof() {
        assert_eq!(read_frame(&mut io::empty()), Ok(None));
    }

    #[test]
    fn read_errors_are_io_errors() {
        assert_eq!(read_frame(&mut FailingReader), Err(FrameError::Io));
    }

    #[test]
    fn interrupted_reads_are_retried() {
        let mut wire = prefix(2).to_vec();
        wire.extend_from_slice(b"hi");
        let mut reader = InterruptingReader {
            data: &wire,
            interrupt_next: false,
        };
        assert_eq!(read_frame(&mut reader), Ok(Some(b"hi".to_vec())));
    }

    #[test]
    fn writes_without_progress_are_io_errors() {
        assert_eq!(write_frame(&mut ZeroWriter, b"abc"), Err(FrameError::Io));
    }

    #[test]
    fn flush_errors_are_io_errors() {
        assert_eq!(
            write_frame(&mut FailingFlushWriter(Vec::new()), b"abc"),
            Err(FrameError::Io)
        );
    }
}
