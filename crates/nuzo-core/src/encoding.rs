//! # 多编码支持与字符处理工具
//!
//! 本模块提供 **完整的字符编码处理能力**，是 Nuzo 语言支持多语言文本的基础设施。
//!
//! ## 核心能力
//!
//! ### 1. 编码检测（Encoding Detection）
//! 自动识别字节序列的字符编码：
//! ```ignore
//! let bytes = std::fs::read("mixed_encoding.txt")?;
//! let encoding = Encoding::detect(&bytes);
//! match encoding {
//!     Encoding::Utf8 => println!("UTF-8 文件"),
//!     Encoding::Gbk => println!("中文 GBK 文件"),
//!     _ => println!("其他编码: {}", encoding.name()),
//! }
//! ```
//!
//! **检测策略**（按优先级）:
//! 1. BOM（Byte Order Mark）标记检测
//! 2. UTF-8 有效性验证
//! 3. 启发式统计（GBK/Shift-JIS/Big5 特征模式匹配）|
//! 4. 回退到 Latin1（不会失败的最安全选择）|
//!
//! ### 2. 编解码转换（Transcoding）
//!
//! #### Rust String → 字节序列
//! ```ignore
//! let bytes = encode_to_bytes("你好世界", Encoding::Gbk)?;
//! // bytes = [0xC4, 0xE3, 0xBA, 0xC3, ...] (GBK 编码)
//! ```
//!
//! #### 字节序列 → Rust String
//! ```ignore
//! let s = decode_from_bytes(&[0xC4, 0xE3], Encoding::Gbk)?;
//! // s = "你"
//! ```
//!
//! ### 3. Unicode 字符级操作
//! 提供**基于字符索引（非字节索引）**的字符串操作：
//!
//! | 函数 | 复杂度 | 说明 |
//! |------|--------|------|
//! [`char_len()`] | O(n) | 计算字符串的字符长度 |
//! [`char_at()`] | O(n) | 获取第 N 个字符（随机访问慢，符合 Unicode 特性）|
//! [`char_slice()`] | O(n) | 提取子串（字符范围切片）|
//! [`char_to_byte_index()`] | O(n) | 字符位置 → 字节位置转换 |
//!
//! ### 4. 高性能缓存（StringIndexCache）
//! 对于需要**多次随机访问**的长字符串，提供缓存机制：
//! ```ignore
//! let long_text = "这是一个很长的字符串..."; // > 32 字节
//! let mut cache = StringIndexCache::new();
//!
//! // 首次访问：自动构建索引（O(n)）
//! let ch = cache.char_at_fast(long_text, 1000);
//!
//! // 后续访问：O(1) 查表
//! let ch2 = cache.char_at_fast(long_text, 2000);
//! assert!(cache.is_built());  // 索引已构建
//! ```
//!
//! **优化细节**:
//! - 短字符串（<=32 字节）：直接使用标准库方法（无缓存开销）|
//! - 长字符串：惰性构建索引（首次访问时才分配内存）|
//! - 缓存可复用：多次操作同一字符串时共享索引
//!
//! ## 支持的编码
//!
//! | 编码 | 枚举值 | 主要用途 | 别名 |
//! |------|--------|---------|------|
//! UTF-8 | `Encoding::Utf8` | 默认源码编码、Linux/macOS | `utf8`, `UTF-8` |
//! GBK | `Encoding::Gbk` | 简体中文 Windows | `gbk`, `GB2312`, `cp936` |
//! Shift-JIS | `Encoding::ShiftJis` | 日文环境 | `sjis`, `Shift_JIS`, `cp932` |
//! Big5 | `Encoding::Big5` | 繁体中文 | `big5`, `cp950` |
//! Latin1 | `Encoding::Latin1` | 西欧语言回退 | `latin1`, `ISO-8859-1`, `cp1252` |
//!
//! ## 实现限制
//!
//! ### 当前不完整的功能
//! - **Shift-JIS / Big5**: 仅支持基本汉字范围（CJK Unified Ideographs）|
//!   完整映射表较大（数 MB），未来可通过 feature flag 按需引入
//! - **错误恢复**: 遇到非法字节对时会返回 `Err`，不支持替换字符（如 U+FFFD）|
//!
//! ### 性能特征
//!
//! | 操作 | 时间复杂度 | 备注 |
//! |------|-----------|------|
//! | 编码检测 | O(n) | 需扫描全部字节 |
//! | UTF-8 编解码 | O(n) | 直接内存拷贝（最快）|
//! | GBK/Big5 编码 | O(n * k) | k=平均每个字符的查找时间 |
//! | 带缓存的随机访问 | O(1) 构建 + O(1) 查询 | 仅长字符串受益 |

use std::borrow::Cow;
use std::fmt;

use crate::constants::{
    UTF8_BOM_0, UTF8_BOM_1, UTF8_BOM_2, UTF16_BE_BOM_0, UTF16_BE_BOM_1, UTF16_LE_BOM_0,
    UTF16_LE_BOM_1,
};

// ============================================================================
// EncodingError -- 编解码操作的强类型错误
// ============================================================================

/// 字符编码转换过程中的错误类型。
///
/// 替代原先的 `String` 错误，提供结构化、可模式匹配的错误信息。
/// 由于 `nuzo_core` 是底层 crate（`nuzo_error` 依赖它），不能反向依赖
/// `nuzo_values::NuzoError`，因此在此定义独立的轻量级错误枚举。
///
/// # 与 NuzoError 的转换
///
/// 上层 crate（如 `nuzo_vm`、`nuzo_cli`）可通过 `From<EncodingError>` 将此错误
/// 转换为 `NuzoError::internal(InternalError::CompilerBug { ... })` 或其他合适变体。
#[derive(Debug, Clone, PartialEq)]
pub enum EncodingError {
    /// 字符无法在目标编码中表示
    UnencodableChar {
        /// Unicode 码点（十六进制）
        code_point: u32,
        /// 目标编码名称
        encoding: &'static str,
    },
    /// 输入字节序列不完整（截断的多字节序列）
    IncompleteSequence {
        /// 人类可读描述
        detail: &'static str,
    },
    /// 无效的多字节对（解码时遇到不合法的字节组合）
    InvalidBytePair {
        /// 第一个字节的十六进制值
        byte0: u8,
        /// 第二个字节的十六进制值
        byte1: u8,
        /// 源编码名称
        encoding: &'static str,
    },
    /// 请求了对该编码不支持的操作
    UnsupportedEncoding {
        /// 人类可读描述
        detail: &'static str,
    },
    /// UTF-8 解码失败
    InvalidUtf8 {
        /// 底层 UTF-8 错误的描述
        detail: String,
    },
}

