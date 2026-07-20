//! MLIC (Multi-Level Inline Cache) -- 多级内联缓存调度系统
//!
//! 三级缓存架构，用于加速函数调用分发的热路径：
//!
//! ```text
//! L1 Monomorphic  → L2 Polymorphic (PIC, 4 slots) → L3 Megamorphic (Hash Table, 64 buckets)
//!     1 次比较            线性查找 + LRU                  FNV-1a + 线性探测
//! ```
//!
//! # 设计原则
//!
//! - **L1 单态缓存**：零额外分配，单个 `MonoCallCache`，一次指纹比较即可命中
//! - **L2 多态缓存**：固定 4 槽位的 PIC（Polymorphic Inline Cache），LRU 淘汰策略
//! - **L3 巨型态哈希表**：64 槽位开放寻址表，FNV-1a 哈希 + 线性探测（最多4步）
//! - **Arc 引用计数存储**：`Arc<FunctionPrototype>` 保证缓存期间原型对象不被释放
//! - **固定大小数组**：避免热路径上的堆分配

use std::sync::Arc;

use nuzo_values::function::FunctionPrototype;
use nuzo_values::heap::BuiltinFnPtr;

// ============================================================================
// 常量
// ============================================================================

/// PIC (Polymorphic Inline Cache) 最大槽位数
const PIC_MAX_SLOTS: usize = 4;

/// Megamorphic 哈希表大小（必须是 2 的幂，用于位掩码取模）
const MEGA_TABLE_SIZE: usize = 64;

/// 线性探测最大步数
const MEGA_PROBE_LIMIT: usize = 4;

/// 调用点统计汇总报告的 String 初始预分配容量
/// （Unicode 表格格式，典型输出约 200-300 字节，256 足以避免扩容）
const CALL_SITE_DEBUG_OUTPUT_CAPACITY: usize = 256;

// ============================================================================
// CallTargetType -- 调用目标类型
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum CallTargetType {
    #[default]
    Unknown = 0,
    Closure = 1,
    Builtin = 2,
}

// ============================================================================
// CallSiteState -- 调用点状态机
// ============================================================================

/// 调用点所处的缓存级别状态。
///
/// 状态转换规则：
/// ```text
/// Uninitialized ──[首次调用]──→ Monomorphic
/// Monomorphic ──[指纹不匹配]──→ Polymorphic
/// Polymorphic ──[PIC满且未命中]──→ Megamorphic
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallSiteState {
    /// 尚未初始化，无任何缓存信息
    #[default]
    Uninitialized = 0,
    /// 单态缓存：仅记录一个目标函数的指纹与元数据
    Monomorphic = 1,
    /// 多态缓存：PIC 表中记录最多 4 个不同目标
    Polymorphic = 2,
    /// 巨型态缓存：退化为 64 槽位哈希表
    Megamorphic = 3,
}

impl CallSiteState {
    /// 返回当前状态的数值标识（用于日志/调试输出）
    #[inline]
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

// ============================================================================
// MonoCallCache -- L1 单态缓存
// ============================================================================

/// L1 单态缓存条目。
///
/// 当一个调用点始终调用同一个目标函数时，此结构体以最低开销
/// 存储该目标的**指纹**和**缓存数据**。命中时仅需一次 `u64` 比较。
///
/// # 目标类型编码 (`target_type`)
///
/// | 值 | 含义 |
/// |----|------|
/// | 0  | Unknown / 未分类 |
/// | 1  | Closure（用户定义函数/闭包） |
/// | 2  | Builtin（内建函数） |
#[derive(Debug, Clone, Default)]
pub struct MonoCallCache {
    /// 目标函数的 64 位指纹（由调用方计算）
    pub target_fingerprint: u64,

    /// 调用目标 Value 的原始 u64 比特位（L1 零开销快速路径）
    pub raw_value_bits: u64,

    /// 目标类型: 0=Unknown, 1=Closure, 2=Builtin
    pub target_type: CallTargetType,

    // --- Closure 路径 ---
    /// 缓存的函数原型 Arc 引用（Closure 路径有效时非空）
    pub cached_prototype: Option<Arc<FunctionPrototype>>,

    // --- Builtin 路径 ---
    /// 缓存的内建函数名称
    pub cached_builtin_name: Option<String>,
    /// 缓存的内建函数指针
    pub cached_builtin_fn: Option<BuiltinFnPtr>,
    /// 缓存的内建函数元数
    pub cached_builtin_arity: Option<u8>,
}

impl MonoCallCache {
    /// 检查给定指纹是否匹配当前单态缓存。
    #[inline]
    pub fn matches(&self, fingerprint: u64) -> bool {
        self.target_fingerprint == fingerprint && self.target_type != CallTargetType::Unknown
    }

    /// L1 零开销快速路径：直接比较 Value 的原始 u64 比特位。
    ///
    /// 比 `matches(fingerprint)` 更快，因为跳过了 compute_call_fingerprint() 的
    /// 5-6 步操作，仅需一次 u64 比较。
    #[inline]
    pub fn matches_value_bits(&self, bits: u64) -> bool {
        self.raw_value_bits != 0
            && self.target_type != CallTargetType::Unknown
            && self.raw_value_bits == bits
    }

    /// 重置为未初始化状态。
    #[inline]
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// ============================================================================
// PicCallEntry + PolyCallCache -- L2 多态缓存 (PIC)
// ============================================================================

/// PIC 单个槽位条目。
///
/// 与 [`MonoCallCache`] 结构对称，支持同时缓存 Closure 和 Builtin 路径。
#[derive(Debug, Clone, Default)]
pub struct PicCallEntry {
    /// 目标函数指纹
    pub fingerprint: u64,
    /// 调用目标 Value 的原始 u64 比特位（L1 零开销快速路径）
    pub raw_value_bits: u64,
    pub target_type: CallTargetType,
    /// Closure 路径: 函数原型 Arc 引用
    pub cached_prototype: Option<Arc<FunctionPrototype>>,
    /// Builtin 路径: 函数名
    pub cached_builtin_name: Option<String>,
    /// Builtin 路径: 函数指针
    pub cached_builtin_fn: Option<BuiltinFnPtr>,
    /// Builtin 路径: 元数
    pub cached_builtin_arity: Option<u8>,
}

impl PicCallEntry {
    /// 创建一个新的 PIC 条目。
    #[inline]
    pub fn new(
        fingerprint: u64,
        target_type: CallTargetType,
        prototype: Option<Arc<FunctionPrototype>>,
        builtin_name: Option<String>,
        builtin_fn: Option<BuiltinFnPtr>,
        builtin_arity: Option<u8>,
        raw_value_bits: u64,
    ) -> Self {
        Self {
            fingerprint,
            raw_value_bits,
            target_type,
            cached_prototype: prototype,
            cached_builtin_name: builtin_name,
            cached_builtin_fn: builtin_fn,
            cached_builtin_arity: builtin_arity,
        }
    }
}

/// L2 多态缓存（Polymorphic Inline Cache）。
///
/// 固定 4 个槽位，采用 **MRU（Most Recently Used）** 策略：
/// 新命中的或新插入的条目总是被移到/放在 `entries[0]`，
/// 以优化局部性。
#[derive(Debug, Clone, Default)]
pub struct PolyCallCache {
    /// PIC 槽位数组
    pub entries: [PicCallEntry; PIC_MAX_SLOTS],
    /// 当前已占用槽数量
    pub count: u8,
}

impl PolyCallCache {
    /// 在 PIC 中线性查找匹配的指纹。
    ///
    /// 返回匹配条目的索引，未找到返回 `None`。
    #[inline]
    pub fn lookup(&self, fingerprint: u64) -> Option<usize> {
        (0..self.count as usize).find(|&i| {
            self.entries[i].fingerprint == fingerprint
                && self.entries[i].target_type != CallTargetType::Unknown
        })
    }

