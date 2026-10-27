//! Bounded synchronous BSA framing over a Unix stream socket.
//!
//! One request is outstanding at a time. A failed exchange poisons the connection
//! to prevent partial replies from being interpreted as cleanup acknowledgements.
//! This transport does not acquire radio ownership or interpret configuration fields.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use socket2::{Domain, SockAddr, Socket, Type};
use thiserror::Error;

/// A connection, framing, or deadline failure in the BSA transport.
#[derive(Debug, Error)]
pub enum BsaError {
    /// An immediate socket connection attempt failed; no retry was made.
    #[error("could not connect to BSA socket {path}: {source}")]
    Connect {
        /// The socket path used for this attempt.
        path: PathBuf,
        /// The underlying connection error.
        source: io::Error,
    },
    /// A socket operation failed for a reason other than deadline expiration.
    #[error("BSA {operation} failed: {source}")]
    Io {
        /// The socket operation that failed.
        operation: &'static str,
        /// The underlying operating-system error.
        source: io::Error,
    },
    /// The request payload exceeds the supported 2046 bytes.
    #[error("BSA request payload is too large")]
    RequestTooLarge,
    /// A response frame length is outside `2..=2048`.
    #[error("BSA frame length is outside the supported range")]
    InvalidFrameLength,
    /// The response ID does not match the outstanding request.
    #[error("BSA response does not match the outstanding request")]
    UnexpectedReply,
    /// The reply payload length differs from the caller's expected size.
    #[error("BSA response payload length does not match the expected length")]
    UnexpectedReplyLength,
    /// More than 16 asynchronous event frames preceded the reply.
    #[error("BSA response exceeded the asynchronous event limit")]
    TooManyEvents,
    /// The total write/read exchange deadline expired.
    #[error("BSA exchange exceeded its total I/O deadline")]
    DeadlineExceeded,
    /// The timeout is zero or cannot be represented by the monotonic clock.
    #[error("BSA exchange timeout must be a finite positive duration")]
    InvalidTimeout,
    /// The peer closed before the next response header.
    #[error("BSA daemon closed the connection before replying")]
    PeerClosed,
    /// The peer closed partway through a frame.
    #[error("BSA daemon closed the connection during a frame")]
    TruncatedFrame,
    /// A previous exchange failed, so the stream cannot be reused.
    #[error("BSA connection cannot be reused after a failed exchange")]
    Poisoned,
}

/// Owns one connected BSA stream and a per-request IO deadline.
///
/// Methods block on socket IO. Dropping the client closes its connection but does
/// not stop advertising, restore identity, or release stock radio sources.
pub struct BsaClient {
    stream: UnixStream,
    timeout: Duration,
    state: ConnectionState,
}

enum ConnectionState {
    Ready,
    Poisoned,
}

#[derive(Clone, Copy)]
enum ReplyPart {
    Header,
    Payload,
}

