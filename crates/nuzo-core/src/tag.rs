//! # NaN 标记位布局常量与纯位操作函数
//!
//! 本模块定义了 [`Value`] 结构体的 **位模式编码方案**中所有位布局常量，
//! 以及仅依赖 `u64` 的纯位操作函数（零开销，编译为 1-3 条机器指令）。
//!
//! 从 `nuzo_values::constants` 下沉到 `nuzo_core`（L1 层），
//! 使 `nuzo_vm`（L5）可以直接导入而无需依赖 `nuzo_values`（L2）。
//!
//! ## 设计原理
//!
//! IEEE 754 双精度浮点数的位布局为：`[符号(1) | 指数(11) | 尾数(52)]`
//! - 当指数部分全为 1 (0x7FF) 且尾数非零时，表示 NaN
//! - 我们利用尾数的高 15 位作为**类型标签 (Type Tag)**，剩余位存储数据
//!
//! ## 位空间分配图
//!
//! ```text
//! 64 位 Value 的完整位布局：
//! ┌──────┬─────────┬─────────────────────────────────────────────┐
//! │ 位域  │ 位范围    │ 用途                                      │
//! ├──────┼─────────┼─────────────────────────────────────────────┤
//! │ Tag  │ [63:49]  │ 类型标记（15 位）                           │
//! │ GC   │ [45]     │ GC 管理标志位（0=HEAP_POOL, 1=GC 堆）       │
//! │ Index│ [44:0]   │ 堆索引 / Smi 值 / 字符串池索引 / 指针地址    │
//! └──────┴─────────┴─────────────────────────────────────────────┘
//!
//! 类型标签 (Tag[63:49]) 分配：
//! ┌────────────────┬───────────┬────────────────────────────────┐
//! │ Tag 值         │ 类型      │ Payload 含义                    │
//! ├────────────────┼───────────┼────────────────────────────────┤
//! │ 0x7FF8_0       │ 特殊值     │ 低 2 位: 1=nil, 2=false, 3=true│
//! │ 0x7FF8_4       │ 堆对象     │ 低 46 位: HEAP_POOL/GC 堆索引   │
//! │ 0x7FF8_8       │ 字符串     │ 低 47 位: 全局字符串池索引      │
//! │ 0x7FF9_        │ Smi 整数   │ 低 48 位: 有符号小整数          │
//! │ 其他           │ Float     │ 完整 IEEE 754 f64 位模式        │
//! └────────────────┴───────────┴────────────────────────────────┘
//! ```
//!
//! ## 使用示例
//!
//! ```ignore
//! use nuzo_core::tag;
//!
//! // 检测值类型（单条位与指令）
//! if tag::is_smi(bits) { /* 是 Smi 整数 */ }
//! if tag::is_heap_object(bits) { /* 是堆对象 */ }
//!
//! // 提取堆索引
//! let heap_idx = bits & tag::HEAP_INDEX_MASK;
//! ```

// ============================================================================
// 堆对象标记常量 (Heap Object Tagging)
// ============================================================================

/// 堆对象标签值。
///
/// 位模式：`0x7FF8_4000_0000_0000`（在 PTR_TAG 空间中设置第 50 位）
///
/// 用于标识存储在全局堆中的对象：数组、字典、范围、闭包、内建函数等。
/// 检测方式：`(bits & HEAP_MASK) == HEAP_TAG`
pub const HEAP_TAG: u64 = 0x7FF8_4000_0000_0000;

/// 堆对象检测掩码。
///
/// 接受 `HEAP_TAG` 和第 51 位也置位的模式（用于区分字符串标签）。
/// 掩码值：`0x7FF8_C000_0000_0000`
pub const HEAP_MASK: u64 = 0x7FF8_C000_0000_0000;

/// 堆索引提取掩码（46 位）。
///
/// 从 Value 中提取堆对象在全局池中的索引位置。
/// 掩码值：`0x0000_3FFF_FFFF_FFFF`（第 0-45 位）
pub const HEAP_INDEX_MASK: u64 = 0x0000_3FFF_FFFF_FFFF;

// ============================================================================
// 特殊值检测掩码 (Special Value Detection)
// ============================================================================

/// 全局特殊/标记值检测掩码。
///
/// 用于快速判断一个 Value 是否属于 NaN 标记空间（非标准浮点数）。
/// 所有特殊值（nil、bool、堆对象、字符串、Smi）的高 15 位都匹配此掩码。
/// 掩码值：`0x7FF8_0000_0000_0000`
pub const SPECIAL_MASK: u64 = 0x7FF8_0000_0000_0000;