    /// 将指定索引的条目移到首位（MRU 提升）。
    ///
    /// 算法：对 [0..=idx] 区间执行 `rotate_right(1)`，
    /// 将 entries[idx] 旋转至 entries[0]，其余元素顺移。
    /// 时间复杂度 O(idx)，但 idx <= PIC_MAX_SLOTS=4。
    #[inline]
    pub fn promote_to_front(&mut self, idx: usize) {
        if idx == 0 || idx >= self.count as usize {
            return;
        }
        self.entries[..=idx].rotate_right(1);
    }

    /// 向 PIC 插入新条目。
    ///
    /// - 如果 PIC 未满（count < 4），插入到末尾并返回 `Ok(())`
    /// - 如果 PIC 已满，返回 `Err(entry)` 并将所有权交还调用方（避免冷路径的字符串 clone）
    #[inline]
    pub fn insert(&mut self, entry: PicCallEntry) -> Result<(), PicCallEntry> {
        let count = self.count as usize;
        if count < PIC_MAX_SLOTS {
            // 先检查是否已有相同指纹（更新语义）
            if let Some(existing) = self.lookup(entry.fingerprint) {
                self.entries[existing] = entry;
                self.promote_to_front(existing);
                return Ok(());
            }
            self.entries[count] = entry;
            self.count += 1;
            // 新插入的条目提升到首位
            self.promote_to_front(count);
            Ok(())
        } else {
            Err(entry)
        }
    }

    /// 重置 PIC 为空状态。
    #[inline]
    pub fn clear(&mut self) {
        self.entries = Default::default();
        self.count = 0;
    }
}

// ============================================================================
// MegaEntry + MegaCallTable -- L3 巨型态哈希表
// ============================================================================

/// Megamorphic 哈希表的单个桶条目。
///
/// 结构与 [`PicCallEntry`] 完全一致，以便在层级间无缝迁移数据。
#[derive(Debug, Clone, Default)]
pub struct MegaEntry {
    /// 目标函数指纹
    pub fingerprint: u64,
    /// 调用目标 Value 的原始 u64 比特位（L1 零开销快速路径）
    pub raw_value_bits: u64,
    pub target_type: CallTargetType,
    /// Closure 路径: 函数原型 Arc 引用
    pub cached_prototype: Option<Arc<FunctionPrototype>>,
    /// Builtin 路径: 函数名
    pub cached_builtin_name: Option<String>,
    /// Builtin 路径: 函数指针
    pub cached_builtin_fn: Option<BuiltinFnPtr>,
    /// Builtin 路径: 元数
    pub cached_builtin_arity: Option<u8>,
}

impl MegaEntry {
    /// 判断此桶是否为空（fingerprint == 0 表示空桶）。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.fingerprint == 0 && self.target_type == CallTargetType::Unknown
    }

    /// 从 PicCallEntry 创建 MegaEntry。
    #[inline]
    pub fn from_pic_entry(entry: &PicCallEntry) -> Self {
        Self {
            fingerprint: entry.fingerprint,
            raw_value_bits: entry.raw_value_bits,
            target_type: entry.target_type,
            cached_prototype: entry.cached_prototype.clone(),
            cached_builtin_name: entry.cached_builtin_name.clone(),
            cached_builtin_fn: entry.cached_builtin_fn,
            cached_builtin_arity: entry.cached_builtin_arity,
        }
    }
}

/// L3 巨型态哈希表（Megamorphic Call Cache）。
///
/// 64 槽位开放寻址哈希表，使用 **FNV-1a** 哈希函数将指纹映射到桶索引，
/// 冲突时采用线性探测（最多 4 步）。当 PIC 溢出时自动升级到此层。
#[derive(Debug, Clone)]
pub struct MegaCallTable {
    /// 哈希桶数组（fingerprint == 0 表示空桶）
    pub buckets: [MegaEntry; MEGA_TABLE_SIZE],
    /// 当前已占用桶数量
    pub count: u32,
}

impl Default for MegaCallTable {
    fn default() -> Self {
        Self { buckets: std::array::from_fn(|_| MegaEntry::default()), count: 0 }
    }
}

impl MegaCallTable {
    /// 使用 FNV-1a 哈希 + 线性探测查找条目。
    ///
    /// 返回对匹配条目的不可变引用，未找到返回 `None`。
    #[inline]
    pub fn lookup(&self, fingerprint: u64) -> Option<&MegaEntry> {
        let mask = (MEGA_TABLE_SIZE - 1) as u64;
        let mut idx = (fingerprint & mask) as usize;

        for _probe in 0..MEGA_PROBE_LIMIT {
            let bucket = &self.buckets[idx];
            if bucket.is_empty() {
                return None; // 遇到空桶，必然不存在
            }
            if bucket.fingerprint == fingerprint {
                return Some(bucket);
            }
            idx = (idx + 1) & (MEGA_TABLE_SIZE - 1);
        }
        None
    }

    /// 使用 FNV-1a 哈希 + 线性探测获取可变引用。
    #[inline]
    pub fn lookup_mut(&mut self, fingerprint: u64) -> Option<&mut MegaEntry> {
        let mask = (MEGA_TABLE_SIZE - 1) as u64;
        let mut idx = (fingerprint & mask) as usize;

        for _probe in 0..MEGA_PROBE_LIMIT {
            if self.buckets[idx].is_empty() {
                return None;
            }
            if self.buckets[idx].fingerprint == fingerprint {
                return Some(&mut self.buckets[idx]);
            }
            idx = (idx + 1) & (MEGA_TABLE_SIZE - 1);
        }
        None
    }