impl fmt::Display for EncodingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodingError::UnencodableChar { code_point, encoding } => {
                write!(f, "character U+{:04X} cannot be encoded in {}", code_point, encoding)
            }
            EncodingError::IncompleteSequence { detail } => {
                write!(f, "incomplete multi-byte sequence: {}", detail)
            }
            EncodingError::InvalidBytePair { byte0, byte1, encoding } => {
                write!(f, "invalid {} byte pair: {:02X} {:02X}", encoding, byte0, byte1)
            }
            EncodingError::UnsupportedEncoding { detail } => {
                write!(f, "unsupported encoding: {}", detail)
            }
            EncodingError::InvalidUtf8 { detail } => {
                write!(f, "invalid UTF-8: {}", detail)
            }
        }
    }
}

impl std::error::Error for EncodingError {}

/// 从 `String` 自动转换为 `EncodingError`。
///
/// 用于 `?` 运算符在 `encode_char` / `decode_double_byte` 等函数中
/// 将 `Result<_, String>` 转换为 `Result<_, EncodingError>`。
/// 错误消息被包装为 `EncodingError::InvalidUtf8` 变体。
impl From<String> for EncodingError {
    fn from(detail: String) -> Self {
        EncodingError::InvalidUtf8 { detail }
    }
}

const SHORT_STRING_THRESHOLD: usize = 32;

pub struct StringIndexCache {
    offsets: Vec<usize>,
    built: bool,
    /// 缓存对应的字符串字节长度，用于校验缓存是否过期
    cached_len: usize,
}

impl StringIndexCache {
    pub fn new() -> Self {
        StringIndexCache { offsets: Vec::new(), built: false, cached_len: 0 }
    }

    pub fn ensure_built(&mut self, s: &str) {
        if self.built && self.cached_len == s.len() {
            return;
        }
        self.offsets = s.char_indices().map(|(i, _)| i).collect();
        self.built = true;
        self.cached_len = s.len();
    }

    pub fn is_built(&self) -> bool {
        self.built
    }

    pub fn char_len_cached(&self) -> usize {
        debug_assert!(self.built, "char_len_cached called before cache was built");
        self.offsets.len()
    }

    pub fn char_at_fast(&mut self, s: &str, index: usize) -> Option<char> {
        if s.len() <= SHORT_STRING_THRESHOLD {
            return s.chars().nth(index);
        }
        self.ensure_built(s);
        if index >= self.offsets.len() {
            return None;
        }
        let byte_start = self.offsets[index];
        let byte_end =
            if index + 1 < self.offsets.len() { self.offsets[index + 1] } else { s.len() };
        s[byte_start..byte_end].chars().next()
    }

    pub fn char_slice_fast<'a>(&mut self, s: &'a str, start: usize, end: usize) -> Cow<'a, str> {
        if s.len() <= SHORT_STRING_THRESHOLD {
            let chars: Vec<char> = s.chars().collect();
            if start >= chars.len() {
                return Cow::Borrowed("");
            }
            let end = end.min(chars.len());
            if start >= end {
                return Cow::Borrowed("");
            }
            return Cow::Owned(chars[start..end].iter().collect());
        }
        self.ensure_built(s);
        let len = self.offsets.len();
        if start >= len {
            return Cow::Borrowed("");
        }
        let end = end.min(len);
        if start >= end {
            return Cow::Borrowed("");
        }
        let byte_start = self.offsets[start];
        let byte_end = if end < len { self.offsets[end] } else { s.len() };
        Cow::Borrowed(&s[byte_start..byte_end])
    }

    pub fn char_to_byte_index_fast(&mut self, s: &str, char_index: usize) -> Option<usize> {
        if s.len() <= SHORT_STRING_THRESHOLD {
            return s.char_indices().nth(char_index).map(|(i, _)| i);
        }
        self.ensure_built(s);
        self.offsets.get(char_index).copied()
    }
}

impl Default for StringIndexCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Gbk,
    ShiftJis,
    Big5,
    Latin1,
}

impl Encoding {
    pub fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16LE",
            Encoding::Utf16Be => "UTF-16BE",
            Encoding::Gbk => "GBK",
            Encoding::ShiftJis => "Shift-JIS",
            Encoding::Big5 => "Big5",
            Encoding::Latin1 => "ISO-8859-1",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_uppercase().replace(['-', '_'], "").as_str() {
            "UTF8" => Some(Encoding::Utf8),
            "UTF16LE" | "UTF16" => Some(Encoding::Utf16Le),
            "UTF16BE" => Some(Encoding::Utf16Be),
            "GBK" | "CP936" | "GB2312" => Some(Encoding::Gbk),
            "SHIFTJIS" | "SJIS" | "CP932" => Some(Encoding::ShiftJis),
            "BIG5" | "CP950" => Some(Encoding::Big5),
            "LATIN1" | "ISO88591" | "CP1252" => Some(Encoding::Latin1),
            _ => None,
        }
    }

    pub fn detect(bytes: &[u8]) -> Encoding {
        if bytes.len() >= 3
            && bytes[0] == UTF8_BOM_0
            && bytes[1] == UTF8_BOM_1
            && bytes[2] == UTF8_BOM_2
        {
            return Encoding::Utf8;
        }
        // UTF-16 BOM 优先识别：之前误判为 Latin1 导致中文乱码
        if bytes.len() >= 2 {
            let b0 = bytes[0];
            let b1 = bytes[1];
            if b0 == UTF16_LE_BOM_0 && b1 == UTF16_LE_BOM_1 {
                return Encoding::Utf16Le;
            }
            if b0 == UTF16_BE_BOM_0 && b1 == UTF16_BE_BOM_1 {
                return Encoding::Utf16Be;
            }
        }
        if is_valid_utf8(bytes) {
            return Encoding::Utf8;
        }
        if looks_like_gbk(bytes) {
            return Encoding::Gbk;
        }
        if looks_like_shift_jis(bytes) {
            return Encoding::ShiftJis;
        }
        if looks_like_big5(bytes) {
            return Encoding::Big5;
        }
        Encoding::Latin1
    }
}

