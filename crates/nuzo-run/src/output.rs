//! Output types and per-session output sink.

use nuzo_core::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Result of a script execution.
#[derive(Debug, Clone)]
pub struct Output {
    pub value: Value,
    pub stdout: Vec<String>,
    pub duration: Duration,
}

impl Output {
    pub fn stdout_text(&self) -> String {
        self.stdout.join("\n")
    }
    pub fn first_line(&self) -> Option<&str> {
        self.stdout.first().map(|s| s.as_str())
    }
}

/// Per-session output destination, replacing global statics.
#[derive(Default)]
pub enum OutputSink {
    #[default]
    Stdout,
    Capture(Arc<Mutex<Vec<String>>>),
    Null,
    Custom(Box<dyn Write + Send>),
}

impl OutputSink {
    pub fn new_capture() -> (Self, Arc<Mutex<Vec<String>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        (OutputSink::Capture(buf.clone()), buf)
    }

    /// 返回此 sink 对应的捕获缓冲区。
    /// - `Capture(buf)` -> 共享缓冲区
    /// - `Null` -> 临时丢弃缓冲区
    /// - `Stdout` / `Custom` -> None（输出到 stdout）
    pub(crate) fn capture_buffer(&self) -> Option<Arc<Mutex<Vec<String>>>> {
        match self {
            OutputSink::Capture(buf) => Some(buf.clone()),
            OutputSink::Null => Some(Arc::new(Mutex::new(Vec::new()))),
            OutputSink::Stdout | OutputSink::Custom(_) => None,
        }
    }

    pub fn install(&self) {
        nuzo_helpers::builtins::configure_output_capture(self.capture_buffer());
    }
}
