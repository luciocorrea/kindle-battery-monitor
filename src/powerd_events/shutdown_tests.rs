//! Actual dedicated-listener shutdown ownership at the zbus socket boundary.

use super::{PowerdCleanupError, PowerdEvents};
use async_io::Async;
use futures_lite::future;
use std::future::Future;
use std::io::{self, BufRead};
use std::os::fd::BorrowedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use zbus::connection::socket::{Socket, Split, WriteHalf};
use zbus::{Connection, Message};

struct PrivateBus {
    child: Child,
    _directory: tempfile::TempDir,
    socket: PathBuf,
}

impl PrivateBus {
    fn start() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("pd-")
            .tempdir_in("/tmp")
            .expect("short private socket directory");
        let socket = directory.path().join("bus");
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}", socket.display()))
            .stdout(Stdio::piped())
            .spawn()
            .expect("isolated test bus starts");
        let mut ready = String::new();
        io::BufReader::new(child.stdout.take().expect("bus address output"))
            .read_line(&mut ready)
            .expect("test bus reports readiness");
        assert!(!ready.is_empty());
        Self {
            child,
            _directory: directory,
            socket,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
struct SocketControl {
    block_remove: AtomicBool,
    block_close: AtomicBool,
    fail_remove: AtomicBool,
    fail_close: AtomicBool,
    remove_started: async_channel::Sender<()>,
    close_started: async_channel::Sender<()>,
    remove_release: async_channel::Receiver<()>,
    close_release: async_channel::Receiver<()>,
    remove_calls: AtomicUsize,
    close_calls: AtomicUsize,
}

struct SocketGates {
    control: Arc<SocketControl>,
    remove_started: async_channel::Receiver<()>,
    close_started: async_channel::Receiver<()>,
    remove_release: async_channel::Sender<()>,
    close_release: async_channel::Sender<()>,
}

impl SocketGates {
    fn new() -> Self {
        let (remove_started, remove_observed) = async_channel::bounded(1);
        let (close_started, close_observed) = async_channel::bounded(1);
        let (remove_release, remove_wait) = async_channel::bounded(1);
        let (close_release, close_wait) = async_channel::bounded(1);
        let control = Arc::new(SocketControl {
            block_remove: AtomicBool::new(false),
            block_close: AtomicBool::new(false),
            fail_remove: AtomicBool::new(false),
            fail_close: AtomicBool::new(false),
            remove_started,
            close_started,
            remove_release: remove_wait,
            close_release: close_wait,
            remove_calls: AtomicUsize::new(0),
            close_calls: AtomicUsize::new(0),
        });
        Self {
            control,
            remove_started: remove_observed,
            close_started: close_observed,
            remove_release,
            close_release,
        }
    }

    fn release_cleanup_gates(&self) {
        let _ = self.remove_release.try_send(());
        let _ = self.close_release.try_send(());
    }
}

#[derive(Debug)]
struct GatedWrite {
    socket: Arc<Async<UnixStream>>,
    control: Arc<SocketControl>,
}

// zbus reexports its async-trait macro, so the socket fake adds no dependency.
#[zbus::export::async_trait::async_trait]
impl WriteHalf for GatedWrite {
    async fn sendmsg(&mut self, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<usize> {
        self.socket.sendmsg(bytes, fds).await
    }

    async fn send_message(&mut self, message: &Message) -> zbus::Result<()> {
        if message.header().member().map(|member| member.as_str()) == Some("RemoveMatch") {
            self.control.remove_calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.control.remove_started.try_send(());
            if self.control.block_remove.load(Ordering::SeqCst) {
                let _ = self.control.remove_release.recv().await;
            }
            if self.control.fail_remove.load(Ordering::SeqCst) {
                return Err(zbus::Error::Failure("scripted unsubscribe failure".into()));
            }
        }
        self.socket.send_message(message).await
    }

    async fn close(&mut self) -> io::Result<()> {
        self.control.close_calls.fetch_add(1, Ordering::SeqCst);
        let _ = self.control.close_started.try_send(());
        if self.control.block_close.load(Ordering::SeqCst) {
            let _ = self.control.close_release.recv().await;
        }
        if self.control.fail_close.load(Ordering::SeqCst) {
            return Err(io::Error::other("scripted close failure"));
        }
        self.socket.close().await
    }

    fn can_pass_unix_fd(&self) -> bool {
        WriteHalf::can_pass_unix_fd(&self.socket)
    }

    async fn peer_credentials(&mut self) -> io::Result<zbus::fdo::ConnectionCredentials> {
        WriteHalf::peer_credentials(&mut self.socket).await
    }
}

#[derive(Debug)]
struct GatedSocket(Arc<Async<UnixStream>>, Arc<SocketControl>);

impl Socket for GatedSocket {
    type ReadHalf = Arc<Async<UnixStream>>;
    type WriteHalf = GatedWrite;

    fn split(self) -> Split<Self::ReadHalf, Self::WriteHalf> {
        Split::new(
            self.0.clone(),
            GatedWrite {
                socket: self.0,
                control: self.1,
            },
        )
    }
}

async fn gated_connection(bus: &PrivateBus, gates: &SocketGates) -> Connection {
    let socket = Arc::new(
        Async::<UnixStream>::connect(&bus.socket)
            .await
            .expect("private bus socket"),
    );
    zbus::connection::Builder::socket(GatedSocket(socket, gates.control.clone()))
        .build()
        .await
        .expect("gated client connects")
}

async fn watchdog<T>(operation: impl Future<Output = T>) -> T {
    future::or(operation, async {
        async_io::Timer::after(Duration::from_secs(4)).await;
        panic!("private-bus shutdown watchdog expired")
    })
    .await
}

#[test]
fn both_cleanup_timeouts_release_the_actual_dedicated_connection() {
    let bus = PrivateBus::start();
    async_io::block_on(watchdog(async {
        let gates = SocketGates::new();
        let connection = gated_connection(&bus, &gates).await;
        // graceful_shutdown consumes its clone on its first poll, then observes
        // ConnectionInner destruction. This witness owns no listener state.
        let mut released = Box::pin(connection.clone().graceful_shutdown());
        assert!(future::poll_once(released.as_mut()).await.is_none());
        let events = PowerdEvents::dedicated(connection)
            .await
            .expect("dedicated subscription");
        gates.control.block_remove.store(true, Ordering::SeqCst);
        gates.control.block_close.store(true, Ordering::SeqCst);
        let (expire_remove, remove_expired) = async_channel::bounded::<()>(1);
        let (expire_all, all_expired) = async_channel::bounded::<()>(1);
        let mut shutdown = Box::pin(events.shutdown_with_deadlines(
            async {
                let _ = remove_expired.recv().await;
            },
            async {
                let _ = all_expired.recv().await;
            },
        ));
        assert!(future::poll_once(shutdown.as_mut()).await.is_none());
        gates
            .remove_started
            .recv()
            .await
            .expect("unsubscribe started");
        assert!(future::poll_once(released.as_mut()).await.is_none());
        expire_remove.send(()).await.expect("expire unsubscribe");
        assert!(future::poll_once(shutdown.as_mut()).await.is_none());
        gates
            .close_started
            .recv()
            .await
            .expect("close attempted after unsubscribe timeout");
        assert!(future::poll_once(released.as_mut()).await.is_none());
        expire_all.send(()).await.expect("expire shared deadline");
        assert!(matches!(
            shutdown.await.as_slice(),
            [
                PowerdCleanupError::UnsubscribeTimeout,
                PowerdCleanupError::CloseTimeout
            ]
        ));
        let ownership_released = future::poll_once(released.as_mut()).await.is_some();
        // Also unblock a regressed queued cleanup task before reporting failure,
        // so the test itself does not retain a gated resource in the background.
        gates.release_cleanup_gates();
        if !ownership_released {
            watchdog(released).await;
        }
        assert!(
            ownership_released,
            "cleanup retained ConnectionInner beyond its deadline"
        );
        assert_eq!(gates.control.remove_calls.load(Ordering::SeqCst), 1);
        assert_eq!(gates.control.close_calls.load(Ordering::SeqCst), 1);
    }));
}

#[test]
fn dropping_a_dedicated_subscription_queues_no_remove_match_and_releases_ownership() {
    let bus = PrivateBus::start();
    async_io::block_on(watchdog(async {
        let gates = SocketGates::new();
        let connection = gated_connection(&bus, &gates).await;
        let mut released = Box::pin(connection.clone().graceful_shutdown());
        assert!(future::poll_once(released.as_mut()).await.is_none());
        let events = PowerdEvents::dedicated(connection)
            .await
            .expect("dedicated subscription");
        gates.control.block_remove.store(true, Ordering::SeqCst);
        drop(events);
        let ownership_released = future::poll_once(released.as_mut()).await.is_some();
        gates.release_cleanup_gates();
        if !ownership_released {
            watchdog(released).await;
        }
        assert!(
            ownership_released,
            "Drop queued a task retaining connection ownership"
        );
        assert_eq!(gates.control.remove_calls.load(Ordering::SeqCst), 0);
        assert_eq!(gates.control.close_calls.load(Ordering::SeqCst), 0);
    }));
}

#[test]
fn shutdown_preserves_native_unsubscribe_and_close_failures_in_order() {
    for fail in [false, true] {
        let bus = PrivateBus::start();
        async_io::block_on(watchdog(async {
            let gates = SocketGates::new();
            let connection = gated_connection(&bus, &gates).await;
            let mut released = Box::pin(connection.clone().graceful_shutdown());
            assert!(future::poll_once(released.as_mut()).await.is_none());
            let events = PowerdEvents::dedicated(connection)
                .await
                .expect("dedicated subscription");
            gates.control.fail_remove.store(fail, Ordering::SeqCst);
            gates.control.fail_close.store(fail, Ordering::SeqCst);
            let failures = events
                .shutdown_with_deadlines(future::pending(), future::pending())
                .await;
            if fail {
                match failures.as_slice() {
                    [PowerdCleanupError::Unsubscribe(unsubscribe), PowerdCleanupError::Close(close)] =>
                    {
                        assert!(matches!(unsubscribe, zbus::Error::Failure(message)
                            if message == "scripted unsubscribe failure"));
                        match close {
                            zbus::Error::InputOutput(source) => {
                                assert_eq!(source.kind(), io::ErrorKind::Other)
                            }
                            other => panic!("unexpected close failure: {other:?}"),
                        }
                    }
                    other => panic!("unexpected cleanup report: {other:?}"),
                }
            } else {
                assert!(failures.is_empty());
            }
            assert!(future::poll_once(released.as_mut()).await.is_some());
            assert_eq!(gates.control.remove_calls.load(Ordering::SeqCst), 1);
            assert_eq!(gates.control.close_calls.load(Ordering::SeqCst), 1);
        }));
    }
}

#[test]
fn cancellation_during_close_preserves_an_already_observed_unsubscribe_failure() {
    let bus = PrivateBus::start();
    async_io::block_on(watchdog(async {
        let gates = SocketGates::new();
        let connection = gated_connection(&bus, &gates).await;
        let mut released = Box::pin(connection.clone().graceful_shutdown());
        assert!(future::poll_once(released.as_mut()).await.is_none());
        let events = PowerdEvents::dedicated(connection)
            .await
            .expect("dedicated subscription");
        gates.control.fail_remove.store(true, Ordering::SeqCst);
        gates.control.block_close.store(true, Ordering::SeqCst);
        let mut failures = Vec::new();
        let mut shutdown = Box::pin(events.shutdown_recording(
            future::pending(),
            future::pending(),
            &mut failures,
        ));
        let close_is_pending = future::or(
            async {
                shutdown.as_mut().await;
                false
            },
            async {
                gates
                    .close_started
                    .recv()
                    .await
                    .expect("close reached its gate");
                true
            },
        )
        .await;
        assert!(close_is_pending, "close is held, not completed");
        assert_eq!(gates.control.close_calls.load(Ordering::SeqCst), 1);
        drop(shutdown);
        // Cancelling releases the mutable borrow and foreground resources, but
        // must not erase the error observed before the close phase began.
        assert!(matches!(
            failures.as_slice(),
            [PowerdCleanupError::Unsubscribe(zbus::Error::Failure(message))]
                if message == "scripted unsubscribe failure"
        ));
        let ownership_released = future::poll_once(released.as_mut()).await.is_some();
        gates.release_cleanup_gates();
        if !ownership_released {
            watchdog(released).await;
        }
        assert!(
            ownership_released,
            "cancelled close retained foreground connection state"
        );
        assert_eq!(gates.control.remove_calls.load(Ordering::SeqCst), 1);
    }));
}

#[test]
fn ignored_unicast_messages_yield_to_a_foreground_sibling() {
    let bus = PrivateBus::start();
    async_io::block_on(watchdog(async {
        let gates = SocketGates::new();
        let connection = gated_connection(&bus, &gates).await;
        let destination = connection
            .unique_name()
            .expect("listener unique name")
            .to_owned();
        let mut events = PowerdEvents::dedicated(connection)
            .await
            .expect("dedicated subscription");
        let mut observed = events.stream.clone();
        let publisher = zbus::connection::Builder::unix_stream(
            UnixStream::connect(&bus.socket).expect("publisher socket"),
        )
        .build()
        .await
        .expect("publisher connection");
        for _ in 0..3 {
            publisher
                .emit_signal(
                    Some(destination.as_str()),
                    "/unrelated",
                    "com.example.Noise",
                    "noise",
                    &(),
                )
                .await
                .expect("send ignored unicast signal");
        }
        publisher
            .emit_signal(
                Some(destination.as_str()),
                "/default",
                "com.lab126.powerd",
                "battLevelChanged",
                &83_i32,
            )
            .await
            .expect("send matching marker");
        // The clone acknowledges real receipt without advancing the candidate's
        // own cursor, so its first poll sees a known-ready ignored-message queue.
        let mut ignored = 0;
        loop {
            let message = futures_lite::StreamExt::next(&mut observed)
                .await
                .expect("probe stream open")
                .expect("probe receives message");
            match message.header().member().map(|member| member.as_str()) {
                Some("noise") => ignored += 1,
                Some("battLevelChanged") => break,
                _ => {}
            }
        }
        assert_eq!(ignored, 3);
        drop(observed);
        let sibling_ran_first = future::or(
            async {
                events
                    .next()
                    .await
                    .expect("matching marker eventually arrives");
                false
            },
            async { true },
        )
        .await;
        assert!(
            sibling_ran_first,
            "filtering ignored messages must yield before consuming the marker"
        );
        events
            .next()
            .await
            .expect("matching marker remains after cancellation");
        assert!(events
            .shutdown_with_deadlines(future::pending(), future::pending())
            .await
            .is_empty());
    }));
}