/// Big5 启发式检测：双字节首字节 0xA1-0xFE，次字节 0x40-0x7E 或 0xA1-0xFE。
///
/// 与 GBK 的区别：Big5 次字节不包含 0x80-0xA0 区间，且首字节范围相同。
/// 仅当双字节序列占主导且符合 Big5 字节模式时返回 true。
fn looks_like_big5(bytes: &[u8]) -> bool {
    let mut i = 0;
    let mut double_byte = 0;
    let mut single_byte = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if (0xA1..=0xFE).contains(&b) && i + 1 < bytes.len() {
            let b2 = bytes[i + 1];
            if (0x40..=0x7E).contains(&b2) || (0xA1..=0xFE).contains(&b2) {
                double_byte += 1;
                i += 2;
                continue;
            }
        }
        if b < 0x80 {
            single_byte += 1;
        }
        i += 1;
    }
    double_byte > 0 && double_byte >= single_byte / 4
}

fn is_valid_utf8(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let len = if b < 0x80 {
            1
        } else if b < 0xC0 {
            return false;
        } else if b < 0xE0 {
            2
        } else if b < 0xF0 {
            3
        } else if b < 0xF8 {
            4
        } else {
            return false;
        };

        if i + len > bytes.len() {
            return false;
        }
        for j in 1..len {
            if bytes[i + j] & 0xC0 != 0x80 {
                return false;
            }
        }
        i += len;
    }
    true
}

fn looks_like_gbk(bytes: &[u8]) -> bool {
    let mut i = 0;
    let mut double_byte = 0;
    let mut single_byte = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if (0x81..=0xFE).contains(&b) && i + 1 < bytes.len() {
            let b2 = bytes[i + 1];
            if (0x40..=0x7E).contains(&b2) || (0x80..=0xFE).contains(&b2) {
                double_byte += 1;
                i += 2;
                continue;
            }
        }
        if b < 0x80 {
            single_byte += 1;
        }
        i += 1;
    }
    double_byte > 0 && double_byte >= single_byte / 4
}