    /// 插入或更新条目。
    ///
    /// 如果指纹已存在则更新（in-place），否则在首个空桶插入。
    /// 如果表已满（所有探测位置均被占用），静默丢弃（极端情况）。
    #[inline]
    pub fn insert(&mut self, entry: MegaEntry) {
        let mask = (MEGA_TABLE_SIZE - 1) as u64;
        let mut idx = (entry.fingerprint & mask) as usize;

        for _probe in 0..MEGA_PROBE_LIMIT {
            let bucket = &mut self.buckets[idx];
            if bucket.is_empty() {
                // 遇到空桶，说明该指纹不在表中，直接插入并返回
                // 修复了原版代码中越过空桶继续探测，导致更新到无法被 lookup 找到的幽灵槽位的 bug
                *bucket = entry;
                self.count += 1;
                return;
            } else if bucket.fingerprint == entry.fingerprint {
                // 已存在：原地更新
                *bucket = entry;
                return;
            }
            idx = (idx + 1) & (MEGA_TABLE_SIZE - 1);
        }
        // 探测链上无空位且未找到相同指纹，极端情况下静默丢弃
    }

    /// 从 PolyCallCache 的条目批量初始化（升级路径）。
    pub fn from_poly_cache(poly: &PolyCallCache) -> Self {
        let mut table = Self::default();
        for i in 0..poly.count as usize {
            let entry = MegaEntry::from_pic_entry(&poly.entries[i]);
            table.insert(entry);
        }
        table
    }

    /// 重置哈希表为空状态。
    #[inline]
    pub fn clear(&mut self) {
        self.buckets = std::array::from_fn(|_| MegaEntry::default());
        self.count = 0;
    }
}

// ============================================================================
// CallSiteStats -- 统计信息
// ============================================================================

/// 单个调用点的缓存命中/未命中统计。
///
/// 用于性能分析和调优，可在运行时通过 [`CallSites::summary()`] 输出汇总报告。
#[derive(Debug, Clone, Default)]
pub struct CallSiteStats {
    /// 总调用次数
    pub total_calls: u64,
    /// L1 单态命中次数
    pub l1_hits: u64,
    /// L2 多态命中次数
    pub l2_hits: u64,
    /// L3 巨型态命中次数
    pub l3_hits: u64,
    /// 完全未命中次数（需走慢速分发路径）
    pub misses: u64,
}

impl CallSiteStats {
    /// 计算总命中率（百分比，保留两位小数）
    #[inline]
    pub fn hit_rate(&self) -> f64 {
        if self.total_calls == 0 {
            return 0.0;
        }
        let hits = self.l1_hits + self.l2_hits + self.l3_hits;
        (hits as f64 / self.total_calls as f64) * 100.0
    }

    /// 重置所有计数器
    #[inline]
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ============================================================================
// CacheHit -- 缓存命中结果
// ============================================================================

/// 快速分发命中的结果载荷。
///
/// 由 [`CallSite::fast_dispatch()`] 返回，携带足够的信息让 VM 直接跳转到
/// 对应的执行路径（Closure 调用 或 Builtin 调用）。
#[derive(Debug, Clone)]
pub struct CacheHit {
    pub target_type: CallTargetType,
    /// Closure 路径: 函数原型 Arc 引用
    pub cached_prototype: Option<Arc<FunctionPrototype>>,
    /// Builtin 路径: 函数指针
    pub cached_builtin_fn: Option<BuiltinFnPtr>,
    /// Builtin 路径: 元数
    pub cached_builtin_arity: Option<u8>,
}

// ============================================================================
// CallSite -- 单个调用点
// ============================================================================

/// 字节码中单个调用点的完整 MLIC 状态。
///
/// 每个 `OP_CALL` 指令对应一个 `CallSite` 实例，维护从 Uninitialized 到
/// Megamorphic 的完整状态机。
#[derive(Debug, Clone, Default)]
pub struct CallSite {
    /// 当前缓存级别状态
    pub state: CallSiteState,
    /// L1 单态缓存
    pub mono: MonoCallCache,
    /// L2 多态缓存 (PIC)
    pub poly: PolyCallCache,
    /// L3 巨型态哈希表（惰性分配，仅在需要时创建）
    pub mega: Option<Box<MegaCallTable>>,
    /// 命中/未命中统计
    pub stats: CallSiteStats,

    /// 缓存的 is_tail_position 结果 (None = 尚未计算/不适用)
    ///
    /// 对于同一个 call_ip，字节码不变则 tail 状态也不变。
    /// 仅在首次调用时计算，后续直接读取。
    pub cached_is_tail: Option<bool>,

    /// 🧬 CSTS: 闭包调用目标快照（单态闭包命中时预计算的 Chunk/Arity/Locals）
    ///
    /// 当 `state == Monomorphic && target_type == Closure` 且快路径验证通过后，
    /// 此字段持有发起闭包调用所需的全部静态信息，
    /// 使得 `execute_closure_fast()` 可完全跳过 HeapObject 解引和 Chunk 查找。
    pub closure_snapshot: Option<crate::vm::dispatch::ClosureSnapshot>,
}

impl CallSite {
    /// 创建新的未初始化调用点。
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// 尝试快速分发 -- 热路径。
    ///
    /// 根据当前 `state` 在对应的缓存层级中查找 `fingerprint`：
    /// - **Monomorphic**: 一次 `u64` 比较
    /// - **Polymorphic**: PIC 线性查找（最多 4 次）
    /// - **Megamorphic**: FNV-1a 哈希 + 线性探测（最多 4 步）
    ///
    /// 命中时返回 `Some(CacheHit)` 并更新统计；未命中返回 `None`。
    pub fn fast_dispatch(&mut self, fingerprint: u64) -> Option<CacheHit> {
        self.stats.total_calls += 1;

        match self.state {
            CallSiteState::Monomorphic => {
                if self.mono.matches(fingerprint) {
                    self.stats.l1_hits += 1;
                    Some(CacheHit {
                        target_type: self.mono.target_type,
                        cached_prototype: self.mono.cached_prototype.clone(),
                        cached_builtin_fn: self.mono.cached_builtin_fn,
                        cached_builtin_arity: self.mono.cached_builtin_arity,
                    })
                } else {
                    self.stats.misses += 1;
                    None
                }
            }

            CallSiteState::Polymorphic => {
                if let Some(idx) = self.poly.lookup(fingerprint) {
                    // 提取 Copy 类型的字段。必须在 promote_to_front 之前提取，
                    // 因为 promote_to_front 会移动数组元素，使得原有的 idx 失效。
                    let target_type = self.poly.entries[idx].target_type;
                    let cached_prototype = self.poly.entries[idx].cached_prototype.clone();
                    let cached_builtin_fn = self.poly.entries[idx].cached_builtin_fn;
                    let cached_builtin_arity = self.poly.entries[idx].cached_builtin_arity;

                    self.poly.promote_to_front(idx);
                    self.stats.l2_hits += 1;

                    Some(CacheHit {
                        target_type,
                        cached_prototype,
                        cached_builtin_fn,
                        cached_builtin_arity,
                    })
                } else {
                    self.stats.misses += 1;
                    None
                }
            }

            CallSiteState::Megamorphic => {
                if let Some(mega) = &self.mega {
                    if let Some(entry) = mega.lookup(fingerprint) {
                        self.stats.l3_hits += 1;
                        Some(CacheHit {
                            target_type: entry.target_type,
                            cached_prototype: entry.cached_prototype.clone(),
                            cached_builtin_fn: entry.cached_builtin_fn,
                            cached_builtin_arity: entry.cached_builtin_arity,
                        })
                    } else {
                        self.stats.misses += 1;
                        None
                    }
                } else {
                    // 异常状态：Megamorphic 但 mega 为 None
                    self.stats.misses += 1;
                    None
                }
            }

            CallSiteState::Uninitialized => {
                self.stats.misses += 1;
                None
            }
        }
    }

