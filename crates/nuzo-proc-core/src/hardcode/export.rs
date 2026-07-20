//! # JSON 导出（手写，无 serde 依赖）
//!
//! 将注册表中的所有常量导出为 JSON 字符串，便于：
//! - 生成配置文档
//! - 与外部工具（如调试器、监控面板）交互
//! - 快照测试
//!
//! ## 输出格式
//!
//! ```json
//! {
//!   "count": 2,
//!   "constants": [
//!     {
//!       "name": "DEFAULT_MAX_STACK_SIZE",
//!       "type": "usize",
//!       "value": "65536",
//!       "doc": "默认最大栈大小",
//!       "module": "nuzo_core::constants"
//!     },
//!     ...
//!   ]
//! }
//! ```
//!
//! # Feature Gate
//!
//! 本模块仅在 `json-export` feature 启用时可用。

#![cfg(feature = "json-export")]

use std::io::Write;

use super::registry;
use super::types::ConstantInfo;

/// 常量导出器。
///
/// 从全局注册表快照所有常量，提供 JSON 序列化能力。
pub struct ConstantExport {
    constants: Vec<ConstantInfo>,
}

impl ConstantExport {
    /// 从全局注册表创建导出器。
    ///
    /// 调用时会快照当前注册表的所有常量（按注册顺序）。
    pub fn new() -> Self {
        Self { constants: registry::all() }
    }

    /// 返回导出器中的常量数量。
    pub fn count(&self) -> usize {
        self.constants.len()
    }

    /// 序列化为 JSON 字符串。
    pub fn to_json(&self) -> String {
        let mut buf = Vec::with_capacity(256 * self.constants.len().max(1));
        self.write_json(&mut buf).expect("writing to Vec<u8> cannot fail");
        String::from_utf8(buf).expect("write_json produces valid UTF-8")
    }

    /// 写入到任意 `Write` 实现。
    ///
    /// 返回 `io::Result<()>`，失败时返回 IO 错误。
    pub fn write_json<W: Write>(&self, writer: W) -> std::io::Result<()> {
        let mut w = writer;
        writeln!(w, "{{")?;
        writeln!(w, "  \"count\": {},", self.constants.len())?;
        writeln!(w, "  \"constants\": [")?;
        for (i, info) in self.constants.iter().enumerate() {
            write!(w, "    {{")?;
            write!(w, "\"name\": {},", json_escape(info.name))?;
            write!(w, "\"type\": {},", json_escape(info.type_name))?;
            write!(w, "\"value\": {},", json_escape(info.value_str))?;
            write!(w, "\"doc\": {},", json_escape(info.doc))?;
            write!(w, "\"module\": {}", json_escape(info.module_path))?;
            write!(w, "}}")?;
            if i + 1 < self.constants.len() {
                writeln!(w, ",")?;
            } else {
                writeln!(w)?;
            }
        }
        writeln!(w, "  ]")?;
        writeln!(w, "}}")?;
        Ok(())
    }
}

impl Default for ConstantExport {
    fn default() -> Self {
        Self::new()
    }
}

/// JSON 字符串转义（手写，避免引入 serde）。
///
/// 转义规则遵循 RFC 8259：
/// - `"` → `\"`
/// - `\` → `\\`
/// - 控制字符（U+0000..U+001F）→ `\uXXXX`
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