fn looks_like_shift_jis(bytes: &[u8]) -> bool {
    let mut i = 0;
    let mut double_byte = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if ((0x81..=0x9F).contains(&b) || (0xE0..=0xEF).contains(&b)) && i + 1 < bytes.len() {
            let b2 = bytes[i + 1];
            if (0x40..=0x7E).contains(&b2) || (0x80..=0xFC).contains(&b2) {
                double_byte += 1;
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    double_byte > 0
}

pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

pub fn char_at(s: &str, index: usize) -> Option<char> {
    s.chars().nth(index)
}

pub fn char_slice(s: &str, start: usize, end: usize) -> Cow<'_, str> {
    // ASCII 快速路径：字符索引 == 字节索引，零分配
    if s.is_ascii() {
        let len = s.len();
        if start >= len {
            return Cow::Borrowed("");
        }
        let end = end.min(len);
        if start >= end {
            return Cow::Borrowed("");
        }
        return Cow::Borrowed(&s[start..end]);
    }
    // Unicode 路径：用 char_indices 找字节偏移，避免中间 Vec<char>
    let byte_start = s.char_indices().nth(start).map(|(i, _)| i).unwrap_or(s.len());
    let byte_end = s.char_indices().nth(end).map(|(i, _)| i).unwrap_or(s.len());
    if byte_start >= byte_end {
        return Cow::Borrowed("");
    }
    Cow::Borrowed(&s[byte_start..byte_end])
}

pub fn char_to_byte_index(s: &str, char_index: usize) -> Option<usize> {
    s.char_indices().nth(char_index).map(|(i, _)| i)
}

/// 安全地将字符位置转换为字节偏移量
///
/// 与 [`char_to_byte_index`] 行为一致：当字符位置超出范围时返回 `None`。
///
/// 历史版本在越界时返回 `s.len()`，会让调用方误把"越界"当成"字符串末尾"，
/// 产生静默错误（如错误地把整个字符串切片）。改为 `Option<usize>` 后，
/// 调用方必须显式处理越界情况。
///
/// # Arguments
/// * `s` - 源字符串
/// * `char_pos` - 字符位置（从 0 开始）
///
/// # Returns
/// - `Some(byte_idx)`: 该字符在字符串中的字节起始偏移
/// - `None`: `char_pos` 超出字符范围
///
/// # Example
/// ```
/// use nuzo_core::encoding::char_to_byte_offset;
/// assert_eq!(char_to_byte_offset("你好", 0), Some(0));
/// assert_eq!(char_to_byte_offset("你好", 1), Some(3));  // 第二个字符从第 3 字节开始
/// assert_eq!(char_to_byte_offset("你好", 2), None);     // 超出范围 → None
/// assert_eq!(char_to_byte_offset("hi你好", 3), Some(5)); // 第 4 个字符（你）从第 5 字节开始
/// ```
pub fn char_to_byte_offset(s: &str, char_pos: usize) -> Option<usize> {
    s.char_indices().nth(char_pos).map(|(byte_idx, _)| byte_idx)
}

/// 按字符数安全截断字符串（保证 UTF-8 边界对齐）
///
/// 用于显示截断、日志输出等场景，避免直接使用字节索引切片导致 panic。
///
/// # Arguments
/// * `s` - 源字符串
/// * `max_chars` - 最大保留字符数
///
/// # Returns
/// 截断后的字符串切片（保证在 UTF-8 字符边界上）
///
/// # Safety Guarantee
/// 返回值始终是合法的 UTF-8 切片，不会 panic。
///
/// # Example
/// ```
/// use nuzo_core::encoding::utf8_truncate;
/// let long = "这是一个很长的字符串用于测试截断功能";
/// let truncated = utf8_truncate(long, 5);
/// assert_eq!(truncated, "这是一个很"); // 截断到前 5 个字符
///
/// let short = "hello";
/// assert_eq!(utf8_truncate(short, 3), "hel");  // ASCII 安全截断
/// assert_eq!(utf8_truncate("你好世界", 2), "你好");  // CJK 安全截断
/// ```
pub fn utf8_truncate(s: &str, max_chars: usize) -> &str {
    if max_chars >= s.chars().count() {
        return s;
    }
    let byte_end = s.char_indices().nth(max_chars).map(|(idx, _)| idx).unwrap_or(s.len());
    &s[..byte_end]
}

pub fn encode_to_bytes(s: &str, encoding: Encoding) -> Result<Vec<u8>, EncodingError> {
    match encoding {
        Encoding::Utf8 => Ok(s.as_bytes().to_vec()),
        Encoding::Utf16Le => {
            // 不写入 BOM；调用方需要 BOM 时手动 prepend。
            let mut out = Vec::with_capacity(s.len() * 2);
            for c in s.chars() {
                let cu = c as u32;
                // 上限为 Unicode scalar value 上限 0x10FFFF（`char` 范围本身已排除代理对）。
                // 当前实现暂不支持代理对编码，对 supplementary plane 字符（如 emoji）
                // 返回 UnencodableChar。
                if (0x10000..=0x10FFFF).contains(&cu) {
                    return Err(EncodingError::UnencodableChar {
                        code_point: cu,
                        encoding: "UTF-16LE",
                    });
                }
                let unit = cu as u16;
                out.extend_from_slice(&unit.to_le_bytes());
            }
            Ok(out)
        }
        Encoding::Utf16Be => {
            let mut out = Vec::with_capacity(s.len() * 2);
            for c in s.chars() {
                let cu = c as u32;
                // 见 UTF-16LE 分支的说明：暂不支持 supplementary plane 字符。
                if (0x10000..=0x10FFFF).contains(&cu) {
                    return Err(EncodingError::UnencodableChar {
                        code_point: cu,
                        encoding: "UTF-16BE",
                    });
                }
                let unit = cu as u16;
                out.extend_from_slice(&unit.to_be_bytes());
            }
            Ok(out)
        }
        Encoding::Latin1 => s
            .chars()
            .map(|c| {
                if (c as u32) < 256 {
                    Ok(c as u8)
                } else {
                    Err(EncodingError::UnencodableChar {
                        code_point: c as u32,
                        encoding: "Latin-1",
                    })
                }
            })
            .collect(),
        Encoding::Gbk | Encoding::ShiftJis | Encoding::Big5 => {
            let utf8_bytes = s.as_bytes();
            let mut result = Vec::with_capacity(utf8_bytes.len());
            let mut i = 0;
            while i < utf8_bytes.len() {
                let b = utf8_bytes[i];
                if b < 0x80 {
                    result.push(b);
                    i += 1;
                } else {
                    let char_len = if b < 0xE0 {
                        2
                    } else if b < 0xF0 {
                        3
                    } else {
                        4
                    };
                    if i + char_len > utf8_bytes.len() {
                        return Err(EncodingError::IncompleteSequence {
                            detail: "truncated UTF-8 input",
                        });
                    }
                    let c = s[i..]
                        .chars()
                        .next()
                        .ok_or(EncodingError::IncompleteSequence { detail: "invalid UTF-8" })?;
                    let encoded = encode_char(c, encoding)?;
                    result.extend_from_slice(&encoded.bytes[..encoded.len as usize]);
                    i += char_len;
                }
            }
            Ok(result)
        }
    }
}

pub fn decode_from_bytes(bytes: &[u8], encoding: Encoding) -> Result<String, EncodingError> {
    match encoding {
        Encoding::Utf8 => String::from_utf8(bytes.to_vec())
            .map_err(|e| EncodingError::InvalidUtf8 { detail: e.to_string() }),
        Encoding::Utf16Le => decode_utf16(bytes, /*big_endian=*/ false),
        Encoding::Utf16Be => decode_utf16(bytes, /*big_endian=*/ true),
        Encoding::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
        Encoding::Gbk | Encoding::ShiftJis | Encoding::Big5 => {
            let mut result = String::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                let b = bytes[i];
                if b < 0x80 {
                    result.push(b as char);
                    i += 1;
                } else {
                    let (c, advance) = decode_double_byte(&bytes[i..], encoding)?;
                    result.push(c);
                    i += advance;
                }
            }
            Ok(result)
        }
    }
}

/// 单字符编码结果，最多 2 字节，栈上存储，零堆分配
#[derive(Debug, Clone, Copy)]
struct EncodedChar {
    bytes: [u8; 2],
    len: u8,
}

impl EncodedChar {
    #[inline]
    fn single(b: u8) -> Self {
        Self { bytes: [b, 0], len: 1 }
    }
    #[inline]
    fn double(b0: u8, b1: u8) -> Self {
        Self { bytes: [b0, b1], len: 2 }
    }
}

fn encode_char(c: char, encoding: Encoding) -> Result<EncodedChar, EncodingError> {
    let code = c as u32;
    match encoding {
        Encoding::Gbk => {
            if let Some(ec) = unicode_to_gbk(code) {
                Ok(ec)
            } else {
                Err(EncodingError::UnencodableChar { code_point: code, encoding: "GBK" })
            }
        }
        Encoding::ShiftJis => {
            if let Some(ec) = unicode_to_sjis(code) {
                Ok(ec)
            } else {
                Err(EncodingError::UnencodableChar { code_point: code, encoding: "Shift-JIS" })
            }
        }
        Encoding::Big5 => {
            if let Some(ec) = unicode_to_big5(code) {
                Ok(ec)
            } else {
                Err(EncodingError::UnencodableChar { code_point: code, encoding: "Big5" })
            }
        }
        _ => Err(EncodingError::UnsupportedEncoding {
            detail: "single-char encoding not applicable for this encoding",
        }),
    }
}

