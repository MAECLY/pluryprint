//! Native Messaging framing: each message is a 32-bit length in the
//! machine's native byte order followed by that many bytes of UTF-8 JSON.
//!
//! The limits are Chrome's: a message *to* the host may be up to 64 MiB, a
//! message *from* the host at most 1 MB
//! (developer.chrome.com/docs/extensions/develop/concepts/native-messaging).
//! A frame announcing more than [`MAX_INBOUND_BYTES`] is drained and reported
//! as [`Frame::Oversized`] so the session can answer it, instead of the host
//! allocating whatever the length says.

use std::io::{self, Read, Write};

/// Largest message the browser may send the host (64 MiB).
pub const MAX_INBOUND_BYTES: u32 = 64 * 1024 * 1024;

/// Largest message the host may send the browser (1 MB). Chrome closes the
/// port on a bigger one, so the session never builds one (see
/// [`super::session::Session::handle_bytes`]).
pub const MAX_OUTBOUND_BYTES: usize = 1024 * 1024;

/// One frame read from the browser.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A complete message body.
    Message(Vec<u8>),
    /// A frame longer than [`MAX_INBOUND_BYTES`]; its body was read and
    /// discarded, so the stream is still in sync. Carries the announced length.
    Oversized(u32),
}

/// Read the next frame. `Ok(None)` is a clean end of stream (the browser
/// closed the port) *before* a length prefix; a stream that ends inside a
/// prefix or a body is an error.
pub fn read_message<R: Read>(input: &mut R) -> io::Result<Option<Frame>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match input.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended inside a length prefix",
                ))
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_ne_bytes(len);
    if len > MAX_INBOUND_BYTES {
        // Drain the body in bounded pieces so the next frame lines up.
        let drained = io::copy(&mut input.take(u64::from(len)), &mut io::sink())?;
        if drained < u64::from(len) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended inside an oversized message",
            ));
        }
        return Ok(Some(Frame::Oversized(len)));
    }
    let mut body = vec![0u8; len as usize];
    input.read_exact(&mut body)?;
    Ok(Some(Frame::Message(body)))
}

/// Write one frame and flush it. Refuses (without writing anything) a body
/// over [`MAX_OUTBOUND_BYTES`]: a partial or oversized frame would make the
/// browser drop the connection.
pub fn write_message<W: Write>(output: &mut W, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_OUTBOUND_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a {} byte message is over the {MAX_OUTBOUND_BYTES} byte limit",
                body.len()
            ),
        ));
    }
    let len = u32::try_from(body.len()).expect("bounded above");
    output.write_all(&len.to_ne_bytes())?;
    output.write_all(body)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_ne_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn fr_120_round_trips_a_message_in_native_byte_order() {
        let mut wire = Vec::new();
        write_message(&mut wire, br#"{"v":1}"#).unwrap();
        assert_eq!(&wire[..4], &7u32.to_ne_bytes());
        let mut r = &wire[..];
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(br#"{"v":1}"#.to_vec()))
        );
        assert_eq!(read_message(&mut r).unwrap(), None, "clean end of stream");
    }

    #[test]
    fn fr_120_reads_consecutive_frames_and_an_empty_one() {
        let mut wire = frame(b"a");
        wire.extend(frame(b""));
        wire.extend(frame(b"bcd"));
        let mut r = &wire[..];
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(b"a".to_vec()))
        );
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(Vec::new()))
        );
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(b"bcd".to_vec()))
        );
        assert_eq!(read_message(&mut r).unwrap(), None);
    }

    /// A reader that hands out one byte at a time: the prefix and the body
    /// must be reassembled from short reads, as a pipe may deliver them.
    struct Trickle<'a>(&'a [u8]);
    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0.is_empty() || buf.is_empty() {
                return Ok(0);
            }
            buf[0] = self.0[0];
            self.0 = &self.0[1..];
            Ok(1)
        }
    }

    #[test]
    fn fr_120_short_reads_are_reassembled() {
        let wire = frame(b"hello");
        let mut r = Trickle(&wire);
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(b"hello".to_vec()))
        );
    }

    #[test]
    fn fr_120_truncated_streams_are_errors_not_messages() {
        let wire = frame(b"hello");
        let mut r = &wire[..2];
        assert_eq!(
            read_message(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut r = &wire[..6];
        assert_eq!(
            read_message(&mut r).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    /// An announced length over 64 MiB is drained, not allocated, and the
    /// next frame is still read correctly.
    #[test]
    fn fr_120_an_oversized_frame_is_drained_and_reported() {
        let len = MAX_INBOUND_BYTES + 1;
        let mut wire = len.to_ne_bytes().to_vec();
        wire.resize(4 + len as usize, b'x');
        wire.extend(frame(b"next"));
        let mut r = &wire[..];
        assert_eq!(read_message(&mut r).unwrap(), Some(Frame::Oversized(len)));
        assert_eq!(
            read_message(&mut r).unwrap(),
            Some(Frame::Message(b"next".to_vec()))
        );
    }

    #[test]
    fn fr_120_the_host_never_writes_more_than_one_megabyte() {
        let mut wire = Vec::new();
        let fits = vec![b' '; MAX_OUTBOUND_BYTES];
        write_message(&mut wire, &fits).unwrap();
        assert_eq!(wire.len(), 4 + MAX_OUTBOUND_BYTES);

        let mut wire = Vec::new();
        let over = vec![b' '; MAX_OUTBOUND_BYTES + 1];
        assert!(write_message(&mut wire, &over).is_err());
        assert!(wire.is_empty(), "nothing is written for a refused frame");
    }
}
