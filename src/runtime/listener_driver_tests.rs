//! Scoped notification driving while publication and its cleanup remain owned.

use super::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Trace = Rc<RefCell<Vec<&'static str>>>;

struct ScriptedNotifications {
    receiver: Pin<Box<Receiver<Result<(), PowerdEventError>>>>,
    acknowledged: async_channel::Sender<usize>,
    reads: Rc<Cell<usize>>,
    live: Rc<Cell<bool>>,
    trace: Trace,
}

impl Stream for ScriptedNotifications {
    type Item = Result<(), PowerdEventError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.as_mut().poll_next(cx) {
            Poll::Ready(Some(notification)) => {
                let reads = self.reads.get() + 1;
                self.reads.set(reads);
                self.acknowledged
                    .try_send(reads)
                    .expect("receive acknowledged");
                Poll::Ready(Some(notification))
            }
            other => other,
        }
    }
}

impl Drop for ScriptedNotifications {
    fn drop(&mut self) {
        self.live.set(false);
        self.trace.borrow_mut().push("receive-dropped");
    }
}

struct FakeNotificationSource {
    sender: async_channel::Sender<Result<(), PowerdEventError>>,
    acknowledged: Receiver<usize>,
    stream: Option<ScriptedNotifications>,
    reads: Rc<Cell<usize>>,
    live: Rc<Cell<bool>>,
}

impl FakeNotificationSource {
    fn new(trace: Trace) -> Self {
        let (sender, receiver) = bounded(8);
        let (acknowledged, acknowledgements) = bounded(8);
        let reads = Rc::new(Cell::new(0));
        let live = Rc::new(Cell::new(true));
        Self {
            sender,
            acknowledged: acknowledgements,
            stream: Some(ScriptedNotifications {
                receiver: Box::pin(receiver),
                acknowledged,
                reads: reads.clone(),
                live: live.clone(),
                trace,
            }),
            reads,
            live,
        }
    }

    fn take_stream(&mut self) -> ScriptedNotifications {
        self.stream.take().expect("one scoped receive stream")
    }
}

struct GatedPublication {
    release: Pin<Box<Receiver<()>>>,
    started: bool,
    live: Rc<Cell<bool>>,
    trace: Trace,
}

impl Future for GatedPublication {
    type Output = Publication;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Publication> {
        if !self.started {
            self.started = true;
            self.trace.borrow_mut().push("publication-running");
        }
        match self.release.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(())) => {
                self.trace.borrow_mut().extend([
                    "stop",
                    "readback",
                    "identity-restoration",
                    "source-release",
                ]);
                Poll::Ready(Publication::Cancelled)
            }
            Poll::Ready(None) => panic!("test must release publication explicitly"),
        }
    }
}

impl Drop for GatedPublication {
    fn drop(&mut self) {
        self.live.set(false);
        self.trace.borrow_mut().push("publication-dropped");
    }
}

#[test]
fn notifications_coalesce_while_publication_is_pending_and_no_driver_survives_return() {
    async_io::block_on(async {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut source = FakeNotificationSource::new(trace.clone());
        let (changed, changes) = bounded(1);
        let (release, released) = bounded(1);
        let publication_live = Rc::new(Cell::new(true));
        let cancellations = Rc::new(Cell::new(0));
        let mut receive_error = None;
        let cancel_count = cancellations.clone();
        let publication = GatedPublication {
            release: Box::pin(released),
            started: false,
            live: publication_live.clone(),
            trace: trace.clone(),
        };
        let mut driver = Box::pin(drive_listener(
            source.take_stream(),
            changed,
            &mut receive_error,
            publication,
            move || cancel_count.set(cancel_count.get() + 1),
        ));

        for expected_read in 1..=4 {
            source.sender.try_send(Ok(())).expect("notification queued");
            assert!(futures_lite::future::poll_once(driver.as_mut())
                .await
                .is_none());
            assert_eq!(source.acknowledged.try_recv(), Ok(expected_read));
            assert_eq!(changes.len(), 1, "coalescer retains one pending trigger");
            assert!(publication_live.get());
        }
        assert_eq!(&*trace.borrow(), &["publication-running"]);
        assert_eq!(source.reads.get(), 4);
        assert_eq!(changes.try_recv(), Ok(()));
        assert!(matches!(
            changes.try_recv(),
            Err(async_channel::TryRecvError::Empty)
        ));

        release
            .try_send(())
            .expect("publication may finish cleanup");
        assert_eq!(
            futures_lite::future::poll_once(driver.as_mut()).await,
            Some(Publication::Cancelled)
        );
        assert!(!source.live.get());
        assert!(!publication_live.get());
        assert!(
            source.sender.is_closed(),
            "no receive future survives return"
        );
        assert!(changes.is_closed());
        assert_eq!(cancellations.get(), 0);
        drop(driver);
        assert!(receive_error.is_none());
        assert_eq!(
            &trace.borrow()[1..5],
            &["stop", "readback", "identity-restoration", "source-release"]
        );
    });
}

