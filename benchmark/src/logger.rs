/// A simple logger that tees to stdout and a file
use log::{set_boxed_logger, set_max_level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use std::fs::File;
use std::io::{stdout, Write};
use std::sync::Mutex;

/// The TeeLogger struct. Takes a file and prints to the file and stdout
pub struct TeeLogger {
    file: Mutex<File>,
}

impl TeeLogger {
    /// Globally initializes the TeeLogger as the one and only logger.
    pub fn init(log_level: LevelFilter, file: File) -> Result<(), SetLoggerError> {
        set_max_level(log_level);
        set_boxed_logger(TeeLogger::new(log_level, file))
    }

    #[must_use]
    pub fn new(_log_level: LevelFilter, file: File) -> Box<TeeLogger> {
        Box::new(TeeLogger {
            file: Mutex::new(file),
        })
    }
}

impl Log for TeeLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // Only log messages from this package: the binary (`benchmark`) and
        // its library (`harvest_benchmark`).
        let target = metadata.target();
        target.starts_with("benchmark") || target.starts_with("harvest_benchmark")
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let stdout = stdout();
            let mut stdout_lock = stdout.lock();
            let mut file_lock = self.file.lock().unwrap();
            writeln!(stdout_lock, "{}", record.args()).unwrap();
            writeln!(file_lock, "{}", record.args()).unwrap();
        }
    }

    fn flush(&self) {
        let _ = stdout().flush();
        let _ = self.file.lock().unwrap().flush();
    }
}