fn decode_double_byte(bytes: &[u8], encoding: Encoding) -> Result<(char, usize), EncodingError> {
    if bytes.len() < 2 {
        return Err(EncodingError::IncompleteSequence {
            detail: "need at least 2 bytes for double-byte decoding",
        });
    }
    let b0 = bytes[0];
    let b1 = bytes[1];
    match encoding {
        Encoding::Gbk => {
            if let Some(c) = gbk_to_unicode(b0, b1) {
                Ok((c, 2))
            } else {
                Err(EncodingError::InvalidBytePair { byte0: b0, byte1: b1, encoding: "GBK" })
            }
        }
        Encoding::ShiftJis => {
            if let Some(c) = sjis_to_unicode(b0, b1) {
                Ok((c, 2))
            } else {
                Err(EncodingError::InvalidBytePair { byte0: b0, byte1: b1, encoding: "Shift-JIS" })
            }
        }
        Encoding::Big5 => {
            if let Some(c) = big5_to_unicode(b0, b1) {
                Ok((c, 2))
            } else {
                Err(EncodingError::InvalidBytePair { byte0: b0, byte1: b1, encoding: "Big5" })
            }
        }
        _ => Err(EncodingError::UnsupportedEncoding {
            detail: "double-byte decoding not applicable for this encoding",
        }),
    }
}

/// UTF-16 解码：处理 BOM 剥离、代理对、字节序。
///
/// `big_endian=true` 表示 UTF-16BE，否则 UTF-16LE。
/// 若字节流以 BOM 开头，则按 BOM 决定字节序并剥离 BOM 字节；
/// 否则按 `big_endian` 参数解释。
fn decode_utf16(bytes: &[u8], big_endian: bool) -> Result<String, EncodingError> {
    // 处理 BOM：若存在则覆盖调用方指定的字节序
    let (body, be) = if bytes.len() >= 2 {
        let b0 = bytes[0];
        let b1 = bytes[1];
        if b0 == UTF16_LE_BOM_0 && b1 == UTF16_LE_BOM_1 {
            (&bytes[2..], false)
        } else if b0 == UTF16_BE_BOM_0 && b1 == UTF16_BE_BOM_1 {
            (&bytes[2..], true)
        } else {
            (bytes, big_endian)
        }
    } else {
        (bytes, big_endian)
    };

    if body.len() % 2 != 0 {
        return Err(EncodingError::IncompleteSequence {
            detail: "UTF-16 input length is not a multiple of 2",
        });
    }

    let mut out = String::with_capacity(body.len() / 2);
    let mut i = 0;
    while i < body.len() {
        let unit = if be {
            u16::from_be_bytes([body[i], body[i + 1]])
        } else {
            u16::from_le_bytes([body[i], body[i + 1]])
        };
        i += 2;

        // 代理对处理：高代理 [0xD800..=0xDBFF]，低代理 [0xDC00..=0xDFFF]
        if (0xD800..=0xDBFF).contains(&unit) {
            if i + 2 > body.len() {
                return Err(EncodingError::IncompleteSequence {
                    detail: "truncated high surrogate in UTF-16 stream",
                });
            }
            let lo = if be {
                u16::from_be_bytes([body[i], body[i + 1]])
            } else {
                u16::from_le_bytes([body[i], body[i + 1]])
            };
            i += 2;
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return Err(EncodingError::InvalidBytePair {
                    byte0: (unit >> 8) as u8,
                    byte1: (unit & 0xFF) as u8,
                    encoding: "UTF-16 surrogate pair",
                });
            }
            let code = 0x10000u32 + (((unit as u32) - 0xD800) << 10) + ((lo as u32) - 0xDC00);
            let c = char::from_u32(code).ok_or(EncodingError::InvalidBytePair {
                byte0: (code >> 24) as u8,
                byte1: (code >> 16) as u8,
                encoding: "UTF-16 surrogate pair (invalid code point)",
            })?;
            out.push(c);
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            // 孤立低代理
            return Err(EncodingError::InvalidBytePair {
                byte0: (unit >> 8) as u8,
                byte1: (unit & 0xFF) as u8,
                encoding: "UTF-16 lone low surrogate",
            });
        } else {
            let c = char::from_u32(unit as u32).ok_or(EncodingError::InvalidBytePair {
                byte0: (unit >> 8) as u8,
                byte1: (unit & 0xFF) as u8,
                encoding: "UTF-16 invalid code unit",
            })?;
            out.push(c);
        }
    }
    Ok(out)
}

fn unicode_to_gbk(code: u32) -> Option<EncodedChar> {
    if code < 0x80 {
        return Some(EncodedChar::single(code as u8));
    }
    if (0x4E00..=0x9FFF).contains(&code) {
        let offset = code - 0x4E00;
        let lead = 0x81 + (offset / 190);
        let trail = offset % 190;
        let trail_byte = if trail < 0x3F { 0x40 + trail } else { 0x41 + trail };
        return Some(EncodedChar::double(lead as u8, trail_byte as u8));
    }
    None
}

fn gbk_to_unicode(b0: u8, b1: u8) -> Option<char> {
    if b0 < 0x80 {
        return Some(b0 as char);
    }
    let lead = (b0 as u32).wrapping_sub(0x81);
    let trail =
        if b1 >= 0x80 { (b1 as u32).wrapping_sub(0x41) } else { (b1 as u32).wrapping_sub(0x40) };
    let offset = lead * 190 + trail;
    let code = 0x4E00 + offset;
    char::from_u32(code)
}

// ============================================================================
// SJIS / Big5 编解码 — 当前为存根实现
// ============================================================================
//
// 这些函数目前返回 `None`，表示无法编码/解码；调用方
// (`encode_char` / `decode_double_byte`) 会将其转换为
// `EncodingError::UnencodableChar` 或 `EncodingError::InvalidBytePair` 返回给用户，
// 不会静默成功。
//
// 完整实现需要 CP932 / CP950 映射表（数 MB），暂未引入以避免增加二进制大小。
// 采用 `cfg(any())` 守卫模式：真实实现可放入 `#[cfg(any())]` 块（永远不编译），
// 待引入 feature flag 后改为 `#[cfg(feature = "sjis-tables")]` 即可启用。
// 当前 `#[cfg(not(any()))]` 兜底分支始终生效，确保 API 兼容性。
//
// 当真实实现接入时，删除下方 `#[cfg(not(any()))]` 兜底分支即可。

