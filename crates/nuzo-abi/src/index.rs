//! 安全索引封装，防止 u8/u16/u32 索引溢出截断。
//!
//! 在 Nuzo 字节码中，许多操作数使用 u8 或 u16 存储索引值，
//! 但运行时实际索引可能超过其范围。`SafeIndex` 提供编译期
//! 可检查的安全转换，避免静默截断导致的 bug（如 BUG-005）。
//!
//! # 类型一览
//!
//! | 类型 | 典型用途 | 范围 |
//! |------|---------|------|
//! | `SafeIndex<u8>` | 参数计数 argc、捕获索引 | 0..=255 |
//! | `SafeIndex<u16>` | 常量池索引 ConstIdx、寄存器 Reg | 0..=65535 |
//! | `SafeIndex<u32>` | 运行时通用索引 | 0..=4294967295 |
//!
//! # 转换关系
//!
//! ```text
//! u8 ──From──→ SafeIndex<u8> ──From──→ u32  (安全向上扩展)
//! u16 ──From──→ SafeIndex<u16> ──From──→ u32
//! u32 ──From──→ SafeIndex<u32> ──From──→ u32
//!
//! usize ──try_from_usize──→ SafeIndex<T>  (溢出检查)
//! u32   ──try_from_u32──→   SafeIndex<T>  (溢出检查)
//! ```

use std::fmt;

/// 安全索引封装，防止 u8/u16/u32 索引溢出截断。
///
/// # 用法
///
/// ```ignore
/// use nuzo_abi::index::SafeIndex;
///
/// // 安全构造（编译期可检查）
/// let idx: SafeIndex<u8> = SafeIndex::from(200u8);
///
/// // 从较大类型安全转换（运行时检查）
/// let idx = SafeIndex::<u8>::try_from_u32(200)?;  // OK: 200 <= 255
/// let idx = SafeIndex::<u8>::try_from_u32(300);   // Err: 300 > 255
///
/// // 安全向上转换（零开销）
/// let wide: u32 = idx.into();  // SafeIndex<u8> → u32
///
/// // 获取内部值
/// let raw: u8 = idx.get();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SafeIndex<T>(T);

// ============================================================================
// SafeIndex<u8>
// ============================================================================

impl SafeIndex<u8> {
    /// 从 u32 值安全转换为 `SafeIndex<u8>`。
    ///
    /// 若 `val > u8::MAX` 返回 `IndexOverflowError`。
    pub fn try_from_u32(val: u32) -> Result<Self, IndexOverflowError> {
        if val > u8::MAX as u32 {
            Err(IndexOverflowError { target: "u8", value: val })
        } else {
            Ok(SafeIndex(val as u8))
        }
    }

    /// 从 usize 值安全转换为 `SafeIndex<u8>`。
    pub fn try_from_usize(val: usize) -> Result<Self, IndexOverflowError> {
        Self::try_from_u32(val as u32)
    }

    /// 获取内部 u8 值。
    pub fn get(self) -> u8 {
        self.0
    }
}

impl From<u8> for SafeIndex<u8> {
    fn from(val: u8) -> Self {
        SafeIndex(val)
    }
}

impl From<SafeIndex<u8>> for u8 {
    fn from(idx: SafeIndex<u8>) -> Self {
        idx.0
    }
}

/// SafeIndex<u8> → u32 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u8>> for u32 {
    fn from(idx: SafeIndex<u8>) -> Self {
        idx.0 as u32
    }
}

/// SafeIndex<u8> → u16 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u8>> for u16 {
    fn from(idx: SafeIndex<u8>) -> Self {
        idx.0 as u16
    }
}

/// SafeIndex<u8> → usize 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u8>> for usize {
    fn from(idx: SafeIndex<u8>) -> Self {
        idx.0 as usize
    }
}

// ============================================================================
// SafeIndex<u16>
// ============================================================================

impl SafeIndex<u16> {
    /// 从 u32 值安全转换为 `SafeIndex<u16>`。
    ///
    /// 若 `val > u16::MAX` 返回 `IndexOverflowError`。
    pub fn try_from_u32(val: u32) -> Result<Self, IndexOverflowError> {
        if val > u16::MAX as u32 {
            Err(IndexOverflowError { target: "u16", value: val })
        } else {
            Ok(SafeIndex(val as u16))
        }
    }

    /// 从 usize 值安全转换为 `SafeIndex<u16>`。
    pub fn try_from_usize(val: usize) -> Result<Self, IndexOverflowError> {
        Self::try_from_u32(val as u32)
    }

    /// 获取内部 u16 值。
    pub fn get(self) -> u16 {
        self.0
    }
}

impl From<u16> for SafeIndex<u16> {
    fn from(val: u16) -> Self {
        SafeIndex(val)
    }
}

impl From<SafeIndex<u16>> for u16 {
    fn from(idx: SafeIndex<u16>) -> Self {
        idx.0
    }
}

/// SafeIndex<u16> → u32 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u16>> for u32 {
    fn from(idx: SafeIndex<u16>) -> Self {
        idx.0 as u32
    }
}

/// SafeIndex<u16> → usize 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u16>> for usize {
    fn from(idx: SafeIndex<u16>) -> Self {
        idx.0 as usize
    }
}

// ============================================================================
// SafeIndex<u32>
// ============================================================================

impl SafeIndex<u32> {
    /// 从 u32 值构造 `SafeIndex<u32>`（永成功）。
    pub fn try_from_u32(val: u32) -> Result<Self, IndexOverflowError> {
        Ok(SafeIndex(val))
    }

    /// 从 usize 值安全转换为 `SafeIndex<u32>`。
    ///
    /// 在 64 位平台上 usize > u32::MAX 时返回错误。
    pub fn try_from_usize(val: usize) -> Result<Self, IndexOverflowError> {
        if val > u32::MAX as usize {
            Err(IndexOverflowError { target: "u32", value: val as u32 })
        } else {
            Ok(SafeIndex(val as u32))
        }
    }

    /// 获取内部 u32 值。
    pub fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for SafeIndex<u32> {
    fn from(val: u32) -> Self {
        SafeIndex(val)
    }
}

impl From<SafeIndex<u32>> for u32 {
    fn from(idx: SafeIndex<u32>) -> Self {
        idx.0
    }
}

/// SafeIndex<u32> → usize 安全向上转换（零开销，值域包含）。
impl From<SafeIndex<u32>> for usize {
    fn from(idx: SafeIndex<u32>) -> Self {
        idx.0 as usize
    }
}

// ============================================================================
// IndexOverflowError
// ============================================================================

/// 索引溢出错误。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexOverflowError {
    /// 目标类型名称（如 "u8"、"u16"）。
    pub target: &'static str,
    /// 导致溢出的值。
    pub value: u32,
}

impl fmt::Display for IndexOverflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "index overflow: {} exceeds {} range", self.value, self.target)
    }
}

impl std::error::Error for IndexOverflowError {}

// ============================================================================
// 便捷类型别名
// ============================================================================

/// 8 位安全索引（0..=255），用于参数计数 argc、捕获索引等。
pub type SafeU8 = SafeIndex<u8>;

/// 16 位安全索引（0..=65535），用于常量池索引 ConstIdx、寄存器 Reg 等。
pub type SafeU16 = SafeIndex<u16>;

/// 32 位安全索引，用于运行时通用索引。
pub type SafeU32 = SafeIndex<u32>;
