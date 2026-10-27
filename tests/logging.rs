//! Complete-record persistence and bounded log size in a temporary file.

use kindle_battery_monitor::logging::BoundedJsonLog;
use pretty_assertions::assert_eq;
use std::io::Write;
use std::num::NonZeroU64;
use tracing_subscriber::fmt::MakeWriter;

#[test]
fn complete_json_records_rotate_before_the_log_exceeds_its_limit() {
    let directory = tempfile::tempdir().expect("test directory");
    let path = directory.path().join("monitor.jsonl");
    let log = BoundedJsonLog::open(&path, NonZeroU64::new(20).expect("nonzero limit"))
        .expect("log opens");

    for record in [b"{\"a\":1}\n", b"{\"a\":2}\n", b"{\"a\":3}\n"] {
        let mut writer = log.make_writer();
        writer.write_all(record).expect("record accepted");
    }

    assert_eq!(std::fs::read(&path).expect("bounded log"), b"{\"a\":3}\n");
    assert!(std::fs::metadata(&path).expect("log metadata").len() <= 20);
}