    /// 更新缓存 -- 冷路径（仅在 fast_dispatch 未命中后调用）。
    ///
    /// 根据当前状态执行缓存升级/插入：
    /// - **Uninitialized** → 写入 L1，状态变为 Monomorphic
    /// - **Monomorphic 且指纹不同** → 升级到 L2 (PIC)，原 L1 数据迁移至 PIC
    /// - **Polymorphic 且 PIC 未满** → 插入新条目到 PIC
    /// - **Polymorphic 且 PIC 已满** → 升级到 L3 (MegaCallTable)
    /// - **Megamorphic** → 插入/更新哈希表
    #[allow(clippy::too_many_arguments)] // 缓存更新需传递完整调用站点信息，拆分需引入 struct 增加间接层
    pub fn update_cache(
        &mut self,
        fingerprint: u64,
        target_type: CallTargetType,
        prototype: Option<Arc<FunctionPrototype>>,
        builtin_name: Option<String>,
        builtin_fn: Option<BuiltinFnPtr>,
        builtin_arity: Option<u8>,
        raw_value_bits: u64,
    ) {
        match self.state {
            CallSiteState::Uninitialized => {
                // 首次调用：写入 L1
                self.mono.target_fingerprint = fingerprint;
                self.mono.raw_value_bits = raw_value_bits;
                self.mono.target_type = target_type;
                self.mono.cached_prototype = prototype;
                self.mono.cached_builtin_name = builtin_name;
                self.mono.cached_builtin_fn = builtin_fn;
                self.mono.cached_builtin_arity = builtin_arity;
                self.state = CallSiteState::Monomorphic;
            }

            CallSiteState::Monomorphic => {
                if self.mono.target_fingerprint == fingerprint {
                    // 同一目标：更新 L1 数据（可能 prototype 变了，如闭包重定义）
                    self.mono.target_type = target_type;
                    self.mono.raw_value_bits = raw_value_bits;
                    self.mono.cached_prototype = prototype;
                    self.mono.cached_builtin_name = builtin_name;
                    self.mono.cached_builtin_fn = builtin_fn;
                    self.mono.cached_builtin_arity = builtin_arity;
                } else {
                    // 不同目标：升级到 L2，将原 L1 数据迁移到 PIC
                    let old_entry = PicCallEntry::new(
                        self.mono.target_fingerprint,
                        self.mono.target_type,
                        self.mono.cached_prototype.clone(),
                        self.mono.cached_builtin_name.take(),
                        self.mono.cached_builtin_fn,
                        self.mono.cached_builtin_arity,
                        self.mono.raw_value_bits,
                    );
                    // 使用 insert 自动维护 MRU 属性，修复原版手动赋值破坏 MRU 顺序的问题
                    let _ = self.poly.insert(old_entry);

                    let new_entry = PicCallEntry::new(
                        fingerprint,
                        target_type,
                        prototype,
                        builtin_name,
                        builtin_fn,
                        builtin_arity,
                        raw_value_bits,
                    );
                    let _ = self.poly.insert(new_entry);

                    self.state = CallSiteState::Polymorphic;
                }
            }

            CallSiteState::Polymorphic => {
                let new_entry = PicCallEntry::new(
                    fingerprint,
                    target_type,
                    prototype,
                    builtin_name,
                    builtin_fn,
                    builtin_arity,
                    raw_value_bits,
                );

                match self.poly.insert(new_entry) {
                    Ok(()) => {
                        // insert 成功则保持在 Polymorphic
                    }
                    Err(rejected_entry) => {
                        // PIC 已满：升级到 L3
                        let mut mega_table = MegaCallTable::from_poly_cache(&self.poly);
                        // 直接使用所有权转移，避免了原版中的 String clone 开销
                        let mega_entry = MegaEntry {
                            fingerprint: rejected_entry.fingerprint,
                            target_type: rejected_entry.target_type,
                            raw_value_bits: rejected_entry.raw_value_bits,
                            cached_prototype: rejected_entry.cached_prototype,
                            cached_builtin_name: rejected_entry.cached_builtin_name,
                            cached_builtin_fn: rejected_entry.cached_builtin_fn,
                            cached_builtin_arity: rejected_entry.cached_builtin_arity,
                        };
                        mega_table.insert(mega_entry);
                        self.mega = Some(Box::new(mega_table));
                        self.state = CallSiteState::Megamorphic;
                    }
                }
            }

            CallSiteState::Megamorphic => {
                if let Some(ref mut mt) = self.mega {
                    let entry = MegaEntry {
                        fingerprint,
                        target_type,
                        raw_value_bits,
                        cached_prototype: prototype,
                        cached_builtin_name: builtin_name,
                        cached_builtin_fn: builtin_fn,
                        cached_builtin_arity: builtin_arity,
                    };
                    mt.insert(entry);
                }
                // 若 mega 为 None（异常），忽略
            }
        }
    }