#[test]
fn receive_failure_cancels_once_and_keeps_publication_alive_through_owned_cleanup() {
    async_io::block_on(async {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut source = FakeNotificationSource::new(trace.clone());
        let (changed, changes) = bounded(1);
        let (release, released) = bounded(1);
        let publication_live = Rc::new(Cell::new(true));
        let cancellations = Rc::new(Cell::new(0));
        let mut receive_error = None;
        let cancel_count = cancellations.clone();
        let cancellation_trace = trace.clone();
        let publication = GatedPublication {
            release: Box::pin(released),
            started: false,
            live: publication_live.clone(),
            trace: trace.clone(),
        };
        let mut driver = Box::pin(drive_listener(
            source.take_stream(),
            changed,
            &mut receive_error,
            publication,
            move || {
                cancel_count.set(cancel_count.get() + 1);
                cancellation_trace.borrow_mut().push("cancel-requested");
            },
        ));
        source.sender.try_send(Ok(())).expect("first event queued");
        assert!(futures_lite::future::poll_once(driver.as_mut())
            .await
            .is_none());
        assert_eq!(source.acknowledged.try_recv(), Ok(1));
        source
            .sender
            .try_send(Err(PowerdEventError::Receive(zbus::Error::Failure(
                "scripted receive failure".into(),
            ))))
            .expect("receive failure queued");
        assert!(futures_lite::future::poll_once(driver.as_mut())
            .await
            .is_none());
        assert_eq!(source.acknowledged.try_recv(), Ok(2));
        assert!(
            changes.is_closed(),
            "failed receiver closes sampling triggers"
        );
        assert_eq!(
            changes.try_recv(),
            Ok(()),
            "already queued event is retained"
        );
        assert!(matches!(
            changes.try_recv(),
            Err(async_channel::TryRecvError::Closed)
        ));
        assert_eq!(cancellations.get(), 1);
        assert!(
            publication_live.get(),
            "cancellation must not drop publication"
        );
        assert_eq!(
            &*trace.borrow(),
            &["publication-running", "cancel-requested"]
        );

        source
            .sender
            .try_send(Err(PowerdEventError::Closed))
            .expect("later event queued");
        assert!(futures_lite::future::poll_once(driver.as_mut())
            .await
            .is_none());
        assert_eq!(source.reads.get(), 2, "failed stream is not driven again");
        assert_eq!(
            cancellations.get(),
            1,
            "cancellation is requested exactly once"
        );
        assert!(publication_live.get());

        release.try_send(()).expect("publication cleanup released");
        assert_eq!(
            futures_lite::future::poll_once(driver.as_mut()).await,
            Some(Publication::Cancelled)
        );
        assert!(!publication_live.get());
        assert!(!source.live.get());
        assert!(source.sender.is_closed());
        drop(driver);
        assert!(matches!(
            receive_error,
            Some(PowerdEventError::Receive(zbus::Error::Failure(ref reason)))
                if reason == "scripted receive failure"
        ));
        assert_eq!(
            &trace.borrow()[2..6],
            &["stop", "readback", "identity-restoration", "source-release"]
        );
    });
}