// ============================================================================
// 字符串标记常量 (String Tagging)
// ============================================================================

/// 字符串值标签。
///
/// 位模式：`0x7FF8_8000_XXXX_XXXX`（在 PTR_TAG 空间中设置第 51 位以区分原始指针）
pub const STRING_TAG: u64 = 0x7FF8_8000_0000_0000;

/// 字符串值检测掩码。
///
/// 掩码值 = PTR_TAG | 第 51 位 = `0x7FF8_8000_0000_0000`
pub const STRING_MASK: u64 = 0x7FF8_8000_0000_0000;

/// 字符串索引提取掩码（47 位）。
///
/// 从 Value 中提取字符串在全局池中的索引位置。
/// 掩码值：`0x0000_7FFF_FFFF_FFFF`（第 0-46 位）
pub const STRING_INDEX_MASK: u64 = 0x0000_7FFF_FFFF_FFFF;

// ============================================================================
// Smi 小整数常量
// ============================================================================

/// Smi (Small Integer) 标签值。
///
/// 位模式：`0x7FF9_0000_0000_0000`（使用 NaN 空间的 0x7FF9... 前缀）
///
/// Smi 编码将小整数直接嵌入 Value 的 64 位中，避免堆分配：
/// - 编码公式：`SMI(i) = SMI_TAG | (i as u64 & SMI_VALUE_MASK)`
/// - 解码时使用 48 位有符号扩展（第 47 位为符号位）
/// - 支持范围：[-140,737,488,355,328, +140,737,488,355,327]
pub const SMI_TAG: u64 = 0x7FF9_0000_0000_0000;

/// Smi 值检测掩码。
///
/// 用于快速判断 Value 是否为 Smi 编码的整数。
/// 掩码值：`0x7FFF_0000_0000_0000`（高 16 位必须为 0x7FFF）
pub const SMI_MASK: u64 = 0x7FFF_0000_0000_0000;

/// Smi 有效载荷提取掩码（48 位）。
///
/// 从 Smi Value 中提取实际的整数值位。
/// 掩码值：`0x0000_FFFF_FFFF_FFFF`（第 0-47 位）
pub const SMI_VALUE_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Smi 最大值：(2^47 - 1) ≈ +140 万亿
pub const SMI_MAX: i64 = (1i64 << 47) - 1;

/// Smi 最小值：-(2^47) ≈ -140 万亿
pub const SMI_MIN: i64 = -(1i64 << 47);

/// Smi 符号位（第 47 位）。
///
/// 用于 Smi 解码时检测负数：`if raw & SMI_SIGN_BIT != 0 { /* 负数 */ }`
pub const SMI_SIGN_BIT: u64 = 1u64 << 47;

/// Smi 符号扩展值（2^48）。
///
/// 用于将 Smi 的 48 位无符号表示转换为有符号 i64：
/// `if raw & SMI_SIGN_BIT != 0 { (raw as i64) - SMI_SIGN_EXTEND } else { raw as i64 }`
pub const SMI_SIGN_EXTEND: i64 = 1i64 << 48;

// ============================================================================
// 指针编码常量 (Pointer Encoding)
// ============================================================================

/// 指针标签基础值。
///
/// 所有指针类型值的公共前缀：`0x7FF8_0000_0000_0000`
/// 实际指针地址通过 `PTR_TAG | addr` 编码到 Value 中。
pub const PTR_TAG: u64 = 0x7FF8_0000_0000_0000;

// ============================================================================
// 特殊单例常量值 (Special Singleton Values)
// ============================================================================

/// nil / null 空值的位模式。
///
/// 编码：`PTR_TAG | 1` = `0x7FF8_0000_0000_0001`
pub const NIL_VALUE: u64 = PTR_TAG | 1;

/// 布尔值 false 的位模式。
///
/// 编码：`PTR_TAG | 2` = `0x7FF8_0000_0000_0002`
pub const FALSE_VALUE: u64 = PTR_TAG | 2;

/// 布尔值 true 的位模式。
///
/// 编码：`PTR_TAG | 3` = `0x7FF8_0000_0000_0003`
pub const TRUE_VALUE: u64 = PTR_TAG | 3;

// ============================================================================
// 指针编码常量 (Pointer Encoding) 续
// ============================================================================

