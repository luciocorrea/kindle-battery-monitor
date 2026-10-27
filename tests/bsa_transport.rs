//! BSA framing, connection reuse, and total IO deadlines against local fake sockets.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kindle_battery_monitor::bsa::{BsaClient, BsaError};
use pretty_assertions::assert_eq;
use socket2::{Domain, SockAddr, Socket, Type};

static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);

struct FakeBsaDaemon {
    socket_path: PathBuf,
    worker: JoinHandle<()>,
}

struct SaturatedBsaDaemon {
    socket_path: PathBuf,
    release: crossbeam_channel::Sender<()>,
    worker: JoinHandle<()>,
}

impl SaturatedBsaDaemon {
    fn start() -> Self {
        let socket_path = std::env::temp_dir().join(format!(
            "bsa-full-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let address = SockAddr::unix(&socket_path).expect("fake socket address");
        let listener = Socket::new(Domain::UNIX, Type::STREAM, None).expect("create fake listener");
        listener.bind(&address).expect("bind fake listener");
        listener.listen(1).expect("listen with a small backlog");
        let mut pending = Vec::new();
        let mut saturated = false;
        for _ in 0..128 {
            let socket = Socket::new(Domain::UNIX, Type::STREAM, None).expect("create filler");
            socket.set_nonblocking(true).expect("nonblocking filler");
            let connected = socket.connect(&address);
            pending.push(socket);
            if connected.is_err() {
                saturated = true;
                break;
            }
        }
        assert!(
            saturated && pending.len() > 1,
            "establish an occupied listener backlog"
        );
        let (release, released) = crossbeam_channel::bounded::<()>(0);
        let worker = thread::spawn(move || {
            crossbeam_channel::select! {
                recv(released) -> _ => {},
                recv(crossbeam_channel::after(Duration::from_millis(500))) -> _ => {
                    let _accepted = listener.accept().expect("release one backlog slot");
                }
            }
            drop(pending);
            drop(listener);
        });
        Self {
            socket_path,
            release,
            worker,
        }
    }

    fn finish(self) {
        drop(self.release);
        self.worker.join().expect("saturated fake daemon completed");
        fs::remove_file(self.socket_path).expect("remove saturated fake socket");
    }
}

#[test]
fn saturated_listener_cannot_block_connection_establishment() {
    let daemon = SaturatedBsaDaemon::start();
    let started = Instant::now();
    let result = BsaClient::connect(&daemon.socket_path, Duration::from_millis(100));
    let elapsed = started.elapsed();
    let rejected = result.is_err();
    drop(result);
    daemon.finish();

    assert!(
        elapsed < Duration::from_millis(250),
        "connect waited {elapsed:?} for backlog capacity"
    );
    assert!(
        rejected,
        "a saturated daemon must reject the connection attempt"
    );
}

fn request_with_reply(reply: Vec<u8>, expected_reply_len: usize) -> Result<Vec<u8>, BsaError> {
    let daemon = FakeBsaDaemon::start(move |mut stream| {
        let mut request = [0; 5];
        stream.read_exact(&mut request).expect("read request");
        stream.write_all(&reply).expect("write reply");
    });
    let mut client = BsaClient::connect(&daemon.socket_path, Duration::from_secs(2))
        .expect("connect fake daemon");
    let result = client.request(100, &[0], expected_reply_len);
    drop(client);
    daemon.finish();
    result
}

#[test]
fn reply_length_must_include_a_message_id() {
    assert!(request_with_reply(vec![0x01, 0x00, 0x64, 0x80], 0).is_err());
}

#[test]
fn reply_frames_larger_than_2048_bytes_are_rejected() {
    let mut reply = vec![0x01, 0x08, 0x64, 0x80];
    reply.extend_from_slice(&[0; 2047]);
    assert!(request_with_reply(reply, 2047).is_err());
}

#[test]
fn a_reply_must_match_the_request_and_expected_payload_length() {
    assert!(request_with_reply(vec![0x04, 0x00, 0x65, 0x80, 0, 0], 2).is_err());
    assert!(request_with_reply(vec![0x04, 0x00, 0x64, 0x80, 0, 0], 836).is_err());
}

#[test]
fn asynchronous_events_are_skipped_with_a_limit_of_sixteen() {
    let event = [0x05, 0x00, 0x01, 0x40, 0xaa, 0xbb, 0xcc];
    let mut sixteen_events = event.repeat(16);
    sixteen_events.extend_from_slice(&[0x04, 0x00, 0x64, 0x80, 0, 0]);
    assert_eq!(
        request_with_reply(sixteen_events, 2).expect("skip events"),
        vec![0, 0]
    );

    let mut seventeen_events = event.repeat(17);
    seventeen_events.extend_from_slice(&[0x04, 0x00, 0x64, 0x80, 0, 0]);
    assert!(request_with_reply(seventeen_events, 2).is_err());
}

#[test]
fn fragmented_reply_cannot_extend_the_total_exchange_deadline() {
    let (cancel, cancelled) = crossbeam_channel::bounded::<()>(0);
    let daemon = FakeBsaDaemon::start(move |mut stream| {
        let mut request = [0; 5];
        stream.read_exact(&mut request).expect("read request");
        for byte in [0x04, 0x00, 0x64, 0x80, 0, 0] {
            crossbeam_channel::select! {
                recv(cancelled) -> _ => return,
                recv(crossbeam_channel::after(Duration::from_millis(60))) -> _ => {
                    if stream.write_all(&[byte]).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let mut client = BsaClient::connect(&daemon.socket_path, Duration::from_millis(150))
        .expect("connect fake daemon");
    let result = client.request(100, &[0], 2);
    drop(cancel);
    drop(client);
    daemon.finish();

    assert!(matches!(result, Err(BsaError::DeadlineExceeded)));
}

#[test]
fn closed_connections_are_distinguished_from_truncated_frames() {
    assert!(matches!(
        request_with_reply(vec![], 2),
        Err(BsaError::PeerClosed)
    ));
    let partial_header = request_with_reply(vec![0x04, 0x00], 2);
    assert!(
        matches!(partial_header, Err(BsaError::TruncatedFrame)),
        "{partial_header:?}"
    );
    assert!(matches!(
        request_with_reply(vec![0x04, 0x00, 0x64, 0x80, 0], 2),
        Err(BsaError::TruncatedFrame)
    ));
}

#[test]
fn oversized_request_is_rejected_before_transmission() {
    let daemon = FakeBsaDaemon::start(|mut stream| {
        let mut header = [0; 4];
        if stream.read_exact(&mut header).is_err() {
            return;
        }
        let payload_len = usize::from(u16::from_le_bytes([header[0], header[1]])) - 2;
        let mut payload = vec![0; payload_len];
        stream.read_exact(&mut payload).expect("consume request");
        stream
            .write_all(&[0x02, 0x00, 0x64, 0x80])
            .expect("write empty reply");
    });
    let mut client = BsaClient::connect(&daemon.socket_path, Duration::from_secs(2))
        .expect("connect fake daemon");
    let result = client.request(100, &[0; 2047], 0);
    drop(client);
    daemon.finish();

    assert!(matches!(result, Err(BsaError::RequestTooLarge)));
}

#[test]
fn a_failed_exchange_poisoned_client_sends_no_further_request() {
    let daemon = FakeBsaDaemon::start(|mut stream| {
        let mut request = [0; 5];
        stream.read_exact(&mut request).expect("read first request");
        stream
            .write_all(&[0x04, 0x00, 0x65, 0x80, 0, 0])
            .expect("write mismatched reply");
        let mut additional_bytes = Vec::new();
        stream
            .read_to_end(&mut additional_bytes)
            .expect("wait for client close");
        assert_eq!(additional_bytes, Vec::<u8>::new());
    });
    let mut client = BsaClient::connect(&daemon.socket_path, Duration::from_millis(150))
        .expect("connect fake daemon");
    assert!(matches!(
        client.request(100, &[0], 2),
        Err(BsaError::UnexpectedReply)
    ));
    let second = client.request(100, &[0], 2);
    drop(client);
    daemon.finish();
    assert!(matches!(second, Err(BsaError::Poisoned)));
}

#[test]
fn zero_timeout_is_rejected_before_connecting() {
    assert!(matches!(
        BsaClient::connect("/unused-bsa-socket", Duration::ZERO),
        Err(BsaError::InvalidTimeout)
    ));
}

impl FakeBsaDaemon {
    fn start(handler: impl FnOnce(UnixStream) + Send + 'static) -> Self {
        let socket_path = std::env::temp_dir().join(format!(
            "bsa-{}-{}.sock",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
        ));
        let listener = UnixListener::bind(&socket_path).expect("bind fake BSA daemon");
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept probe connection");
            handler(stream);
        });
        Self {
            socket_path,
            worker,
        }
    }

    fn finish(self) {
        self.worker.join().expect("fake daemon completed");
        fs::remove_file(self.socket_path).expect("remove fake socket");
    }
}

#[test]
fn get_config_request_accepts_a_fragmented_reply() {
    let daemon = FakeBsaDaemon::start(|mut stream| {
        let mut request = [0; 5];
        stream.read_exact(&mut request).expect("read request");
        assert_eq!(request, [0x03, 0x00, 0x64, 0x00, 0x00]);

        stream.write_all(&[0x46]).expect("write first length byte");
        stream.write_all(&[0x03, 0x64]).expect("write split header");
        stream.write_all(&[0x80]).expect("finish reply header");
        for chunk in [0; 836].chunks(7) {
            stream.write_all(chunk).expect("write reply fragment");
        }
    });
    let mut client = BsaClient::connect(&daemon.socket_path, Duration::from_secs(2))
        .expect("connect fake daemon");

    assert_eq!(
        client.request(100, &[0], 836).expect("read config"),
        vec![0; 836]
    );
    drop(client);
    daemon.finish();
}
