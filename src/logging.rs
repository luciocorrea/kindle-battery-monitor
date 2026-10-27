//! Size-limited tracing output with serialized record writes.
//!
//! The writer buffers bytes; tracing supplies JSON formatting. Oversized records
//! are discarded rather than partially persisted. These adapters do not redact
//! data: callers must exclude credentials and personal identifiers from events.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing_subscriber::fmt::MakeWriter;

/// A cloneable tracing writer factory sharing one mutex-protected append file.
///
/// When a record would cross the size limit, the existing file is truncated
/// before that complete record is written. This is not a rotating log archive.
#[derive(Clone)]
pub struct BoundedJsonLog {
    file: Arc<Mutex<File>>,
    maximum_bytes: NonZeroU64,
}

impl BoundedJsonLog {
    /// Opens or creates the append file without truncating existing contents.
    ///
    /// The limit is applied on subsequent record persistence. File IO is blocking.
    ///
    /// # Errors
    ///
    /// Returns the underlying open/create error; parent directories must exist.
    pub fn open(path: impl AsRef<Path>, maximum_bytes: NonZeroU64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            maximum_bytes,
        })
    }
}

/// A per-record buffer produced by [`BoundedJsonLog`].
///
/// `Write::flush` persists buffered bytes under the shared file lock. Drop also
/// attempts persistence but discards errors. Once a record exceeds the limit,
/// all of its bytes are discarded and writes continue to report success. Callers
/// must provide complete JSON records if they require JSON Lines output.
pub struct BoundedJsonWriter {
    log: BoundedJsonLog,
    record: Vec<u8>,
    discarded: bool,
}

impl BoundedJsonWriter {
    fn persist(&mut self) -> io::Result<()> {
        if self.discarded || self.record.is_empty() {
            return Ok(());
        }
        let mut file = match self.log.file.lock() {
            Ok(file) => file,
            Err(poisoned) => poisoned.into_inner(),
        };
        if file
            .metadata()?
            .len()
            .saturating_add(self.record.len() as u64)
            > self.log.maximum_bytes.get()
        {
            file.set_len(0)?;
        }
        file.write_all(&self.record)?;
        file.flush()?;
        self.record.clear();
        Ok(())
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.discarded {
            return Ok(bytes.len());
        }
        if (self.record.len() as u64).saturating_add(bytes.len() as u64)
            > self.log.maximum_bytes.get()
        {
            self.record.clear();
            self.discarded = true;
            return Ok(bytes.len());
        }
        self.record.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.persist()
    }
}

impl Drop for BoundedJsonWriter {
    fn drop(&mut self) {
        let _ = self.persist();
    }
}

impl<'a> MakeWriter<'a> for BoundedJsonLog {
    type Writer = BoundedJsonWriter;

    fn make_writer(&'a self) -> Self::Writer {
        BoundedJsonWriter {
            log: self.clone(),
            record: Vec::new(),
            discarded: false,
        }
    }
}