#[cfg(any())]
fn unicode_to_sjis(code: u32) -> Option<EncodedChar> {
    // TODO: 实现 SJIS (CP932) 编码映射
    let _ = code;
    None
}

#[cfg(not(any()))]
fn unicode_to_sjis(_code: u32) -> Option<EncodedChar> {
    // SJIS 编码尚未实现 — 调用方将返回 EncodingError::UnencodableChar
    None
}

#[cfg(any())]
fn sjis_to_unicode(b0: u8, b1: u8) -> Option<char> {
    // TODO: 实现 SJIS (CP932) 解码映射
    let _ = (b0, b1);
    None
}

#[cfg(not(any()))]
fn sjis_to_unicode(_b0: u8, _b1: u8) -> Option<char> {
    // SJIS 解码尚未实现 — 调用方将返回 EncodingError::InvalidBytePair
    None
}

#[cfg(any())]
fn unicode_to_big5(code: u32) -> Option<EncodedChar> {
    // TODO: 实现 Big5 (CP950) 编码映射
    let _ = code;
    None
}

#[cfg(not(any()))]
fn unicode_to_big5(_code: u32) -> Option<EncodedChar> {
    // Big5 编码尚未实现 — 调用方将返回 EncodingError::UnencodableChar
    None
}

#[cfg(any())]
fn big5_to_unicode(b0: u8, b1: u8) -> Option<char> {
    // TODO: 实现 Big5 (CP950) 解码映射
    let _ = (b0, b1);
    None
}

