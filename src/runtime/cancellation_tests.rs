//! Cancellation before a burst avoids opening the BSA socket.

use crate::bthome::{BatteryLevel, BthomeSample};
use crate::monitor::Publication;
use crate::ownership::AdvertisementBurst;
use crate::runtime::BsaAdvertisement;
use pretty_assertions::assert_eq;

#[test]
fn cancellation_before_configuration_never_opens_the_bsa_socket() {
    async_io::block_on(async {
        let (cancel, cancelled) = async_channel::bounded(1);
        cancel.try_send(()).expect("cancel queued");
        let mut advertisement = BsaAdvertisement::new("/nonexistent/bsa-socket".into(), cancelled);
        let sample = BthomeSample::new(BatteryLevel::try_from(99).expect("percentage"), 1);
        assert_eq!(
            advertisement
                .broadcast(sample)
                .await
                .expect("clean cancellation"),
            Publication::Cancelled
        );
    });
}
