//! BSA adapter contracts for request ordering, preflight refusal, and socket reuse.

use crate::adapter::SocketDaemon;
use crate::adapter::{BsaRadio, RequestExecutor};
use crate::broadcast::BroadcastRadio;
use crate::bsa::BsaError;

type RecordedRequests = std::rc::Rc<std::cell::RefCell<Vec<(u16, Vec<u8>)>>>;

struct RecordingDaemon(RecordedRequests);
impl RequestExecutor for RecordingDaemon {
    async fn execute(&mut self, id: u16, payload: &[u8], size: usize) -> Result<Vec<u8>, BsaError> {
        self.0.borrow_mut().push((id, payload.to_vec()));
        let mut reply = vec![0; size];
        if id == 100 {
            reply[8] = 1;
        }
        Ok(reply)
    }
}

#[test]
fn privacy_is_selected_before_advertising_and_restored_after_verification() {
    async_io::block_on(async {
        use crate::broadcast::{run_broadcast, EndReason, WaitForEnd};
        use crate::bthome::{BatteryLevel, BthomeSample};
        struct Deadline;
        impl WaitForEnd for Deadline {
            async fn wait(self) -> EndReason {
                EndReason::Deadline
            }
        }
        let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut radio = BsaRadio::new(RecordingDaemon(requests.clone()));
        run_broadcast(
            &mut radio,
            BthomeSample::new(BatteryLevel::try_from(94).expect("percentage"), 0),
            Deadline,
        )
        .await
        .expect("transaction");
        let requests = requests.borrow();
        let operations: Vec<_> = requests
            .iter()
            .map(|(id, bytes)| {
                if *id == 100 {
                    (100, 0, 0)
                } else {
                    let mask = u32::from_le_bytes(bytes[..4].try_into().expect("mask"));
                    (
                        101,
                        mask,
                        if mask == 0x20000 {
                            bytes[0x203]
                        } else if mask == 0x2000 {
                            bytes[0x201]
                        } else {
                            0
                        },
                    )
                }
            })
            .collect();
        pretty_assertions::assert_eq!(
            operations,
            vec![
                (100, 0, 0),
                (101, 0x20000, 0),
                (101, 0x8000, 0),
                (101, 0x2000, 1),
                (101, 0x2000, 0),
                (100, 0, 0),
                (101, 0x20000, 1)
            ]
        );
    });
}

struct ExistingAdvertisementDaemon;
impl RequestExecutor for ExistingAdvertisementDaemon {
    async fn execute(&mut self, id: u16, payload: &[u8], size: usize) -> Result<Vec<u8>, BsaError> {
        assert_eq!(id, 100, "preflight must only read configuration");
        assert_eq!(payload, &[0]);
        assert_eq!(size, 836);
        let mut reply = vec![0; 836];
        reply[8] = 1;
        reply[4 + 0x201] = 1;
        Ok(reply)
    }
}

#[test]
fn refuses_to_take_over_an_existing_legacy_advertisement() {
    async_io::block_on(async {
        let mut radio = BsaRadio::new(ExistingAdvertisementDaemon);
        assert!(radio.ensure_available().await.is_err());
    });
}

struct TruncatedStatusDaemon;
impl RequestExecutor for TruncatedStatusDaemon {
    async fn execute(&mut self, _: u16, _: &[u8], _: usize) -> Result<Vec<u8>, BsaError> {
        Ok(vec![0])
    }
}

#[test]
fn rejects_a_truncated_operation_status_without_panicking() {
    async_io::block_on(async {
        assert!(BsaRadio::new(TruncatedStatusDaemon).start().await.is_err());
    });
}

#[test]
fn reuses_the_control_connection_for_successive_operations() {
    async_io::block_on(async {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        use std::time::Duration;
        let path =
            std::env::temp_dir().join(format!("kindle-bsa-adapter-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).expect("fake daemon socket");
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("fake client");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("deadline");
            for _ in 0..2 {
                let mut request = [0; 5];
                if stream.read_exact(&mut request).is_err() {
                    return false;
                }
                assert_eq!(request, [3, 0, 100, 0, 0]);
                let mut response = vec![0x46, 3, 0x64, 0x80];
                response.extend_from_slice(&[0; 836]);
                stream.write_all(&response).expect("fake config reply");
            }
            true
        });
        let mut daemon = SocketDaemon::new(path.clone(), Duration::from_millis(500));
        let first = daemon.execute(100, &[0], 836).await;
        let second = daemon.execute(100, &[0], 836).await;
        drop(daemon);
        let reused = worker.join().expect("fake daemon completed");
        std::fs::remove_file(path).expect("remove fake socket");
        assert!(first.is_ok());
        assert!(second.is_ok(), "{second:?}");
        assert!(reused);
    });
}