/// 静默 NaN 模式掩码（用于指针检测）。
///
/// IEEE 754 静默 NaN 的特征：指数全 1 (0x7FF) + 尾数最高位为 1。
/// 掩码值：`0x7FFC_0000_0000_0000`
pub const QNAN_MASK: u64 = 0x7FFC_0000_0000_0000;

/// 指针地址掩码（低 46 位）。
///
/// bit 46-48 在 PTR_TAG 空间内被用作子标签（heap/string/smi），
/// 所以指针地址只能使用低 46 位。x86-64 用户空间地址远小于 64TB，足够用。
/// 掩码值：`0x0000_3FFF_FFFF_FFFF`
pub const PTR_MASK: u64 = 0x0000_3FFF_FFFF_FFFF;

// ============================================================================
// 规范化 NaN (Canonical NaN)
// ============================================================================

/// 规范化 NaN 位模式。
///
/// 值：`0x7FFC_0000_0000_0001`（第 50 位=1, 第 51 位=1）
///
/// 当用户代码产生 IEEE 754 NaN 值时，统一转换为此规范模式，
/// 避免与 SPECIAL_MASK 空间（第 51 位=1, 第 50 位=0）发生冲突。
pub const CANONICAL_NAN: u64 = 0x7FFC_0000_0000_0001;

// ============================================================================
// GC 集成常量 (GC Integration)
// ============================================================================

/// GC 管理标志位（第 45 位，位于 HEAP_INDEX_MASK 范围内）。
///
/// 此位用于区分堆对象的存储后端：
/// - **置位 (1)**：对象由 GC 垃圾回收器管理，索引指向 GC 堆
/// - **清除 (0)**：对象由默认 HEAP_POOL 管理，使用 Arc 引用计数
pub const GC_MANAGED_BIT: u64 = 1u64 << 45;

/// 排除 GC_MANAGED_BIT 后的堆索引掩码（45 位）。
///
/// 当从 Value 提取堆索引时，使用此掩码确保不包含 GC 标志位。
/// 有效索引范围：[0, 2^45-1] = [0, 35,184,372,088,831]
/// 掩码值：`0x0000_1FFF_FFFF_FFFF`
pub const HEAP_INDEX_MASK_NO_GC: u64 = 0x0000_1FFF_FFFF_FFFF;

/// 划痕区（Scratch Arena）索引基址（u32 最高位 = 划痕标识）。
///
/// 用于 ERSA (Epoch-Based Scratch Allocation) 机制中区分划痕区索引和持久区索引：
/// - **划痕区索引**：`[SCRATCH_BASE, SCRATCH_BASE + SCRATCH_CAP)` = `[0x8000_0000, 0x8000_1000)`
/// - **持久区索引**：`[0, SCRATCH_BASE)` = `[0, 0x7FFF_FFFF)`
///
/// # 正确性保证
///
/// 判断是否为划痕索引时，**必须使用 `idx >= SCRATCH_BASE` 进行无符号比较**，
/// 禁止使用 `(idx as i32) < 0` 符号位 hack。
pub const SCRATCH_BASE: u32 = 0x8000_0000;

/// HEAP_POOL 索引空间设计上限。
///
/// HEAP_POOL 使用连续 Vec 索引分配堆对象，索引 `[0, HEAP_POOL_INDEX_LIMIT)`
/// 属于 HEAP_POOL 空间；超过此上限后与 GC chunk 索引空间碰撞。
///
/// # Safety invariant
///
/// `mark_index` in `nuzo_vm::gc::mark` uses `idx < HEAP_POOL_INDEX_LIMIT` to
/// distinguish HEAP_POOL indices from GC chunk indices. When increasing this
/// value, the GC initialization must be updated to reserve enough placeholder
/// chunks so that `active_chunk` starts at `HEAP_POOL_INDEX_LIMIT >> GC_CHUNK_SHIFT`.
///
/// Current value: 4096 = 4 chunks (GC starts from chunk 4, idx >= 4096).
pub const HEAP_POOL_INDEX_LIMIT: usize = 4096;

// ============================================================================
// Arena (Region Allocator) 索引常量
// ============================================================================

/// Arena 索引区起始值（位于 GC Persistent 和 Scratch 之间）。
///
/// 编码空间: `[ARENA_BASE, SCRATCH_BASE)` = `[0x4000_0000, 0x8000_0000)` = 1GB
pub const ARENA_BASE: u32 = 0x4000_0000;

