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
fn charging_bursts_encode_complete_advertisements_and_finish_radio_cleanup() {
    async_io::block_on(async {
        use crate::broadcast::{run_broadcast, EndReason, WaitForEnd};
        use crate::bthome::{BatteryLevel, BthomeSample};
        struct Deadline;
        impl WaitForEnd for Deadline {
            async fn wait(self) -> EndReason {
                EndReason::Deadline
            }
        }

        for charging in [Some(true), Some(false), None] {
            let requests = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let mut radio = BsaRadio::new(RecordingDaemon(requests.clone()));
            let mut sample = BthomeSample::new(BatteryLevel::try_from(52).expect("percentage"), 42);
            let mut service = vec![0xd2, 0xfc, 0x44, 0, 42, 1, 52];
            if let Some(charging) = charging {
                sample = sample.with_charging(charging);
                service.extend_from_slice(&[0x16, u8::from(charging)]);
            }
            run_broadcast(&mut radio, sample, Deadline)
                .await
                .expect("complete burst");
            let requests = requests.borrow();
            let masks: Vec<_> = requests
                .iter()
                .map(|(id, payload)| {
                    (
                        *id,
                        if *id == 101 {
                            u32::from_le_bytes(payload[..4].try_into().expect("mask"))
                        } else {
                            0
                        },
                    )
                })
                .collect();
            assert_eq!(
                masks,
                [
                    (100, 0),
                    (101, 0x20000),
                    (101, 0x8000),
                    (101, 0x2000),
                    (101, 0x2000),
                    (100, 0),
                    (101, 0x20000)
                ]
            );
            let payload = &requests[2].1;
            assert_eq!(payload.len(), 832);
            assert_eq!(&payload[0x23c..0x240], &[2, 0x81, 0, 0]);
            assert_eq!(usize::from(payload[0x250]), service.len() - 2);
            assert_eq!(&payload[0x254..0x256], &[2, 0]);
            assert_eq!(&payload[0x258..0x25a], &service[..2]);
            assert_eq!(&payload[0x268..0x268 + service.len() - 2], &service[2..]);
            assert_eq!(&payload[0x2b0..0x2b3], &[1, 9, 6]);
            assert_eq!(&payload[0x2b3..0x2b9], b"Kindle");
            // Reconstruct AD elements from the recorded daemon request lengths.
            let mut advertisement = vec![2, 1, payload[0x219]];
            advertisement.extend_from_slice(&[payload[0x250] + 3, 0x16]);
            advertisement.extend_from_slice(&payload[0x258..0x25a]);
            advertisement.extend_from_slice(&payload[0x268..0x268 + usize::from(payload[0x250])]);
            advertisement.extend_from_slice(&[payload[0x2b2] + 1, payload[0x2b1]]);
            advertisement.extend_from_slice(&payload[0x2b3..0x2b3 + usize::from(payload[0x2b2])]);
            let mut expected = vec![2, 1, 6, if charging.is_some() { 10 } else { 8 }, 0x16];
            expected.extend_from_slice(&service);
            expected.extend_from_slice(&[7, 9, b'K', b'i', b'n', b'd', b'l', b'e']);
            assert_eq!(advertisement, expected);
            assert_eq!(
                advertisement.len(),
                if charging.is_some() { 22 } else { 20 }
            );
            assert!(advertisement.len() <= 31);
            assert_eq!(requests[1].1[0x203], 0, "public identity requested");
            assert_eq!(requests[3].1[0x201], 1, "advertising started");
            assert_eq!(
                requests[4].1[0x201], 0,
                "advertising stopped before readback"
            );
            assert_eq!(requests[6].1[0x203], 1, "rotation restored after readback");
        }
    });
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