    /// 重置调用点到初始未初始化状态（释放所有缓存数据）。
    pub fn reset(&mut self) {
        self.state = CallSiteState::Uninitialized;
        self.mono.clear();
        self.poly.clear();
        self.mega = None;
        self.stats.reset();
        self.cached_is_tail = None;
    }
}

// ============================================================================
// CallSites -- 所有调用点的容器
// ============================================================================

/// 全局调用点集合，按字节码 IP 索引。
///
/// 在编译阶段根据 chunk 中的 `OP_CALL` 数量预分配，
/// 运行时每个 `OP_CALL` 通过其 IP 索引对应的 `CallSite`。
#[derive(Debug, Clone, Default)]
pub struct CallSites {
    /// 调用点向量，索引 = 字节码 IP
    sites: Vec<CallSite>,
}

impl CallSites {
    /// 创建空的调用点集合。
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// 预分配 N 个 CallSite（按字节码中 OP_CALL 的数量）。
    ///
    /// 如果新 size 小于当前长度则截断；大于则追加新的默认 CallSite。
    pub fn resize(&mut self, size: usize) {
        self.sites.resize_with(size, CallSite::new);
    }

    /// 确保 idx 位置有 CallSite（按需扩展，不缩小）
    ///
    /// 与 `resize()` 不同: `ensure()` 只扩展不缩小，适合按需初始化场景。
    /// 对于无函数调用的程序，call_sites 向量保持为空，零开销。
    #[inline]
    pub fn ensure(&mut self, idx: usize) -> &mut CallSite {
        if idx >= self.sites.len() {
            self.sites.resize_with(idx + 1, CallSite::default);
        }
        // SAFETY: idx < self.sites.len() guaranteed by the resize above
        unsafe { self.sites.get_unchecked_mut(idx) }
    }

    /// 获取指定 IP 的 CallSite 可变引用，若越界返回 None。
    ///
    /// 与 `ensure()` 不同：不触发分配，适合快速检查是否已有缓存。
    #[inline]
    pub fn get_mut_or_none(&mut self, ip: usize) -> Option<&mut CallSite> {
        self.sites.get_mut(ip)
    }

    /// 获取指定 IP 的 CallSite（可变引用）。
    ///
    /// # Panics
    ///
    /// 当 `ip >= sites.len()` 时 panic（调用方应确保 ip 有效）。
    #[inline]
    pub fn get_mut(&mut self, ip: usize) -> &mut CallSite {
        &mut self.sites[ip]
    }

    /// 获取指定 IP 的 CallSite（不可变引用）。
    ///
    /// # Panics
    ///
    /// 当 `ip >= sites.len()` 时 panic。
    #[inline]
    pub fn get(&self, ip: usize) -> &CallSite {
        &self.sites[ip]
    }

    /// 返回当前管理的调用点总数。
    #[inline]
    pub fn len(&self) -> usize {
        self.sites.len()
    }

    /// 是否没有任何调用点。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    /// 生成 Unicode 表格格式的统计汇总报告。
    ///
    /// 包含每个活跃调用点的状态、命中率和全局汇总行。
    pub fn summary(&self) -> String {
        use std::fmt::Write;

        let mut output = String::with_capacity(CALL_SITE_DEBUG_OUTPUT_CAPACITY);

        // 表头
        let _ =
            writeln!(output, "┌──────┬─────────────┬──────────┬───────┬───────┬───────┬─────────┐");
        let _ = writeln!(
            output,
            "│  IP  │    State    │ TotalCalls│ L1 Hit│ L2 Hit│ L3 Hit│ Misses  │"
        );
        let _ =
            writeln!(output, "├──────┼─────────────┼──────────┼───────┼───────┼───────┼─────────┤");

        let mut total_calls: u64 = 0;
        let mut total_l1: u64 = 0;
        let mut total_l2: u64 = 0;
        let mut total_l3: u64 = 0;
        let mut total_misses: u64 = 0;

        for (ip, site) in self.sites.iter().enumerate() {
            let s = &site.stats;
            // 仅输出有活动记录的行
            if s.total_calls == 0 {
                continue;
            }

            total_calls += s.total_calls;
            total_l1 += s.l1_hits;
            total_l2 += s.l2_hits;
            total_l3 += s.l3_hits;
            total_misses += s.misses;

            let state_str = match site.state {
                CallSiteState::Uninitialized => "Init",
                CallSiteState::Monomorphic => "Mono",
                CallSiteState::Polymorphic => "Poly",
                CallSiteState::Megamorphic => "Mega",
            };

            let _ = writeln!(
                output,
                "│ {:4} │ {:11} │ {:8} │ {:5} │ {:5} │ {:5} │ {:7} │",
                ip, state_str, s.total_calls, s.l1_hits, s.l2_hits, s.l3_hits, s.misses
            );
        }

        // 汇总行
        let _ =
            writeln!(output, "├──────┼─────────────┼──────────┼───────┼───────┼───────┼─────────┤");

        let overall_rate = if total_calls > 0 {
            ((total_l1 + total_l2 + total_l3) as f64 / total_calls as f64) * 100.0
        } else {
            0.0
        };

        let _ = writeln!(
            output,
            "│ {:4} │ {:11} │ {:8} │ {:5} │ {:5} │ {:5} │ {:7} │",
            "SUM", "TOTAL", total_calls, total_l1, total_l2, total_l3, total_misses
        );
        // 修正表格底边框长度，使其与表头严格对齐
        let _ =
            writeln!(output, "└──────┴─────────────┴──────────┴───────┴───────┴───────┴─────────┘");
        let _ = writeln!(output, "Overall hit rate: {:.2}%", overall_rate);

        output
    }