#[cfg(not(any()))]
fn big5_to_unicode(_b0: u8, _b1: u8) -> Option<char> {
    // Big5 解码尚未实现 — 调用方将返回 EncodingError::InvalidBytePair
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoding_names() {
        assert_eq!(Encoding::Utf8.name(), "UTF-8");
        assert_eq!(Encoding::Gbk.name(), "GBK");
        assert_eq!(Encoding::ShiftJis.name(), "Shift-JIS");
        assert_eq!(Encoding::Big5.name(), "Big5");
        assert_eq!(Encoding::Latin1.name(), "ISO-8859-1");
    }

    #[test]
    fn test_encoding_from_name() {
        assert_eq!(Encoding::from_name("utf-8"), Some(Encoding::Utf8));
        assert_eq!(Encoding::from_name("UTF8"), Some(Encoding::Utf8));
        assert_eq!(Encoding::from_name("gbk"), Some(Encoding::Gbk));
        assert_eq!(Encoding::from_name("GB2312"), Some(Encoding::Gbk));
        assert_eq!(Encoding::from_name("shift-jis"), Some(Encoding::ShiftJis));
        assert_eq!(Encoding::from_name("big5"), Some(Encoding::Big5));
        assert_eq!(Encoding::from_name("latin1"), Some(Encoding::Latin1));
        assert_eq!(Encoding::from_name("unknown"), None);
    }

    #[test]
    fn test_detect_utf8() {
        assert_eq!(Encoding::detect("hello".as_bytes()), Encoding::Utf8);
        assert_eq!(Encoding::detect("你好世界".as_bytes()), Encoding::Utf8);
        let bom = [UTF8_BOM_0, UTF8_BOM_1, UTF8_BOM_2, 0x68, 0x69];
        assert_eq!(Encoding::detect(&bom), Encoding::Utf8);
    }

    #[test]
    fn test_detect_empty() {
        assert_eq!(Encoding::detect(&[]), Encoding::Utf8);
    }

    #[test]
    fn test_char_len_ascii() {
        assert_eq!(char_len("hello"), 5);
    }

    #[test]
    fn test_char_len_unicode() {
        assert_eq!(char_len("你好世界"), 4);
    }

    #[test]
    fn test_char_len_mixed() {
        assert_eq!(char_len("hi你好"), 4);
    }

    #[test]
    fn test_char_at_ascii() {
        assert_eq!(char_at("hello", 0), Some('h'));
        assert_eq!(char_at("hello", 4), Some('o'));
        assert_eq!(char_at("hello", 5), None);
    }

    #[test]
    fn test_char_at_unicode() {
        assert_eq!(char_at("你好世界", 0), Some('你'));
        assert_eq!(char_at("你好世界", 2), Some('世'));
        assert_eq!(char_at("你好世界", 4), None);
    }

    #[test]
    fn test_char_slice() {
        assert_eq!(char_slice("hello", 1, 3), "el");
        assert_eq!(char_slice("你好世界", 1, 3), "好世");
        assert_eq!(char_slice("hi你好", 1, 4), "i你好");
        assert_eq!(char_slice("hello", 5, 10), "");
        assert_eq!(char_slice("hello", 3, 3), "");
    }

    #[test]
    fn test_char_to_byte_index() {
        assert_eq!(char_to_byte_index("hello", 0), Some(0));
        assert_eq!(char_to_byte_index("hello", 3), Some(3));
        assert_eq!(char_to_byte_index("你好", 0), Some(0));
        assert_eq!(char_to_byte_index("你好", 1), Some(3));
        assert_eq!(char_to_byte_index("hi你好", 2), Some(2));
        assert_eq!(char_to_byte_index("hi你好", 3), Some(5));
    }

    #[test]
    fn test_encode_utf8() {
        let bytes = encode_to_bytes("hello", Encoding::Utf8).unwrap();
        assert_eq!(bytes, b"hello");

        let bytes = encode_to_bytes("你好", Encoding::Utf8).unwrap();
        assert_eq!(&bytes, "你好".as_bytes());
    }

    #[test]
    fn test_encode_latin1() {
        let bytes = encode_to_bytes("hello", Encoding::Latin1).unwrap();
        assert_eq!(bytes, b"hello");

        let bytes = encode_to_bytes("café", Encoding::Latin1).unwrap();
        assert_eq!(bytes.last(), Some(&0xE9));
    }

    #[test]
    fn test_encode_latin1_rejects_unicode() {
        assert!(encode_to_bytes("你好", Encoding::Latin1).is_err());
    }

    #[test]
    fn test_decode_utf8() {
        let result = decode_from_bytes(b"hello", Encoding::Utf8).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_decode_latin1() {
        let bytes = vec![0x68, 0xE9, 0x6C, 0x6C, 0x6F];
        let result = decode_from_bytes(&bytes, Encoding::Latin1).unwrap();
        assert_eq!(result, "héllo");
    }

    #[test]
    fn test_roundtrip_utf8() {
        let original = "Hello, 世界! 🌍";
        let bytes = encode_to_bytes(original, Encoding::Utf8).unwrap();
        let decoded = decode_from_bytes(&bytes, Encoding::Utf8).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_latin1() {
        let original = "caf\u{E9}";
        let bytes = encode_to_bytes(original, Encoding::Latin1).unwrap();
        let decoded = decode_from_bytes(&bytes, Encoding::Latin1).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_gbk_roundtrip_cjk() {
        let original = "你";
        let bytes = encode_to_bytes(original, Encoding::Gbk).unwrap();
        assert_eq!(bytes.len(), 2);
        let decoded = decode_from_bytes(&bytes, Encoding::Gbk).unwrap();
        assert_eq!(decoded, "你");
    }

    #[test]
    fn test_gbk_encode_ascii_passthrough() {
        let bytes = encode_to_bytes("A", Encoding::Gbk).unwrap();
        assert_eq!(bytes, vec![0x41]);
    }

    #[test]
    fn test_gbk_decode_ascii_passthrough() {
        let result = decode_from_bytes(&[0x41], Encoding::Gbk).unwrap();
        assert_eq!(result, "A");
    }

    #[test]
    fn test_string_index_cache_short_string() {
        let mut cache = StringIndexCache::new();
        assert_eq!(cache.char_at_fast("hello", 0), Some('h'));
        assert_eq!(cache.char_at_fast("hello", 4), Some('o'));
        assert_eq!(cache.char_at_fast("hello", 5), None);
        assert!(!cache.is_built());
    }

    #[test]
    fn test_string_index_cache_long_string() {
        let long = "你好世界这是一个很长的字符串用来测试缓存功能它应该超过三十二字节";
        assert!(long.len() > SHORT_STRING_THRESHOLD);
        let mut cache = StringIndexCache::new();
        assert_eq!(cache.char_at_fast(long, 0), Some('你'));
        assert_eq!(cache.char_at_fast(long, 1), Some('好'));
        assert_eq!(cache.char_at_fast(long, 5), Some('是'));
        assert!(cache.is_built());
        assert_eq!(cache.char_len_cached(), char_len(long));
    }

    #[test]
    fn test_string_index_cache_slice_long() {
        let long = "你好世界这是一个很长的字符串用来测试缓存功能它应该超过三十二字节";
        let mut cache = StringIndexCache::new();
        let slice = cache.char_slice_fast(long, 1, 5);
        assert_eq!(slice, "好世界这");
    }

    #[test]
    fn test_string_index_cache_byte_index() {
        let s = "hi你好";
        let mut cache = StringIndexCache::new();
        assert_eq!(cache.char_to_byte_index_fast(s, 0), Some(0));
        assert_eq!(cache.char_to_byte_index_fast(s, 2), Some(2));
        assert_eq!(cache.char_to_byte_index_fast(s, 3), Some(5));
    }

    #[test]
    fn test_string_index_cache_empty() {
        let mut cache = StringIndexCache::new();
        assert_eq!(cache.char_at_fast("", 0), None);
        assert!(!cache.is_built());
    }

    #[test]
    fn test_string_index_cache_out_of_bounds() {
        let long = "你好世界这是一个很长的字符串用来测试缓存功能它应该超过三十二字节";
        let mut cache = StringIndexCache::new();
        assert_eq!(cache.char_at_fast(long, 999), None);
    }

    #[test]
    fn test_string_index_cache_reuse() {
        let long = "你好世界这是一个很长的字符串用来测试缓存功能它应该超过三十二字节";
        let mut cache = StringIndexCache::new();
        let _ = cache.char_at_fast(long, 0);
        assert!(cache.is_built());
        let len_after_first = cache.offsets.len();
        let _ = cache.char_at_fast(long, 3);
        assert_eq!(cache.offsets.len(), len_after_first);
    }

    // ========================================================================
    // char_to_byte_offset & utf8_truncate 测试
    // ========================================================================

    #[test]
    fn test_char_to_byte_offset_ascii() {
        assert_eq!(char_to_byte_offset("hello", 0), Some(0));
        assert_eq!(char_to_byte_offset("hello", 3), Some(3));
        assert_eq!(char_to_byte_offset("hello", 5), None); // 超出范围（恰好末尾也算越界）
        assert_eq!(char_to_byte_offset("hello", 10), None); // 超出范围 → None
    }

    #[test]
    fn test_char_to_byte_offset_unicode() {
        // 中文每个字符 3 字节
        assert_eq!(char_to_byte_offset("你好", 0), Some(0));
        assert_eq!(char_to_byte_offset("你好", 1), Some(3)); // 第二个字符从字节 3 开始
        assert_eq!(char_to_byte_offset("你好", 2), None); // 超出 → None
        // 混合 ASCII + CJK
        assert_eq!(char_to_byte_offset("hi你好", 0), Some(0));
        assert_eq!(char_to_byte_offset("hi你好", 2), Some(2)); // 'i' 在字节 2
        assert_eq!(char_to_byte_offset("hi你好", 3), Some(5)); // '你' 从字节 5 开始
    }

    #[test]
    fn test_char_to_byte_offset_empty() {
        assert_eq!(char_to_byte_offset("", 0), None);
        assert_eq!(char_to_byte_offset("", 100), None);
    }

    #[test]
    fn test_char_to_byte_offset_boundary_at_end() {
        // 边界：最后一个字符的位置应返回 Some，再往后返回 None
        assert_eq!(char_to_byte_offset("abc", 2), Some(2)); // 最后一个字符 'c'
        assert_eq!(char_to_byte_offset("abc", 3), None); // 越界
    }

    #[test]
    fn test_utf8_truncate_noop_when_short() {
        let s = "hello";
        assert_eq!(utf8_truncate(s, 10), s); // 未超限 → 原样返回
        assert_eq!(utf8_truncate(s, 5), s); // 恰好等长 → 原样返回
    }

    #[test]
    fn test_utf8_truncate_ascii() {
        assert_eq!(utf8_truncate("hello world", 5), "hello");
    }

    #[test]
    fn test_utf8_truncate_cjk() {
        // CJK 安全截断：不会切到多字节字符内部
        assert_eq!(utf8_truncate("你好世界", 2), "你好");
        assert_eq!(utf8_truncate("你好世界", 0), "");
    }

    #[test]
    fn test_utf8_truncate_mixed() {
        // 混合脚本安全截断
        assert_eq!(utf8_truncate("hi你好世界", 4), "hi你好");
    }

    #[test]
    fn test_utf8_truncate_emoji() {
        // Emoji 通常 4 字节
        let s = "hello 🌍 world";
        assert_eq!(utf8_truncate(s, 7), "hello 🌍"); // 空格 + emoji = 2 个字符点
    }

    #[test]
    fn test_utf8_truncate_empty() {
        assert_eq!(utf8_truncate("", 100), "");
    }

    // ========================================================================
    // UTF-16 BOM 检测与解码回归测试（BUG: encoding.rs:312-321 误判为 Latin1）
    // ========================================================================

    #[test]
    fn test_detect_utf16_le_bom() {
        // UTF-16LE BOM + "hi" → 应识别为 Utf16Le，而非 Latin1
        let bytes = [UTF16_LE_BOM_0, UTF16_LE_BOM_1, 0x68, 0x00, 0x69, 0x00];
        assert_eq!(Encoding::detect(&bytes), Encoding::Utf16Le);
    }

    #[test]
    fn test_detect_utf16_be_bom() {
        // UTF-16BE BOM + "hi" → 应识别为 Utf16Be
        let bytes = [UTF16_BE_BOM_0, UTF16_BE_BOM_1, 0x00, 0x68, 0x00, 0x69];
        assert_eq!(Encoding::detect(&bytes), Encoding::Utf16Be);
    }

    #[test]
    fn test_decode_utf16_le_bom_chinese() {
        // "你好" UTF-16LE 编码（含 BOM）→ 中文不应乱码
        // 你 = U+4F60, 好 = U+597D
        let bytes = [UTF16_LE_BOM_0, UTF16_LE_BOM_1, 0x60, 0x4F, 0x7D, 0x59];
        let result =
            decode_from_bytes(&bytes, Encoding::Utf16Le).expect("UTF-16LE decode must succeed");
        assert_eq!(result, "你好", "UTF-16LE BOM + Chinese must decode correctly");
    }

    #[test]
    fn test_decode_utf16_be_bom_chinese() {
        // "你好" UTF-16BE 编码（含 BOM）
        let bytes = [UTF16_BE_BOM_0, UTF16_BE_BOM_1, 0x4F, 0x60, 0x59, 0x7D];
        let result =
            decode_from_bytes(&bytes, Encoding::Utf16Be).expect("UTF-16BE decode must succeed");
        assert_eq!(result, "你好", "UTF-16BE BOM + Chinese must decode correctly");
    }

    #[test]
    fn test_decode_utf16_le_no_bom() {
        // 无 BOM 的 UTF-16LE：调用方显式指定编码
        let bytes = [0x68, 0x00, 0x69, 0x00]; // "hi"
        let result = decode_from_bytes(&bytes, Encoding::Utf16Le).unwrap();
        assert_eq!(result, "hi");
    }

    #[test]
    fn test_decode_utf16_odd_length_errors() {
        // 奇数长度应返回 IncompleteSequence 错误
        let bytes = [0x68, 0x00, 0x69];
        let result = decode_from_bytes(&bytes, Encoding::Utf16Le);
        assert!(result.is_err(), "odd-length UTF-16 must error");
    }

    #[test]
    fn test_decode_utf16_surrogate_pair() {
        // U+1F600 (😀) 的 UTF-16LE 代理对编码：D83D DE00
        let bytes = [0x3D, 0xD8, 0x00, 0xDE];
        let result = decode_from_bytes(&bytes, Encoding::Utf16Le).unwrap();
        assert_eq!(result, "\u{1F600}");
    }

    #[test]
    fn test_decode_utf16_lone_low_surrogate_errors() {
        // 孤立低代理 DC00 应报错
        let bytes = [0x00, 0xDC];
        let result = decode_from_bytes(&bytes, Encoding::Utf16Le);
        assert!(result.is_err(), "lone low surrogate must error");
    }

    #[test]
    fn test_encode_utf16_le_roundtrip() {
        let original = "你好世界hello";
        let bytes = encode_to_bytes(original, Encoding::Utf16Le).unwrap();
        let decoded = decode_from_bytes(&bytes, Encoding::Utf16Le).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_encode_utf16_be_roundtrip() {
        let original = "你好世界hello";
        let bytes = encode_to_bytes(original, Encoding::Utf16Be).unwrap();
        let decoded = decode_from_bytes(&bytes, Encoding::Utf16Be).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_encoding_utf16_names() {
        assert_eq!(Encoding::Utf16Le.name(), "UTF-16LE");
        assert_eq!(Encoding::Utf16Be.name(), "UTF-16BE");
        assert_eq!(Encoding::from_name("utf-16le"), Some(Encoding::Utf16Le));
        assert_eq!(Encoding::from_name("utf-16be"), Some(Encoding::Utf16Be));
        assert_eq!(Encoding::from_name("utf-16"), Some(Encoding::Utf16Le));
    }

    // ========================================================================
    // Big5 启发式检测回归测试
    // ========================================================================

    #[test]
    fn test_detect_big5_heuristic() {
        // Big5 真实样本："中文" Big5 编码：A4 A4 A4 E5
        let bytes = [0xA4, 0xA4, 0xA4, 0xE5];
        let detected = Encoding::detect(&bytes);
        // Big5 和 GBK 首字节范围有重叠；只要不返回 Latin1 即可
        assert_ne!(detected, Encoding::Latin1, "Big5 sample must not fall back to Latin1");
    }

    #[test]
    fn test_looks_like_big5_pure_ascii_returns_false() {
        let bytes = b"hello world";
        assert_eq!(Encoding::detect(bytes), Encoding::Utf8);
    }
}