/// Arena 索引掩码（用于从完整索引中提取偏移量）。
///
/// 掩码值：`0x3FFF_FFFF`（低 30 位）
pub const ARENA_MASK: u32 = 0x3FFF_FFFF;

// ============================================================================
// 哈希常量 (Hash Constants)
// ============================================================================

/// 黄金比例乘法哈希常数（Knuth's multiplicative hash）。
///
/// 用于乘法哈希混合，对连续整数键具有优秀的雪崩效应。
pub const GOLDEN_64: u64 = 0x9E37_79B9_7F4A_7C15;

/// FxHash 乘法常数。
///
/// 用于 FxHash 变体哈希函数的乘法步骤，提供良好的位混合。
pub const FX_HASH_MULTIPLIER: u64 = 0x517C_C1B7_2722_0A95;

// ============================================================================
// 纯位操作函数（所有函数编译为 1-3 条机器指令）
// ============================================================================

/// 检查 raw bits 是否为 Smi 整数。
///
/// 机器码：`test rax, 0x7FFF000000000000; jz .Lis_smi`（1 指令）
#[inline(always)]
pub const fn is_smi(bits: u64) -> bool {
    (bits & SMI_MASK) == SMI_TAG
}

/// 检查 raw bits 是否为堆对象。
///
/// 机器码：`and rax, HEAP_MASK; cmp rax, HEAP_TAG`（2 指令）
#[inline(always)]
pub const fn is_heap_object(bits: u64) -> bool {
    (bits & HEAP_MASK) == HEAP_TAG
}

/// 检查 raw bits 是否为字符串值。
#[inline(always)]
pub const fn is_string(bits: u64) -> bool {
    (bits & STRING_MASK) == STRING_TAG
}

/// 检查 raw bits 是否为 nil 值。
#[inline(always)]
pub const fn is_nil(bits: u64) -> bool {
    bits == NIL_VALUE
}

/// 检查 raw bits 是否为布尔值。
#[inline(always)]
pub const fn is_bool(bits: u64) -> bool {
    bits == FALSE_VALUE || bits == TRUE_VALUE
}

/// 检查 raw bits 是否为规范 IEEE 754 浮点数（不在 NaN 标记空间）。
///
/// 机器码：`test rax, SPECIAL_MASK; jz .Lnot_float`（2 指令）
#[inline(always)]
pub const fn is_canonical_float(bits: u64) -> bool {
    (bits & SPECIAL_MASK) != SPECIAL_MASK && bits != CANONICAL_NAN
}

/// 检查 raw bits 是否可视为数字（Smi 或规范浮点数）。
#[inline(always)]
pub const fn is_number(bits: u64) -> bool {
    if is_smi(bits) {
        return true;
    }
    (bits & SPECIAL_MASK) != SPECIAL_MASK || bits == CANONICAL_NAN
}

/// 检查 raw bits 是否可视为 truthy（非 nil、非 false、非零）。
///
/// Falsy 值：NIL、FALSE、数值 0（包括 Smi 0、Float +0.0、Float -0.0）。
/// 其他所有值（TRUE、非零数字、字符串、堆对象）均为 truthy。
///
/// 注意：字符串和堆对象的 truthiness 总是 true，但字符串空串也是 truthy（Nuzo 语义）。
#[inline(always)]
pub const fn is_truthy(bits: u64) -> bool {
    // 快速拒绝：NIL 和 FALSE 是 PTR_TAG 空间中的特定位模式
    if bits == NIL_VALUE || bits == FALSE_VALUE {
        return false;
    }
    // Float +0.0（全零）和 Float -0.0（仅符号位置1）
    if bits == 0 || bits == 0x8000_0000_0000_0000 {
        return false;
    }
    // Smi 0：标签为 SMI_TAG，有效载荷为零 → (bits & SMI_VALUE_MASK) == 0
    if is_smi(bits) && (bits & SMI_VALUE_MASK) == 0 {
        return false;
    }
    true
}

/// 检查 raw bits 是否为 GC 管理的堆对象。
///
/// GC 管理的对象：`(bits & HEAP_MASK) == HEAP_TAG && (bits & GC_MANAGED_BIT) != 0`
#[inline(always)]
pub const fn is_gc_managed(bits: u64) -> bool {
    is_heap_object(bits) && (bits & GC_MANAGED_BIT) != 0
}

/// 检查 raw bits 是否为特殊值（非数字）。
#[inline(always)]
pub const fn is_special(bits: u64) -> bool {
    !is_number(bits)
}