    /// 重置所有调用点。
    pub fn reset_all(&mut self) {
        for site in self.sites.iter_mut() {
            site.reset();
        }
    }
}

// ============================================================================
// FNV-1a 哈希辅助函数
// ============================================================================

/// FNV-1a 32-bit 偏移基值
pub const FNV_OFFSET_BASIS_32: u32 = 0x811c9dc5;

/// FNV-1a 32-bit 质数
pub const FNV_PRIME_32: u32 = 0x01000193;

/// FNV-1a 64-bit 偏移基值
pub const FNV_OFFSET_BASIS_64: u64 = 0xcbf29ce484222325;

/// FNV-1a 64-bit 质数
pub const FNV_PRIME_64: u64 = 0x00000100000001b3;

/// FNV-1a 32-bit 哈希函数。
///
/// 极快的非加密哈希，适用于 builtin name → fingerprint 映射等场景。
/// 对短字符串（典型 builtin 名称 < 20 字节）有极好的分布特性。
///
/// # 示例
///
/// ```rust
/// use nuzo_vm::vm_lic::fnv_hash_32;
///
/// let hash = fnv_hash_32(b"println");
/// assert_ne!(hash, 0);
/// ```
#[inline]
pub fn fnv_hash_32(bytes: &[u8]) -> u32 {
    let mut hash = FNV_OFFSET_BASIS_32;
    for &byte in bytes {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(FNV_PRIME_32);
    }
    hash
}

/// FNV-1a 64-bit 哈希函数。
///
/// 与 `fnv_hash_32` 算法一致，但产出 64 位指纹，降低碰撞概率。
/// 用于 [`MonoCallCache::target_fingerprint`] 和 [`MegaCallTable`] 的键。
///
/// # 示例
///
/// ```rust
/// use nuzo_vm::vm_lic::fnv_hash_64;
///
/// let fp = fnv_hash_64(b"my_function_v2");
/// assert_ne!(fp, 0);
/// ```
#[inline]
pub fn fnv_hash_64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS_64;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME_64);
    }
    hash
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // CallSiteState
    // -----------------------------------------------------------------------

    #[test]
    fn test_call_site_state_default_is_uninitialized() {
        let s = CallSiteState::default();
        assert_eq!(s, CallSiteState::Uninitialized);
        assert_eq!(s.as_u8(), 0);
    }

    #[test]
    fn test_call_site_state_values() {
        assert_eq!(CallSiteState::Uninitialized.as_u8(), 0);
        assert_eq!(CallSiteState::Monomorphic.as_u8(), 1);
        assert_eq!(CallSiteState::Polymorphic.as_u8(), 2);
        assert_eq!(CallSiteState::Megamorphic.as_u8(), 3);
    }

    // -----------------------------------------------------------------------
    // MonoCallCache
    // -----------------------------------------------------------------------

    #[test]
    fn test_mono_matches_on_same_fingerprint() {
        let cache = MonoCallCache {
            target_fingerprint: 42,
            target_type: CallTargetType::Closure,
            ..Default::default()
        };
        assert!(cache.matches(42));
        assert!(!cache.matches(99));
    }

    #[test]
    fn test_mono_clear() {
        let mut cache = MonoCallCache {
            target_fingerprint: 42,
            target_type: CallTargetType::Closure,
            ..Default::default()
        };
        cache.clear();
        assert_eq!(cache.target_fingerprint, 0);
        assert_eq!(cache.target_type, CallTargetType::Unknown);
    }

    // -----------------------------------------------------------------------
    // PolyCallCache (PIC)
    // -----------------------------------------------------------------------

    #[test]
    fn test_pic_lookup_and_insert() {
        let mut pic = PolyCallCache::default();

        // 插入 3 个条目
        for i in 1..=3u64 {
            let entry = PicCallEntry::new(i, CallTargetType::Closure, None, None, None, None, 0u64);
            assert!(pic.insert(entry).is_ok());
        }

        assert_eq!(pic.count, 3);

        // 查找存在的条目
        assert!(pic.lookup(1).is_some());
        assert!(pic.lookup(2).is_some());
        assert!(pic.lookup(3).is_some());
        assert!(pic.lookup(99).is_none());
    }

    #[test]
    fn test_pic_promote_to_front() {
        let mut pic = PolyCallCache::default();

        pic.entries[0] =
            PicCallEntry::new(1, CallTargetType::Closure, None, None, None, None, 0u64);
        pic.entries[1] =
            PicCallEntry::new(2, CallTargetType::Builtin, None, None, None, None, 0u64);
        pic.entries[2] =
            PicCallEntry::new(3, CallTargetType::Closure, None, None, None, None, 0u64);
        pic.count = 3;

        // 将 index=2 提升到前面
        pic.promote_to_front(2);

        // 现在 entries[0] 应该是原来的 entries[2]
        assert_eq!(pic.entries[0].fingerprint, 3);
        assert_eq!(pic.entries[1].fingerprint, 1);
        assert_eq!(pic.entries[2].fingerprint, 2);
    }

    #[test]
    fn test_pic_full_returns_err() {
        let mut pic = PolyCallCache::default();

        // 填满 4 个槽
        for i in 1..=4u64 {
            let entry = PicCallEntry::new(i, CallTargetType::Closure, None, None, None, None, 0u64);
            assert!(pic.insert(entry).is_ok());
        }

        // 第 5 个应该失败
        let entry = PicCallEntry::new(99, CallTargetType::Closure, None, None, None, None, 0u64);
        assert!(pic.insert(entry).is_err());
        assert_eq!(pic.count, 4);
    }

    #[test]
    fn test_pic_update_existing() {
        let mut pic = PolyCallCache::default();

        let _ =
            pic.insert(PicCallEntry::new(1, CallTargetType::Closure, None, None, None, None, 0u64));
        // 再次插入相同指纹 -> 更新而非新增
        assert!(
            pic.insert(PicCallEntry::new(
                1,
                CallTargetType::Builtin,
                None,
                Some("updated".to_string()),
                None,
                None,
                0u64
            ))
            .is_ok()
        );
        assert_eq!(pic.count, 1);
        assert_eq!(pic.entries[0].target_type, CallTargetType::Builtin);
        assert_eq!(pic.entries[0].cached_builtin_name.as_deref(), Some("updated"));
    }

    // -----------------------------------------------------------------------
    // MegaCallTable
    // -----------------------------------------------------------------------

    #[test]
    fn test_mega_lookup_and_insert() {
        let mut table = MegaCallTable::default();

        let entry = MegaEntry {
            fingerprint: 42,
            target_type: CallTargetType::Closure,
            ..Default::default()
        };
        table.insert(entry);

        assert_eq!(table.count, 1);
        assert!(table.lookup(42).is_some());
        assert!(table.lookup(99).is_none());
    }

    #[test]
    fn test_mega_update_existing() {
        let mut table = MegaCallTable::default();

        table.insert(MegaEntry {
            fingerprint: 10,
            target_type: CallTargetType::Closure,
            ..Default::default()
        });
        table.insert(MegaEntry {
            fingerprint: 10,
            target_type: CallTargetType::Builtin,
            ..Default::default()
        });

        assert_eq!(table.count, 1); // 不增加
        assert_eq!(table.lookup(10).unwrap().target_type, CallTargetType::Builtin);
    }

    #[test]
    fn test_mega_from_poly_cache() {
        let mut pic = PolyCallCache::default();
        let _ = pic.insert(PicCallEntry::new(
            10,
            CallTargetType::Closure,
            None,
            None,
            None,
            None,
            0u64,
        ));
        let _ = pic.insert(PicCallEntry::new(
            20,
            CallTargetType::Builtin,
            None,
            None,
            None,
            None,
            0u64,
        ));

        let table = MegaCallTable::from_poly_cache(&pic);
        assert!(table.lookup(10).is_some());
        assert!(table.lookup(20).is_some());
    }

    // -----------------------------------------------------------------------
    // CallSite -- 状态机转换
    // -----------------------------------------------------------------------

    #[test]
    fn test_call_site_uninitialized_to_monomorphic() {
        let mut site = CallSite::new();

        // fast_dispatch 在 Uninitialized 下总是 miss
        assert!(site.fast_dispatch(1).is_none());
        assert_eq!(site.stats.total_calls, 1);
        assert_eq!(site.stats.misses, 1);

        // update_cache: Uninitialized → Monomorphic
        site.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);
        assert_eq!(site.state, CallSiteState::Monomorphic);
        assert_eq!(site.mono.target_fingerprint, 1);
    }

    #[test]
    fn test_call_site_monomorphic_hit() {
        let mut site = CallSite::new();
        site.update_cache(42, CallTargetType::Closure, None, None, None, None, 0u64);

        // 同一指纹应命中 L1
        let hit = site.fast_dispatch(42);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().target_type, CallTargetType::Closure);
        assert_eq!(site.stats.l1_hits, 1);
    }

    #[test]
    fn test_call_site_mono_to_polymorphic() {
        let mut site = CallSite::new();
        site.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64); // → Mono

        // 不同指纹触发 miss
        assert!(site.fast_dispatch(2).is_none());

        // update_cache 不同目标 → 升级到 Poly
        site.update_cache(2, CallTargetType::Builtin, None, None, None, None, 0u64);
        assert_eq!(site.state, CallSiteState::Polymorphic);
        assert_eq!(site.poly.count, 2);
    }

    #[test]
    fn test_call_site_polymorphic_hit() {
        let mut site = CallSite::new();
        site.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);
        site.update_cache(2, CallTargetType::Builtin, None, None, None, None, 0u64); // → Poly

        // 两个指纹都应该能命中
        assert!(site.fast_dispatch(1).is_some());
        assert_eq!(site.stats.l2_hits, 1);

        assert!(site.fast_dispatch(2).is_some());
        assert_eq!(site.stats.l2_hits, 2);
    }

    #[test]
    fn test_call_site_poly_to_megamorphic() {
        let mut site = CallSite::new();

        // 填满 PIC: 1 (mono) + 3 (poly inserts) = 4 unique targets → triggers mega
        site.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64); // → Mono
        site.update_cache(2, CallTargetType::Builtin, None, None, None, None, 0u64); // → Poly (2 slots)
        site.update_cache(3, CallTargetType::Closure, None, None, None, None, 0u64); // Poly (3 slots)
        site.update_cache(4, CallTargetType::Builtin, None, None, None, None, 0u64); // Poly (4 slots, full)

        assert_eq!(site.state, CallSiteState::Polymorphic);
        assert_eq!(site.poly.count, 4);

        // 第 5 个不同目标触发升级到 Megamorphic
        site.update_cache(5, CallTargetType::Closure, None, None, None, None, 0u64);
        assert_eq!(site.state, CallSiteState::Megamorphic);
        assert!(site.mega.is_some());
    }

    #[test]
    fn test_call_site_megamorphic_hit() {
        let mut site = CallSite::new();

        // 快速填入多个目标直到 megamorphic
        for i in 1..=6u64 {
            site.update_cache(
                i,
                if i % 2 == 0 { CallTargetType::Builtin } else { CallTargetType::Closure },
                None,
                None,
                None,
                None,
                0u64,
            );
        }

        assert_eq!(site.state, CallSiteState::Megamorphic);

        // 之前的目标都应在 mega table 中
        for i in 1..=6u64 {
            assert!(site.fast_dispatch(i).is_some(), "fingerprint {} should hit", i);
        }
        assert_eq!(site.stats.l3_hits, 6);
    }

    #[test]
    fn test_call_site_reset() {
        let mut site = CallSite::new();
        site.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);
        site.update_cache(2, CallTargetType::Builtin, None, None, None, None, 0u64);
        site.update_cache(3, CallTargetType::Closure, None, None, None, None, 0u64);
        site.update_cache(4, CallTargetType::Builtin, None, None, None, None, 0u64);
        site.update_cache(5, CallTargetType::Closure, None, None, None, None, 0u64); // → Mega

        site.reset();

        assert_eq!(site.state, CallSiteState::Uninitialized);
        assert_eq!(site.mono.target_fingerprint, 0);
        assert_eq!(site.poly.count, 0);
        assert!(site.mega.is_none());
        assert_eq!(site.stats.total_calls, 0);
    }

    // -----------------------------------------------------------------------
    // CallSites 容器
    // -----------------------------------------------------------------------

    #[test]
    fn test_call_sites_resize_and_get() {
        let mut sites = CallSites::new();
        sites.resize(10);

        assert_eq!(sites.len(), 10);
        assert!(!sites.is_empty());

        // 获取并修改特定 IP 的 CallSite
        sites.get_mut(3).update_cache(100, CallTargetType::Closure, None, None, None, None, 0u64);
        assert_eq!(sites.get(3).state, CallSiteState::Monomorphic);
    }

    #[test]
    fn test_call_sites_summary_format() {
        let mut sites = CallSites::new();
        sites.resize(3);

        sites.get_mut(0).update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);
        let _ = sites.get_mut(0).fast_dispatch(1); // L1 hit

        sites.get_mut(1).update_cache(10, CallTargetType::Builtin, None, None, None, None, 0u64);
        sites.get_mut(1).update_cache(20, CallTargetType::Closure, None, None, None, None, 0u64);
        let _ = sites.get_mut(1).fast_dispatch(10); // L2 hit

        let summary = sites.summary();
        // 验证包含关键字段
        assert!(summary.contains("IP"));
        assert!(summary.contains("Mono"));
        assert!(summary.contains("Poly"));
        assert!(summary.contains("hit rate"));
    }

    #[test]
    fn test_call_sites_reset_all() {
        let mut sites = CallSites::new();
        sites.resize(5);
        sites.get_mut(0).update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);

        sites.reset_all();
        assert_eq!(sites.get(0).state, CallSiteState::Uninitialized);
    }

    // -----------------------------------------------------------------------
    // FNV Hash
    // -----------------------------------------------------------------------

    #[test]
    fn test_fnv_hash_32_deterministic() {
        let h1 = fnv_hash_32(b"hello");
        let h2 = fnv_hash_32(b"hello");
        assert_eq!(h1, h2);

        // 不同输入产生不同哈希
        assert_ne!(fnv_hash_32(b"hello"), fnv_hash_32(b"world"));
    }

    #[test]
    fn test_fnv_hash_64_deterministic() {
        let h1 = fnv_hash_64(b"println");
        let h2 = fnv_hash_64(b"println");
        assert_eq!(h1, h2);

        assert_ne!(fnv_hash_64(b"println"), fnv_hash_64(b"print"));
    }

    #[test]
    fn test_fnv_hash_empty_input() {
        // 空输入应返回 offset basis
        assert_eq!(fnv_hash_32(b""), FNV_OFFSET_BASIS_32);
        assert_eq!(fnv_hash_64(b""), FNV_OFFSET_BASIS_64);
    }

    // -----------------------------------------------------------------------
    // CallSiteStats
    // -----------------------------------------------------------------------

    #[test]
    fn test_stats_hit_rate() {
        let mut stats = CallSiteStats::default();
        assert_eq!(stats.hit_rate(), 0.0);

        stats.total_calls = 100;
        stats.l1_hits = 80;
        stats.l2_hits = 15;
        stats.l3_hits = 3;
        stats.misses = 2;

        // (80+15+3)/100 = 98%
        let rate = stats.hit_rate();
        assert!((rate - 98.0).abs() < 0.01);
    }

    // -----------------------------------------------------------------------
    // CacheHit
    // -----------------------------------------------------------------------

    #[test]
    fn test_cache_hit_closure_path() {
        use nuzo_values::function::FunctionPrototype;
        let proto = Arc::new(FunctionPrototype::new(
            "<anonymous>".to_string(),
            0,
            0,
            Arc::new(vec![]),
            Arc::new(vec![]),
            vec![],
            Arc::new(vec![]),
            Arc::new(Default::default()),
            0,
        ));
        let hit = CacheHit {
            target_type: CallTargetType::Closure,
            cached_prototype: Some(proto),
            cached_builtin_fn: None,
            cached_builtin_arity: None,
        };
        assert_eq!(hit.target_type, CallTargetType::Closure);
        assert!(hit.cached_prototype.is_some());
        assert!(hit.cached_builtin_fn.is_none());
    }

    #[test]
    fn test_cache_hit_builtin_path() {
        let hit = CacheHit {
            target_type: CallTargetType::Builtin,
            cached_prototype: None,
            cached_builtin_fn: Some(|_| Ok(nuzo_core::Value::from_number(0.0))),
            cached_builtin_arity: Some(0),
        };
        assert_eq!(hit.target_type, CallTargetType::Builtin);
        assert!(hit.cached_builtin_fn.is_some());
        assert_eq!(hit.cached_builtin_arity, Some(0));
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_call_site_state_as_u8_all_variants() {
        assert_eq!(CallSiteState::Uninitialized.as_u8(), 0);
        assert_eq!(CallSiteState::Monomorphic.as_u8(), 1);
        assert_eq!(CallSiteState::Polymorphic.as_u8(), 2);
        assert_eq!(CallSiteState::Megamorphic.as_u8(), 3);
    }

    #[test]
    fn test_mono_matches_value_bits_matching() {
        let cache = MonoCallCache {
            raw_value_bits: 0x1234,
            target_type: CallTargetType::Closure,
            ..Default::default()
        };
        assert!(cache.matches_value_bits(0x1234));
    }

    #[test]
    fn test_mono_matches_value_bits_nonmatching() {
        let cache = MonoCallCache {
            raw_value_bits: 0x1234,
            target_type: CallTargetType::Closure,
            ..Default::default()
        };
        assert!(!cache.matches_value_bits(0x5678));
    }

    #[test]
    fn test_mono_matches_value_bits_zero_bits() {
        let cache = MonoCallCache::default();
        // raw_value_bits == 0 means not set
        assert!(!cache.matches_value_bits(0));
    }

    #[test]
    fn test_mega_entry_from_pic_entry() {
        let pic = PicCallEntry::new(42, CallTargetType::Closure, None, None, None, None, 0u64);
        let mega = MegaEntry::from_pic_entry(&pic);
        assert_eq!(mega.fingerprint, 42);
    }

    #[test]
    fn test_mega_call_table_lookup_mut_existing() {
        let mut table = MegaCallTable::default();
        let entry = MegaEntry::from_pic_entry(&PicCallEntry::new(
            100,
            CallTargetType::Closure,
            None,
            None,
            None,
            None,
            0u64,
        ));
        // Insert at the hashed slot
        let mask = (MEGA_TABLE_SIZE - 1) as u64;
        let idx = (100u64 & mask) as usize;
        table.buckets[idx] = entry;
        let found = table.lookup_mut(100);
        assert!(found.is_some());
        if let Some(e) = found {
            assert_eq!(e.fingerprint, 100);
        }
    }

    #[test]
    fn test_mega_call_table_lookup_mut_nonexistent() {
        let mut table = MegaCallTable::default();
        assert!(table.lookup_mut(999).is_none());
    }

    #[test]
    fn test_call_sites_ensure_grows() {
        let mut sites = CallSites::default();
        assert_eq!(sites.len(), 0);
        sites.ensure(5);
        assert_eq!(sites.len(), 6);
    }

    #[test]
    fn test_call_sites_ensure_existing() {
        let mut sites = CallSites::default();
        sites.ensure(3);
        let len_before = sites.len();
        sites.ensure(2); // idx < len, should not grow
        assert_eq!(sites.len(), len_before);
    }

    #[test]
    fn test_call_sites_get_mut_or_none_existing() {
        let mut sites = CallSites::default();
        sites.ensure(3);
        let site = sites.get_mut_or_none(2);
        assert!(site.is_some());
    }

    #[test]
    fn test_call_sites_get_mut_or_none_nonexistent() {
        let mut sites = CallSites::default();
        let site = sites.get_mut_or_none(999);
        assert!(site.is_none());
    }

    #[test]
    fn test_call_site_fast_dispatch_uninitialized() {
        let mut cs = CallSite::default();
        // Uninitialized state: should return None
        assert!(cs.fast_dispatch(42).is_none());
    }

    #[test]
    fn test_call_site_fast_dispatch_monomorphic_hit() {
        let mut cs = CallSite::default();
        cs.update_cache(42, CallTargetType::Closure, None, None, None, None, 0u64);
        let hit = cs.fast_dispatch(42);
        assert!(hit.is_some());
    }

    #[test]
    fn test_call_site_fast_dispatch_monomorphic_miss() {
        let mut cs = CallSite::default();
        cs.update_cache(42, CallTargetType::Closure, None, None, None, None, 0u64);
        let hit = cs.fast_dispatch(99);
        assert!(hit.is_none());
    }

    #[test]
    fn test_call_site_update_cache_uninitialized_to_mono() {
        let mut cs = CallSite::default();
        assert_eq!(cs.state, CallSiteState::Uninitialized);
        cs.update_cache(1, CallTargetType::Closure, None, None, None, None, 0u64);
        assert_eq!(cs.state, CallSiteState::Monomorphic);
        assert_eq!(cs.mono.target_fingerprint, 1);
    }
}