impl BsaClient {
    /// Connects without waiting for socket backlog capacity or retrying.
    ///
    /// `timeout` must be positive and representable by the monotonic clock. It bounds
    /// each subsequent request's combined write/read exchange, not a whole burst.
    ///
    /// # Errors
    ///
    /// Returns [`BsaError::InvalidTimeout`] or an immediate connection error.
    pub fn connect(path: impl AsRef<Path>, timeout: Duration) -> Result<Self, BsaError> {
        if timeout.is_zero() || Instant::now().checked_add(timeout).is_none() {
            return Err(BsaError::InvalidTimeout);
        }
        let path = path.as_ref();
        let stream = connect_immediately(path).map_err(|source| BsaError::Connect {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self {
            stream,
            timeout,
            state: ConnectionState::Ready,
        })
    }

    /// Sends one framed request and waits for an exactly sized matching reply.
    ///
    /// Uses one total deadline across all writes and reads. At most 16 asynchronous
    /// event frames may be skipped. There is no cancellation input; the caller waits
    /// until success or an exchange failure.
    ///
    /// # Errors
    ///
    /// Rejects oversized payloads and poisoned clients before writing. Once an
    /// exchange begins, any error poisons the client, requiring a new connection.
    /// Reply IDs, frame lengths, expected payload length, and deadline are validated.
    pub fn request(
        &mut self,
        message_id: u16,
        payload: &[u8],
        expected_reply_len: usize,
    ) -> Result<Vec<u8>, BsaError> {
        match self.state {
            ConnectionState::Ready => {}
            ConnectionState::Poisoned => return Err(BsaError::Poisoned),
        }
        if payload.len() > 2046 {
            return Err(BsaError::RequestTooLarge);
        }
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or(BsaError::InvalidTimeout)?;
        let length = u16::try_from(payload.len() + 2).map_err(|_| BsaError::RequestTooLarge)?;
        let mut frame = Vec::with_capacity(payload.len() + 4);
        frame.extend_from_slice(&length.to_le_bytes());
        frame.extend_from_slice(&message_id.to_le_bytes());
        frame.extend_from_slice(payload);
        self.state = ConnectionState::Poisoned;
        let result = self.exchange(message_id, &frame, expected_reply_len, deadline);
        if result.is_ok() {
            self.state = ConnectionState::Ready;
        }
        result
    }

    fn exchange(
        &mut self,
        message_id: u16,
        frame: &[u8],
        expected_reply_len: usize,
        deadline: Instant,
    ) -> Result<Vec<u8>, BsaError> {
        self.write_all_until(frame, deadline)?;

        for skipped_events in 0..=16 {
            let (reply_id, reply) = self.read_frame(deadline)?;
            if reply_id & 0x4000 != 0 {
                if skipped_events == 16 {
                    return Err(BsaError::TooManyEvents);
                }
                continue;
            }
            if reply_id != message_id | 0x8000 {
                return Err(BsaError::UnexpectedReply);
            }
            if reply.len() != expected_reply_len {
                return Err(BsaError::UnexpectedReplyLength);
            }
            return Ok(reply);
        }
        Err(BsaError::TooManyEvents)
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<(u16, Vec<u8>), BsaError> {
        let mut header = [0; 4];
        self.read_exact_until(&mut header, deadline, ReplyPart::Header)?;
        let length = u16::from_le_bytes([header[0], header[1]]);
        if !(2..=2048).contains(&length) {
            return Err(BsaError::InvalidFrameLength);
        }
        let payload_len = usize::from(length - 2);
        let reply_id = u16::from_le_bytes([header[2], header[3]]);
        let mut reply = vec![0; payload_len];
        self.read_exact_until(&mut reply, deadline, ReplyPart::Payload)?;
        Ok((reply_id, reply))
    }

    fn read_exact_until(
        &mut self,
        mut bytes: &mut [u8],
        deadline: Instant,
        part: ReplyPart,
    ) -> Result<(), BsaError> {
        let original_len = bytes.len();
        let operation = match part {
            ReplyPart::Header => "reply header read",
            ReplyPart::Payload => "reply payload read",
        };
        while !bytes.is_empty() {
            let remaining = remaining_time(deadline)?;
            let received = match self.stream.set_read_timeout(Some(remaining)) {
                Ok(()) => self.stream.read(bytes),
                Err(source) => self.read_after_timeout_configuration_failure(bytes, source)?,
            };
            match received {
                Ok(0) => {
                    return Err(
                        if matches!(part, ReplyPart::Header) && bytes.len() == original_len {
                            BsaError::PeerClosed
                        } else {
                            BsaError::TruncatedFrame
                        },
                    );
                }
                Ok(received) => bytes = &mut bytes[received..],
                Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) => return Err(classify_io(operation, source)),
            }
        }
        remaining_time(deadline)?;
        Ok(())
    }

    fn read_after_timeout_configuration_failure(
        &mut self,
        bytes: &mut [u8],
        timeout_error: io::Error,
    ) -> Result<io::Result<usize>, BsaError> {
        // macOS can reject timeout updates after the peer closes. A single
        // nonblocking read distinguishes buffered tail bytes and EOF without
        // allowing a fallback read to outlive the exchange deadline.
        self.stream
            .set_nonblocking(true)
            .map_err(|source| BsaError::Io {
                operation: "EOF inspection configuration",
                source,
            })?;
        let received = self.stream.read(bytes);
        self.stream
            .set_nonblocking(false)
            .map_err(|source| BsaError::Io {
                operation: "blocking mode restoration",
                source,
            })?;
        if matches!(&received, Err(error) if error.kind() == io::ErrorKind::WouldBlock) {
            return Err(BsaError::Io {
                operation: "read timeout configuration",
                source: timeout_error,
            });
        }
        Ok(received)
    }

    fn write_all_until(&mut self, mut bytes: &[u8], deadline: Instant) -> Result<(), BsaError> {
        while !bytes.is_empty() {
            let remaining = remaining_time(deadline)?;
            self.stream
                .set_write_timeout(Some(remaining))
                .map_err(|source| BsaError::Io {
                    operation: "write timeout configuration",
                    source,
                })?;
            match self.stream.write(bytes) {
                Ok(0) => {
                    return Err(BsaError::Io {
                        operation: "request write",
                        source: io::Error::from(io::ErrorKind::WriteZero),
                    })
                }
                Ok(written) => bytes = &bytes[written..],
                Err(source) if source.kind() == io::ErrorKind::Interrupted => continue,
                Err(source) => return Err(classify_io("request write", source)),
            }
        }
        remaining_time(deadline)?;
        Ok(())
    }
}

fn connect_immediately(path: &Path) -> io::Result<UnixStream> {
    let address = SockAddr::unix(path)?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    socket.set_nonblocking(true)?;
    // A pending or full-backlog result is a failed connection attempt. Never
    // wait for capacity or retry; dropping the socket cancels any pending work.
    socket.connect(&address)?;
    socket.set_nonblocking(false)?;
    Ok(socket.into())
}

fn remaining_time(deadline: Instant) -> Result<Duration, BsaError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(BsaError::DeadlineExceeded);
    }
    Ok(remaining)
}

fn classify_io(operation: &'static str, source: io::Error) -> BsaError {
    if matches!(
        source.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        BsaError::DeadlineExceeded
    } else {
        BsaError::Io { operation, source }
    }
}