/// 检查 raw bits 是否为指针类型值。
///
/// 指针编码：`(bits & QNAN_MASK) == PTR_TAG` 且不是 bool/nil/smi/string/heap。
#[inline(always)]
pub const fn is_ptr(bits: u64) -> bool {
    (bits & QNAN_MASK) == PTR_TAG
        && !is_bool(bits)
        && !is_nil(bits)
        && !is_smi(bits)
        && !is_string(bits)
        && !is_heap_object(bits)
}

/// 从 Smi raw bits 解码为 i64（有符号小整数）。
///
/// # 安全性
/// 调用方须确保 `is_smi(bits)` 为 true，否则结果无意义。
#[inline(always)]
pub const fn as_smi(bits: u64) -> i64 {
    debug_assert!(is_smi(bits), "called as_smi() on non-smi bits");
    smi_to_i64(bits)
}

/// 从 raw bits 提取布尔值。
///
/// # 安全性
/// 调用方须确保 `is_bool(bits)` 为 true。
#[inline(always)]
pub const fn as_bool(bits: u64) -> bool {
    debug_assert!(is_bool(bits), "called as_bool() on non-bool bits");
    bits == TRUE_VALUE
}

/// 从 raw bits 提取数值（Smi 或 Float → f64）。
///
/// # 安全性
/// 调用方须确保 `is_number(bits)` 为 true。
#[inline(always)]
pub fn as_number(bits: u64) -> f64 {
    if is_smi(bits) {
        smi_to_i64(bits) as f64
    } else {
        debug_assert!(is_number(bits), "called as_number() on non-number bits");
        f64::from_bits(bits)
    }
}

/// 提取 raw bits 的高 8 位标签（用于快速类型分派）。
#[inline(always)]
pub const fn tag_byte(bits: u64) -> u8 {
    (bits >> 56) as u8
}

// ============================================================================
// f64 ↔ u64 转换（LLVM intrinsic → 单条 movsd 指令）
// ============================================================================

/// u64 → f64：编译为 `movsd xmm0, qword ptr[rax]`
#[inline(always)]
pub fn to_f64(bits: u64) -> f64 {
    f64::from_bits(bits)
}

/// f64 → u64：编译为 `movsd qword ptr[rax], xmm0`
#[inline(always)]
pub fn from_f64(val: f64) -> u64 {
    val.to_bits()
}

// ============================================================================
// Smi 算术（纯位操作，零 FPU 调用）
// ============================================================================

/// Smi 加法（含溢出检测）。
///
/// 返回 `Some(result_bits)` 若结果在 Smi 范围内，否则 `None`。
#[inline(always)]
pub fn smi_add(a: u64, b: u64) -> Option<u64> {
    let result = a.wrapping_add(b).wrapping_sub(SMI_TAG);
    if (result & SMI_MASK) == SMI_TAG { Some(result) } else { None }
}

/// Smi 减法（含溢出检测）。
#[inline(always)]
pub fn smi_sub(a: u64, b: u64) -> Option<u64> {
    let result = a.wrapping_sub(b).wrapping_add(SMI_TAG);
    if (result & SMI_MASK) == SMI_TAG { Some(result) } else { None }
}

/// Smi 乘法（含溢出检测，溢出需退化为 Float）。
#[inline(always)]
pub fn smi_mul(a: u64, b: u64) -> Option<u64> {
    let ai = smi_to_i64(a);
    let bi = smi_to_i64(b);
    let result = ai.checked_mul(bi)?;
    if !(SMI_MIN..=SMI_MAX).contains(&result) {
        return None;
    }
    Some(SMI_TAG | (result as u64 & SMI_VALUE_MASK))
}

/// 将 Smi raw bits 解码为 i64。
#[inline(always)]
pub const fn smi_to_i64(bits: u64) -> i64 {
    let raw = bits & SMI_VALUE_MASK;
    if raw & SMI_SIGN_BIT != 0 { (raw as i64) - SMI_SIGN_EXTEND } else { raw as i64 }
}

/// 将 i64 编码为 Smi raw bits。
///
/// # Panics
/// 若 `i` 超出 Smi 范围 `[SMI_MIN, SMI_MAX]`。
#[inline(always)]
pub const fn i64_to_smi(i: i64) -> u64 {
    debug_assert!(i >= SMI_MIN && i <= SMI_MAX, "Smi overflow");
    SMI_TAG | (i as u64 & SMI_VALUE_MASK)
}
