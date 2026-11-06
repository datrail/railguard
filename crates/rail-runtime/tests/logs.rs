//! The log, captured through the field formatter `logging::init` installs.

use std::sync::{Arc, Mutex};

use rail_runtime::logging::QuotedFields;
use tracing::subscriber::DefaultGuard;

#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    /// Captures this thread's log until the guard drops.
    pub fn capture(&self) -> DefaultGuard {
        let writer = self.clone();
        let subscriber = tracing_subscriber::fmt()
            .fmt_fields(QuotedFields)
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
