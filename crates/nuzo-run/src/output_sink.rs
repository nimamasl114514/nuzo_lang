//! Output sink abstraction for capturing stdout/stderr.
//!
//! 原生环境使用 `StdoutSink`（直接写 stdout），wasm 环境使用 `StringSink`（收集到 String）。

/// 输出目标 trait
///
/// 抽象 println/print 输出，使 Engine 可在 wasm32 下捕获输出。
pub trait OutputSink {
    /// 写一行（自动追加 \n）
    fn write_line(&mut self, s: &str);

    /// 写原始字符串（不追加换行）
    fn write_raw(&mut self, s: &str);

    /// 写入带格式的一行
    fn write_line_fmt(&mut self, args: std::fmt::Arguments<'_>) {
        self.write_line(&format!("{}", args));
    }
}

/// 标准输出 sink（原生环境默认实现）
///
/// 直接写 std::io::stdout，保留原有行为。
pub struct StdoutSink;

impl OutputSink for StdoutSink {
    fn write_line(&mut self, s: &str) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = writeln!(lock, "{}", s);
    }

    fn write_raw(&mut self, s: &str) {
        use std::io::Write;
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = write!(lock, "{}", s);
    }
}

/// 字符串收集 sink（wasm 环境使用）
///
/// 收集所有写入到内部 String，便于序列化返回给 JS。
pub struct StringSink {
    buffer: String,
}

impl StringSink {
    pub fn new() -> Self {
        Self { buffer: String::new() }
    }

    /// 获取收集的内容
    pub fn into_string(self) -> String {
        self.buffer
    }

    /// 获取收集的内容的引用
    pub fn as_str(&self) -> &str {
        &self.buffer
    }
}

impl Default for StringSink {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputSink for StringSink {
    fn write_line(&mut self, s: &str) {
        self.buffer.push_str(s);
        self.buffer.push('\n');
    }

    fn write_raw(&mut self, s: &str) {
        self.buffer.push_str(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_sink_collect() {
        let mut sink = StringSink::new();
        sink.write_line("hello");
        sink.write_line("world");
        assert_eq!(sink.as_str(), "hello\nworld\n");
    }

    #[test]
    fn test_string_sink_empty() {
        let sink = StringSink::new();
        assert_eq!(sink.as_str(), "");
    }

    #[test]
    fn test_string_sink_raw() {
        let mut sink = StringSink::new();
        sink.write_raw("foo");
        sink.write_raw("bar");
        assert_eq!(sink.as_str(), "foobar");
    }

    #[test]
    fn test_string_sink_into_string() {
        let mut sink = StringSink::new();
        sink.write_line("test");
        let s = sink.into_string();
        assert_eq!(s, "test\n");
    }

    #[test]
    fn test_stdout_sink_write() {
        // 仅验证不 panic，stdout 无法在单元测试中捕获
        let mut sink = StdoutSink;
        sink.write_line("test_stdout_sink_write");
    }

    #[test]
    fn test_write_line_fmt() {
        let mut sink = StringSink::new();
        sink.write_line_fmt(format_args!("{} = {}", "x", 42));
        assert_eq!(sink.as_str(), "x = 42\n");
    }
}
