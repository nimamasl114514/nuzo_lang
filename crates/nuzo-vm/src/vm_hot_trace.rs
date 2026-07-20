//! # Hot Trace Cache - 热路径序列哈希缓存
//!
//! 对频繁执行的字节码序列计算 FNV-1a 哈希指纹。
//! 命中时批量执行整个序列，跳过 N 次 fetch+dispatch+decode 循环往返。
//!
//! ## 设计目标
//!
//! - **极速查询**: `check()` 必须是 O(1)，每条指令都调用
//! - **轻量采样**: `profile()` 仅做计数器递增和比较，无堆分配
//! - **零开销冷启动**: Cold 状态不影响正常执行速度
//!
//! ## 使用场景
//!
//! VM 主循环:
//!   1. check(ip) -> 命中? -> 批量执行热路径 -> 跳到 end_ip
//!   2. 正常 dispatch
//!   3. profile(ip) -> 采样频率数据
//!
//! ## 性能特征
//!
//! | 操作       | 时间复杂度 | 内存分配     | 调用频率   |
//! |-----------|-----------|-------------|-----------|
//! | check()   | O(1)      | 无          | 每条指令   |
//! | profile() | O(1) 均   | 可能 resize  | 每条指令   |
//! | mark_hot()| O(1) 均 | XxHashMap insert| 检测循环时 |
//!
//! ## 热点追踪算法详解
//!
//! ### 三态状态机模型
//!
//! 本模块采用**三态有限状态机**（Finite State Machine, FSM）来管理热路径的生命周期：
//!
//! ```text
//! ┌──────────┐    hit_count >= warming_threshold    ┌───────────┐
//! │          ├─────────────────────────────────────→│           │
//! │  Cold    │                                     │ Warming(N)│
//! │          │                                     │           │
//! └──────────┘                                     └─────┬─────┘
//!                                                        │
//!                                              N 次采样后
//!                                              (remaining == 0)
//!                                                        │
//!                                                        v
//!                                                   ┌──────────┐
//!                                                   │          │
//!                                                   │   Hot    │
//!                                                   │ (就绪)   │
//!                                                   └──────────┘
//! ```
//!
//! ### 阈值调优策略
//!
//! #### warming_threshold = 500（默认值）
//! - **作用**: 控制何时从 Cold 状态进入 Warming 状态
//! - **调优依据**:
//!   - 过低（<100）：噪声干扰大，临时代码路径可能被误判为热路径
//!   - 过高（>2000）：延迟识别真正的热路径，错过优化窗口期
//!   - 500 是经验值，适用于大多数解释型语言工作负载
//! - **性能影响**: 此阈值直接影响内存占用和 CPU 开销的平衡点
//!
//! #### hot_threshold = 2000（参考值）
//! - **作用**: 提供给 VM 的建议阈值，用于决定是否调用 `mark_hot()`
//! - **注意**: 实际的状态转换由 Warming 计数器控制，此值仅作参考
//! - **设计理由**: 将采样决策权交给 VM 层，便于实现特定优化策略
//!
//! #### max_trace_length = 16（默认值）
//! - **作用**: 单条热路径的最大指令数限制
//! - **设计权衡**:
//!   - 太短（<8）：无法覆盖完整的基本块，加速效果不明显
//!   - 太长（>32）：增加哈希碰撞概率，且长序列稳定性差
//!   - 16 条指令足以覆盖大多数循环体和热点函数
//! - **内存影响**: 每个条目增加约 100 字节（含 hash 和元数据）
//!
//! #### max_traces = 128（默认值）
//! - **作用**: 同时追踪的最大独立 IP 数量
//! - **资源估算**: 128 × sizeof(HotTraceEntry) ≈ 1.5 KB（可忽略）
//! - **扩展性**: 对于大型程序，可通过配置调整此参数
//!
//! ## FNV-1a 哈希算法选择理由
//!
//! 选择 FNV-1a 而非其他哈希算法的原因：
//!
//! 1. **性能优势**: 仅需 XOR + MUL 两个 CPU 指令，无分支预测失败风险
//! 2. **分布均匀性**: 对于短序列（≤16 字节）具有优秀的雪崩效应
//! 3. **无状态特性**: 不需要维护内部状态，适合内联到热路径中
//! 4. **碰撞概率**: 64 位输出空间下，碰撞概率 < 2^(-64)，实际可忽略
//!
//! ## 缓存一致性保证
//!
//! 本模块不处理字节码修改导致的缓存失效问题。
//! 当字节码被重新编译或优化时，VM 负责调用 `clear()` 重置所有热路径数据。
//! 这种设计简化了实现复杂度，将一致性责任上移至 VM 层。

use nuzo_bytecode::{Chunk, Opcode};
use nuzo_core::{XxHashMap, xx_hash_map};

use crate::vm_lic::{FNV_OFFSET_BASIS_64, FNV_PRIME_64};

// ============================================================================
// Superinstruction Fusion 数据结构
// ============================================================================

/// 微操作类型（未来扩展）
///
/// 当前为占位符，未来可扩展为具体的融合微操作，例如：
/// - `LoadConstant { dest, const_idx }` — 常量加载
/// - `BinaryOp { op, dest, left, right }` — 二元运算
/// - `Guard { condition, fallback_ip }` — 类型守卫
///
/// 每个 MicroOp 对应一条或多条原始指令的语义等价操作，
/// 但跳过了 fetch→decode→dispatch 的开销。
#[derive(Debug, Clone)]
pub enum MicroOp {
    /// 占位符：不执行任何操作
    Nop,
}

/// 融合循环入口（Superinstruction Fusion 的元数据）
///
/// 描述一段已被融合优化的字节码序列：
/// - `start_ip`: 融合序列在字节码中的起始位置
/// - `length`: 融合序列包含的原始指令数
/// - `micro_ops`: 微操作序列（当前为空，未来填充）
///
/// ## 生命周期
///
/// 1. HotTrace 检测到热点 IP → 状态转为 Hot
/// 2. `get_fused_entry(ip)` 查询是否有对应的融合入口
/// 3. 若存在，VM 调用 `execute_fused_loop()` 执行微操作序列
/// 4. 若 Guard 检查失败，回退到正常 dispatch 路径
///
/// ## 扩展路径
///
/// 当前 `micro_ops` 始终为空（保守策略），未来可由 JIT 编译器
/// 或 AOT 分析器填充具体的微操作序列。
#[derive(Debug, Clone)]
pub struct FusedLoopEntry {
    /// 融合序列起始 IP
    pub start_ip: usize,
    /// 融合序列长度（原始指令数）
    pub length: usize,
    /// 微操作序列（当前为空，未来由 JIT/AOT 填充）
    pub micro_ops: Vec<MicroOp>,
}

// ============================================================================
// 可追溯性事件（Hot Trace Hit/Abort Events）
// ============================================================================

/// 热路径执行事件记录，用于可追溯性分析。
///
/// 仅在 tracer 启用时记录，零运行时开销（冷路径）。
///
/// # 事件类型
///
/// - `Hit`: 热路径成功批量执行（从 start_ip 到 end_ip）
/// - `Abort`: 热路径执行中途退出（guard 触发、chunk 切换、迭代上限等）
#[derive(Debug, Clone)]
pub enum HotTraceEvent {
    /// 热路径命中：成功进入批量执行模式
    Hit {
        /// 热路径起始 IP
        start_ip: usize,
        /// 热路径终止 IP
        end_ip: usize,
    },
    /// 热路径中止：guard 条件触发导致提前退出
    Abort {
        /// 中止时的 IP
        ip: usize,
        /// 中止原因（静态字符串，避免分配）
        reason: &'static str,
    },
}

// ============================================================================
// 容量常量
// ============================================================================

/// HotTraceTable entries Vec 初始预分配容量
/// （64 个 IP 槽位足以覆盖大多数函数的热路径，按需懒扩容）
const HOT_TRACE_ENTRIES_INITIAL_CAPACITY: usize = 64;

/// HotTraceTable hash_index HashMap 初始预分配容量
/// （32 个桶对应约 50% load factor 下的 16 条热路径，减少 rehash）
const HOT_TRACE_HASH_INITIAL_CAPACITY: usize = 32;

// ============================================================================
// 数据结构定义
// ============================================================================

/// 热路径状态机
///
/// 三态转换: Cold -> Warming -> Hot
///
/// Cold --[hit_count >= warming_threshold]--> Warming(N)
///                                              |
///                                      [N 次采样后]
///                                              v
///                                           Hot (就绪)
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TraceStatus {
    /// 未采样或采样次数不足
    Cold,
    /// 采样中（剩余采样次数）
    /// 当 remaining 递减至 0 时转为 Hot
    Warming(u32),
    /// 就绪状态，可被 check() 命中加速
    Hot,
}

/// 单条热路径记录
///
/// 存储一条已被识别为"热"的字节码序列信息。
/// 当 `status == Hot` 时，VM 可以跳过逐条解码，
/// 直接批量执行从 `start_ip` 到 `end_ip` 的整个序列。
///
/// 字段为私有，通过访问器方法读取，通过状态转换方法修改。
#[derive(Debug)]
pub struct HotTraceEntry {
    /// FNV-1a 序列指纹（字节码内容的哈希）
    hash: u64,
    /// 起始字节码 IP（指令指针）
    start_ip: usize,
    /// 序列长度（指令数），最大值由 config.max_trace_length 控制
    length: u8,
    /// 终止 IP（start_ip + sequence_length）
    end_ip: usize,
    /// 累计命中计数（用于统计和调试）
    hit_count: u32,
    /// 当前状态（Cold / Warming / Hot）
    status: TraceStatus,
}

impl HotTraceEntry {
    /// 返回 FNV-1a 序列指纹
    #[inline]
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// 返回起始字节码 IP
    #[inline]
    pub fn start_ip(&self) -> usize {
        self.start_ip
    }

    /// 返回序列长度（指令数）
    #[inline]
    pub fn length(&self) -> u8 {
        self.length
    }

    /// 返回终止 IP
    #[inline]
    pub fn end_ip(&self) -> usize {
        self.end_ip
    }

    /// 返回累计命中计数
    #[inline]
    pub fn hit_count(&self) -> u32 {
        self.hit_count
    }

    /// 返回当前状态
    #[inline]
    pub fn status(&self) -> TraceStatus {
        self.status
    }

    /// 判断是否处于 Hot 状态
    #[inline]
    pub fn is_hot(&self) -> bool {
        self.status == TraceStatus::Hot
    }
}

impl Default for HotTraceEntry {
    fn default() -> Self {
        Self { hash: 0, start_ip: 0, length: 0, end_ip: 0, hit_count: 0, status: TraceStatus::Cold }
    }
}

// ============================================================================
// 配置
// ============================================================================

/// 热路径追踪配置
///
/// 控制何时开始采样、何时启用加速、以及资源上限。
///
/// ## 默认值调优说明
///
/// - `warming_threshold = 500`: 需要至少 500 次执行才开始采样
///   - 过低：噪声干扰大，误判率高
///   - 过高：延迟识别真正的热路径
/// - `hot_threshold = 2000`: 实际在 profile() 中未直接使用，
///   但 mark_hot() 的调用时机应由 VM 根据此阈值决定
/// - `max_trace_length = 16`: 单条热路径最多 16 条指令
///   - 平衡：太长会增加哈希碰撞概率，太短收益不足
/// - `max_traces = 128`: 最多同时追踪 128 个不同的起始 IP
///   - 内存占用：128 * sizeof(HotTraceEntry) ≈ 1.5 KB
#[derive(Debug, Clone)]
pub struct HotTraceConfig {
    /// 开始采样的阈值（默认 500）
    ///
    /// 当某 IP 的 hit_count 达到此值时，状态从 Cold 转为 Warming。
    pub warming_threshold: u32,

    /// 启用加速的阈值（默认 2000）
    ///
    /// 此值供 VM 参考决定何时调用 mark_hot()。
    /// 实际的 Hot 转换由 Warming 计数器控制。
    pub hot_threshold: u32,

    /// 单条最大指令数（默认 16）
    ///
    /// 超过此长度的序列不会被标记为热路径。
    pub max_trace_length: u8,

    /// 最大缓存条目数（默认 128）
    ///
    /// 当活跃条目数量达到此上限时，新的 IP 将被忽略。
    pub max_traces: usize,

    /// Batch 执行的最小指令数阈值（默认 5）
    ///
    /// 当扫描到的循环体指令数小于此值时，跳过 batch 生成，
    /// 回退到逐条 dispatch 执行。避免极短 trace 的 batch
    /// 固定开销（函数调用 + while 循环 + 融合检查）超过
    /// 节省的 dispatch 次数，导致净性能退化。
    ///
    /// 临界长度估算：FixedOverhead ≈ 2.5 条指令等价开销，
    /// DispatchSavingPerIns ≈ 0.8 条指令/次，
    /// CriticalLength = ceil(2.5 / 0.8) = 4，取 5 留余量。
    pub min_batch_length: u8,
}

impl Default for HotTraceConfig {
    fn default() -> Self {
        Self {
            warming_threshold: 500,
            hot_threshold: 2000,
            max_trace_length: 16,
            max_traces: 128,
            min_batch_length: 5,
        }
    }
}

// ============================================================================
// 主结构：HotTraceTable
// ============================================================================

/// 热路径缓存表
///
/// 核心数据结构，管理所有热路径条目。
///
/// ## 内部布局
///
/// [HotTraceTable]
///   entries: Vec<HotTraceEntry>
///     [0] -> IP=0 的条目
///     [1] -> IP=1 的条目
///     ...
///     [N] -> IP=N 的条目
///
///   hash_index: XxHashMap<u64, usize>
///     hash -> entries 索引
///
///   config: HotTraceConfig
///   total_profiled: u64
///   total_hits: u64
///
/// ## 设计决策
///
/// 1. **Vec 按 IP 索引**（而非 HashMap）:
///    - `check(ip)` 只需一次边界检查 + 数组索引 = 极速 O(1)
///    - 代价：稀疏数组可能浪费内存（但 max_traces=128 时代价很小）
///
/// 2. **HashMap 辅助索引**:
///    - 用于通过 hash 快速查找对应的 IP
///    - 在 mark_hot() 时建立映射
///
/// 3. **预分配 vs 懒扩容**:
///    - 采用懒扩容策略（resize_with）：
///    - 冷启动时零分配，仅在实际使用时增长
pub struct HotTraceTable {
    /// 按 IP 索引的条目表（稀疏数组）
    ///
    /// 使用 Default 填充未使用的槽位。
    /// 访问前必须检查 ip < len。
    entries: Vec<HotTraceEntry>,

    /// FNV-1a 哈希 -> entries 索引的倒排索引
    ///
    /// 仅在 mark_hot() 时插入，用于快速查找已知热路径。
    hash_index: XxHashMap<u64, usize>,

    /// 配置参数
    config: HotTraceConfig,

    /// 总采样次数（用于统计和调优）
    total_profiled: u64,

    /// 总命中次数（check() 返回 Some 的次数）
    total_hits: u64,

    /// [新增] 追踪当前已激活的独立 IP 数量（hit_count > 0 的条目数）
    /// 用于正确实施 max_traces 限制，避免被稀疏数组的最大 IP 干扰。
    active_traces: usize,

    /// 热路径执行事件日志（仅 tracer 启用时记录）
    ///
    /// 记录 Hit/Abort 事件，用于可追溯性分析。
    /// 冷路径开销：仅在 tracer 启用时写入，正常执行时为空 Vec。
    pub events: Vec<HotTraceEvent>,

    // ========================================================================
    // Superinstruction 融合优化：Opcode 相邻对统计器
    // ========================================================================
    //
    // 设计目标：
    // - 统计 VM 执行过程中高频出现的 opcode 相邻对 (Opcode_A, Opcode_B)
    // - 为 fused handler（超级指令）提供数据驱动的融合决策依据
    // - 仅在 Hot/Warming 状态下记录，避免冷启动噪声污染统计数据
    //
    // 使用场景：
    // 1. profile() 方法在每次执行后记录 (last_opcode, current_opcode) 对
    // 2. top_pairs() 方法返回 Top-N 高频相邻对，用于指导 fused handler 选择
    // 3. 编译期/运行时可基于此数据生成或选择最优的 superinstruction 组合
    //
    // 性能特征：
    /// - 时间复杂度: O(1) 摊还（XxHashMap 插入/更新）
    // - 内存占用: O(unique_pairs)，典型工作负载 < 100 个唯一相邻对
    // - 开销影响: 仅在 Hot/Warming 状态下启用，Cold 状态零开销

    ///   Opcode 相邻对频率计数器
    ///
    /// Key: (前一条 Opcode discriminant, 当前 Opcode discriminant) 有序对
    /// Value: 该相邻对的出现次数
    ///
    /// ## 实现细节
    ///
    /// 使用 `Discriminant<Opcode>` 而非 `(Opcode, Opcode)` 或 `(u8, u8)` 的原因：
    /// - `Opcode` 枚举未实现 `Hash` trait（由 nuzo_proc::define_opcodes! 宏生成）
    /// - `u8` 需要 `From<Opcode>` 实现，但 Opcode 也未提供此转换
    /// - `Discriminant<Opcode>` 由 `std::mem::discriminant()` 生成，天然支持 Hash + Eq + Copy
    /// - 零运行时开销：仅提取枚举的 discriminant 值（编译期已知偏移）
    /// - 类型安全：保证只比较同一枚举类型的变体身份
    ///
    /// ## 线程安全
    ///
    /// 此字段仅被 `profile()` 方法修改，而 `profile()` 仅在 VM 主循环的单线程上下文中调用，
    /// 因此无需同步原语。
    pair_counter: XxHashMap<(std::mem::Discriminant<Opcode>, std::mem::Discriminant<Opcode>), u32>,

    /// 上一条执行的 Opcode（用于构建相邻对）
    ///
    /// ## 生命周期
    ///
    /// - 在 `profile()` 中更新：每次执行后记录当前 opcode 作为"上一条"
    /// - 在 `clear()` 中重置为 None
    /// - 初始值为 None（表示尚未执行任何指令）
    ///
    /// ## 状态机语义
    ///
    /// ```text
    /// None --[第一条指令]--> Some(Opcode_A)
    ///   |--[第二条指令]--> Some(Opcode_B) + 记录 (A, B) 相邻对
    ///   |--[第三条指令]--> Some(Opcode_C) + 记录 (B, C) 相邻对
    ///   | ...
    /// ```
    last_opcode: Option<Opcode>,
}

impl Default for HotTraceTable {
    fn default() -> Self {
        Self::new()
    }
}

impl HotTraceTable {
    /// 创建使用默认配置的热路径表
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let mut table = HotTraceTable::new();
    /// assert_eq!(table.stats().2, 0); // 无条目
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(HotTraceConfig::default())
    }

    /// 创建使用自定义配置的热路径表
    ///
    /// # Parameters
    ///
    /// - `config`: 热路径追踪配置参数
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_vm::vm_hot_trace::{HotTraceConfig, HotTraceTable};
    ///
    /// let config = HotTraceConfig {
    ///     warming_threshold: 100,  // 更激进的采样
    ///     ..Default::default()
    /// };
    /// let table = HotTraceTable::with_config(config);
    /// assert_eq!(table.config().warming_threshold, 100);
    /// ```
    pub fn with_config(config: HotTraceConfig) -> Self {
        Self {
            // 预分配容量以减少首次扩容
            entries: Vec::with_capacity(HOT_TRACE_ENTRIES_INITIAL_CAPACITY),
            hash_index: xx_hash_map(HOT_TRACE_HASH_INITIAL_CAPACITY),
            config,
            total_profiled: 0,
            total_hits: 0,
            active_traces: 0,
            events: Vec::new(),
            // Superinstruction 统计器初始化：预分配 16 个桶（覆盖常见相邻对）
            pair_counter: xx_hash_map(16),
            last_opcode: None,
        }
    }

    // ========================================================================
    // 核心方法：O(1) 极速查询
    // ========================================================================

    /// 检查当前 IP 是否是已知热路径起点
    ///
    /// **这是性能关键路径！** 在 VM 主循环的最顶部调用，
    /// 每条指令都会经过此函数。必须保持极简：
    ///
    /// - 一次边界检查
    /// - 一次数组索引
    /// - 一次字段比较
    ///
    /// # Parameters
    ///
    /// - `ip`: 当前指令指针
    ///
    /// # Returns
    ///
    /// - `Some(entry)` 如果该 IP 是已就绪的热路径起点
    /// - `None` 否则（包括 Cold、Warming、或未知 IP）
    ///
    /// # Performance
    ///
    /// 时间复杂度: **O(1)** （最坏情况也是常数时间）
    /// 内存分配: **无**
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_bytecode::Opcode;
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let mut table = HotTraceTable::new();
    ///
    /// // 未标记的热路径不会被命中
    /// assert!(table.check(42).is_none());
    ///
    /// // 先让系统知道这个 IP 存在
    /// table.profile(42, Opcode::LoadK);
    ///
    /// // 手动标记一个热路径后才能命中
    /// table.mark_hot(42, 12345u64, 8, 50);
    /// assert!(table.check(42).is_some());
    /// ```
    #[inline]
    pub fn check(&self, ip: usize) -> Option<&HotTraceEntry> {
        if ip < self.entries.len() {
            let entry = &self.entries[ip];

            // [修复] 移除 hit_count > 0 限制。
            // 原因：mark_hot() 文档明确说明手动标记后 check() 应能命中，
            // 但手动标记时 hit_count 可能为 0，原逻辑会导致手动标记失效。
            if entry.status == TraceStatus::Hot {
                return Some(entry);
            }
        }

        None
    }

    /// Check if IP is a hot trace start point (returns bool, no borrow conflict).
    ///
    /// This is a convenience wrapper around `check()` that avoids returning
    /// a reference, which can cause borrow checker issues when the caller
    /// needs to mutably borrow `self` afterwards.
    #[inline]
    #[must_use]
    pub fn is_hot_trace(&self, ip: usize) -> bool {
        if ip < self.entries.len() {
            let entry = &self.entries[ip];
            // Must have valid trace bounds — end_ip must be > ip (start position)
            // to avoid infinite empty-loop in execute_hot_trace_batch().
            return entry.status == TraceStatus::Hot && entry.end_ip > ip && entry.length > 0;
        }
        false
    }

    /// Get the end_ip for a hot trace at the given IP.
    ///
    /// Should only be called after `is_hot_trace()` returns true.
    #[inline]
    #[must_use]
    pub fn hot_trace_end(&self, ip: usize) -> usize {
        if ip < self.entries.len() {
            let end = self.entries[ip].end_ip;
            if end > ip {
                return end;
            }
        }
        ip + 1 // Fallback: advance by 1 instruction (no batching)
    }

    // ========================================================================
    // 采样：轻量级频率记录
    // ========================================================================

    /// 记录每个 IP 的执行频率（轻量级采样）
    ///
    /// 在 VM 主循环尾部调用，用于收集热点数据。
    ///
    /// # 工作流程
    ///
    /// 1. 递增全局采样计数器
    /// 2. 按需扩容 entries 数组（懒分配）
    /// 3. 递增该 IP 的 hit_count
    /// 4. 状态机转换:
    ///    - `Cold` -> `Warming(threshold)` 当 hit_count 达到阈值
    ///    - `Warming(n)` -> `Warming(n-1)` -> ... -> `Hot`
    ///    - `Hot` -> 保持 Hot，递增 total_hits
    ///
    /// # Parameters
    ///
    /// - `ip`: 当前执行的指令指针
    ///
    /// # Performance
    ///
    /// - 平均时间复杂度: **O(1)**（摊还分析）
    /// - 最坏情况: **O(n)** 当触发 resize_with 时（n = 新容量）
    /// - 但 resize_with 频率极低（仅在遇到新 IP 时）
    ///
    /// # Memory Allocation
    ///
    /// 仅在以下情况分配内存：
    /// - 首次遇到新 IP 且未达 max_traces 上限时
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_bytecode::Opcode;
    /// use nuzo_vm::vm_hot_trace::{HotTraceTable, TraceStatus};
    ///
    /// let mut table = HotTraceTable::new();
    ///
    /// // 模拟足够多次执行，使 IP 10 进入 Hot 状态
    /// for _ in 0..1001 {
    ///     table.profile(10, Opcode::LoadK);
    /// }
    ///
    /// let entry = table.check(10).expect("should be hot");
    /// assert_eq!(entry.hit_count(), 1001);
    /// assert!(matches!(entry.status(), TraceStatus::Hot));
    /// ```
    #[inline(always)]
    pub fn profile(&mut self, ip: usize, opcode: Opcode) {
        // ====================================================================
        // 快速路径：已存在的 Cold 条目，且不会触发状态转换
        // ====================================================================
        //
        // 这是绝大多数指令的执行路径。在程序运行期间，每条指令会被 profile
        // 数百次（直到达到 warming_threshold），其中绝大多数都处于 Cold 状态
        // 且不会触发状态转换。此路径仅包含边界检查 + 状态比较 + 两次递增。
        //
        // 快速路径条件：
        // 1. ip < entries.len() — 条目已存在，无需扩容
        // 2. hit_count > 0 — 非新条目，无需更新 active_traces
        // 3. status == Cold — 无需相邻对统计、无需状态机转换
        // 4. hit_count + 1 < warming_threshold — 递增后不会触发 Cold→Warming 转换
        if ip < self.entries.len() {
            let entry = &mut self.entries[ip];
            if entry.status == TraceStatus::Cold
                && entry.hit_count > 0
                && entry.hit_count + 1 < self.config.warming_threshold
            {
                entry.hit_count += 1;
                self.total_profiled += 1;
                return;
            }
        }

        // 慢速路径：处理新 IP、扩容、状态转换、相邻对统计等
        self.profile_slow_path(ip, opcode);
    }

    /// profile() 的慢速路径：处理新 IP、扩容、状态转换、相邻对统计等复杂情况。
    ///
    /// 标记为 `#[cold]` + `#[inline(never)]` 以确保编译器：
    /// 1. 不会将此函数内联到调用点，保持快速路径精简
    /// 2. 将此函数的机器码放到 .text.unlikely 段，减少 i-cache 污染
    #[cold]
    #[inline(never)]
    fn profile_slow_path(&mut self, ip: usize, opcode: Opcode) {
        self.total_profiled += 1;

        let is_new = self.entries.get(ip).is_none_or(|e| e.hit_count == 0);

        // 资源保护：达到上限后不再记录新 IP
        if is_new && self.active_traces >= self.config.max_traces {
            return;
        }

        // 按需扩容：懒分配策略
        if ip >= self.entries.len() {
            self.entries.resize_with(ip + 1, HotTraceEntry::default);
        }

        // 安全访问：上面已保证 ip < len
        let entry = &mut self.entries[ip];

        // 确保 start_ip 正确（防御性编程）
        entry.start_ip = ip;

        if is_new {
            self.active_traces += 1;
        }

        entry.hit_count += 1;

        // ====================================================================
        // Superinstruction 融合优化：Opcode 相邻对统计
        // ====================================================================
        //
        // 设计决策：仅在 Hot/Warming 状态下记录相邻对
        // - Cold 状态下的执行序列通常是"一次性"代码，不值得融合优化
        // - Warming/Hot 状态表示此 IP 已被多次执行，其相邻模式具有代表性
        // - 减少 XxHashMap 写入次数，降低对 Cold 路径的性能影响
        if matches!(entry.status, TraceStatus::Hot | TraceStatus::Warming(_)) {
            if let Some(prev_opcode) = self.last_opcode {
                let prev_disc = std::mem::discriminant(&prev_opcode);
                let curr_disc = std::mem::discriminant(&opcode);
                *self.pair_counter.entry((prev_disc, curr_disc)).or_insert(0) += 1;
            }
            self.last_opcode = Some(opcode);
        }

        // 状态机驱动
        match entry.status {
            TraceStatus::Cold => {
                if entry.hit_count >= self.config.warming_threshold {
                    entry.status = TraceStatus::Warming(self.config.warming_threshold);
                }
            }
            TraceStatus::Warming(ref mut remaining) => {
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    entry.status = TraceStatus::Hot;
                }
            }
            TraceStatus::Hot => {
                self.total_hits += 1;
            }
        }
    }

    // ========================================================================
    // FNV-1a 哈希计算
    // ========================================================================

    /// 计算字节码序列的 FNV-1a 指纹
    ///
    /// FNV-1a 是一种非加密哈希算法，特点是：
    /// - **极快**：仅需 XOR + MUL 两个操作
    /// - **低碰撞**：对于短序列（<=16 字节）分布均匀
    /// - **无状态**：不需要维护内部状态，适合内联调用
    ///
    /// # Algorithm
    ///
    /// hash = FNV_OFFSET_BASIS (0xcbf29ce484222325)
    /// for each byte in sequence:
    ///     hash = hash XOR byte
    ///     hash = hash * FNV_PRIME (0x00000100000001b3)
    /// return hash
    ///
    /// # Parameters
    ///
    /// - `code`: 完整字节码切片
    /// - `start`: 序列起始偏移量
    /// - `len`: 序列长度（字节数）
    ///
    /// # Returns
    ///
    /// 64 位 FNV-1a 哈希值
    ///
    /// # Safety
    ///
    /// 自动处理边界条件：如果 start + len 超出 code.len()，
    /// 则只计算到末尾。
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let bytecode = [0x01, 0x02, 0x03, 0x04, 0x05];
    /// let hash = HotTraceTable::compute_sequence_hash(&bytecode, 1, 3);
    /// // 计算 [0x02, 0x03, 0x04] 的哈希
    /// assert_ne!(hash, 0); // 几乎不可能为 0
    /// ```
    #[inline]
    pub fn compute_sequence_hash(code: &[u8], start: usize, len: usize) -> u64 {
        let mut hash: u64 = FNV_OFFSET_BASIS_64;

        // 边界安全：防止越界访问
        let end = std::cmp::min(start + len, code.len());

        for &byte in &code[start..end] {
            hash ^= byte as u64;

            // 乘以 FNV prime（使用 wrapping_mul 避免溢出 panic）
            hash = hash.wrapping_mul(FNV_PRIME_64);
        }

        hash
    }

    // ========================================================================
    // 管理接口
    // ========================================================================

    /// 手动标记某个 IP 为热路径
    ///
    /// 通常由 VM 在检测到循环结构时主动调用。
    /// 调用后，该 IP 可被 `check()` 命中。
    ///
    /// # Parameters
    ///
    /// - `ip`: 热路径起始 IP
    /// - `hash`: 该序列的 FNV-1a 指纹（由 compute_sequence_hash 计算）
    /// - `length`: 序列长度（指令数，<= max_trace_length）
    /// - `end_ip`: 序列终止 IP
    ///
    /// # Side Effects
    ///
    /// 1. 更新 entry 的 hash/length/end_ip/status 字段
    /// 2. 在 hash_index 中插入映射关系
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_bytecode::Opcode;
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let mut table = HotTraceTable::new();
    ///
    /// // 先让系统知道这个 IP 存在
    /// table.profile(100, Opcode::LoadK);
    ///
    /// // 标记为热路径
    /// table.mark_hot(100, 0xDEAD_BEEF_CAFE_BABE, 12, 112);
    ///
    /// // 现在可以被 check() 命中
    /// assert!(table.check(100).is_some());
    /// ```
    pub fn mark_hot(&mut self, ip: usize, hash: u64, length: u8, end_ip: usize) {
        if ip < self.entries.len() {
            let entry = &mut self.entries[ip];

            entry.hash = hash;
            entry.length = length;
            entry.end_ip = end_ip;

            entry.status = TraceStatus::Hot;

            // 建立倒排索引：hash -> ip
            //
            // 注意：如果同一 hash 已存在，会覆盖旧值。
            // 这是可接受的，因为：
            // - 哈希碰撞概率极低（FNV-1a 对于短序列）
            // - 即使碰撞，最坏情况只是错误地批量执行了错误的序列
            //   （仍能正确执行，只是没获得加速效果）
            self.hash_index.insert(hash, ip);
        }
    }

    /// Try to detect and register a hot trace at a specific IP.
    ///
    /// This method encapsulates the loop detection logic to avoid exposing
    /// private fields to VM. It checks if the IP has enough hits and is not
    /// already hot, then scans forward to estimate the loop body length.
    ///
    /// # Arguments
    ///
    /// * `code` - Reference to the bytecode array
    /// * `ip` - The instruction pointer to check/register
    pub fn try_register_at_ip(&mut self, code: &[u8], ip: usize) {
        // Check if entry exists and meets threshold
        if ip >= self.entries.len() {
            return;
        }

        let trace = &self.entries[ip];
        if trace.hit_count < self.config.warming_threshold {
            return; // Not enough samples yet
        }
        if trace.status == TraceStatus::Hot {
            return; // Already hot
        }

        // Scan forward to estimate loop body length.
        // **Critical**: stop at control-flow opcodes so the hot trace never
        // includes a back-edge (Jmp/Test) that would cause infinite looping.
        let start_ip = ip;
        let mut length: u8 = 0;
        let mut pos = start_ip;

        while pos < code.len() && length < self.config.max_trace_length {
            // Decode the opcode at current position
            let opcode_byte = code[pos];
            let Some(opcode) = Chunk::decode_opcode(opcode_byte) else {
                break; // Unknown opcode, stop scanning
            };

            // Stop BEFORE any control-flow instruction — do NOT include it
            // in the trace. This ensures end_ip lands before the jump-back.
            if matches!(
                opcode,
                Opcode::Jmp | Opcode::Test | Opcode::Return | Opcode::Halt | Opcode::Call
            ) {
                break;
            }

            // Advance by this instruction's actual size (not just 1 byte)
            let instr_size = opcode.instruction_size();
            pos += instr_size;
            length += 1;
        }

        // Batch 最小指令数阈值：极短 trace 的 batch 固定开销
        // （函数调用 + while 循环 + 融合检查）会超过节省的 dispatch
        // 次数，导致净性能退化。仅当指令数 >= min_batch_length 时
        // 才标记为 Hot，否则保持 Warming 状态，回退到逐条 dispatch。
        if length >= self.config.min_batch_length {
            let hash = Self::compute_sequence_hash(code, start_ip, pos - start_ip);
            self.mark_hot(start_ip, hash, length, pos);
        }
    }

    /// 获取统计信息（调试和监控用）
    ///
    /// # Returns
    ///
    /// 元组包含：
    /// 0. `total_profiled`: 总采样次数
    /// 1. `total_hits`: 总命中次数（check() 成功次数）
    /// 2. `entries_len`: 当前管理的 IP 数量
    /// 3. `hot_count`: 处于 Hot 状态的条目数
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_bytecode::Opcode;
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let mut table = HotTraceTable::new();
    ///
    /// // 采样 1000 次
    /// for _ in 0..1000 { table.profile(0, Opcode::LoadK); }
    ///
    /// let (profiled, hits, len, hot) = table.stats();
    /// assert_eq!(profiled, 1000);
    /// assert_eq!(hits, 0); // 还没有 Hot 条目
    /// assert!(len > 0);
    /// ```
    pub fn stats(&self) -> (u64, u64, usize, usize) {
        let hot_count = self.entries.iter().filter(|e| e.status == TraceStatus::Hot).count();

        (self.total_profiled, self.total_hits, self.entries.len(), hot_count)
    }

    /// 获取配置引用（只读）
    pub fn config(&self) -> &HotTraceConfig {
        &self.config
    }

    /// 通过哈希值查找对应的 IP（如果有）
    ///
    /// # Returns
    ///
    /// - `Some(ip)` 如果该哈希对应的热路径存在
    /// - `None` 如果未找到
    #[inline]
    #[must_use]
    pub fn find_by_hash(&self, hash: u64) -> Option<usize> {
        self.hash_index.get(&hash).copied()
    }

    // ========================================================================
    // Superinstruction 融合优化：相邻对查询接口
    // ========================================================================

    /// 返回 Top-N 高频 Opcode 相邻对（按频率降序排列）
    ///
    /// 此方法是 Superinstruction 融合优化的核心数据源。
    /// 编译器/运行时可根据返回的高频相邻对，选择性地生成 fused handler，
    /// 从而消除重复的指令分发开销（dispatch overhead）。
    ///
    /// # Parameters
    ///
    /// - `n`: 返回的最大相邻对数量（建议值：5~20）
    ///
    /// # Returns
    ///
    /// Vec<((Opcode, Opcode), u32)> — 按频率降序排列的相邻对列表：
    /// - 元组第一项: (前一条 Opcode, 当前 Opcode) 有序对
    /// - 元组第二项: 该相邻对的出现次数
    ///
    /// # Performance
    ///
    /// - 时间复杂度: O(P log P) 其中 P = pair_counter.len()（排序开销）
    /// - 空间复杂度: O(min(n, P))（结果集大小）
    /// - 典型场景: P < 100, n < 20 → 排序开销可忽略不计
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nuzo_bytecode::Opcode;
    /// use nuzo_vm::vm_hot_trace::HotTraceTable;
    ///
    /// let mut table = HotTraceTable::new();
    ///
    /// // 先让 IP 0 进入 Warming 状态并建立 last_opcode
    /// for _ in 0..501 {
    ///     table.profile(0, Opcode::LoadK);
    /// }
    ///
    /// // 继续采样以产生相邻对
    /// table.profile(0, Opcode::Add);
    /// table.profile(0, Opcode::LoadK);
    /// table.profile(0, Opcode::Add);
    ///
    /// let top3 = table.top_pairs(3);
    /// assert_eq!(top3[0], ((Opcode::LoadK, Opcode::Add), 2));  // 最高频
    /// assert_eq!(top3.len(), 2);  // 实际只有 2 个唯一相邻对
    /// ```
    #[inline]
    #[must_use]
    pub fn top_pairs(&self, n: usize) -> Vec<((Opcode, Opcode), u32)> {
        // 预构建 Discriminant → Opcode 反向映射（O(Opcode_COUNT) 一次性开销）
        //
        // Why?
        // - pair_counter 使用 Discriminant 作为 key（避免 Hash trait 约束）
        // - 但用户需要的是 (Opcode, Opcode) 格式的结果
        // - 通过预构建映射表，将 O(P * O) 查找降为 O(P + O)
        //   其中 P = pair_counter.len(), O = Opcode::ALL.len()
        let disc_to_op: XxHashMap<_, _> =
            Opcode::ALL.iter().map(|op| (std::mem::discriminant(op), *op)).collect();

        let mut pairs: Vec<_> = self
            .pair_counter
            .iter()
            .filter_map(|(&(prev_disc, curr_disc), &count)| {
                // 通过反向映射表查找对应的 Opcode
                // 安全性：Discriminant 来源于合法的 Opcode，因此一定能找到
                let prev_op = disc_to_op.get(&prev_disc).copied()?;
                let curr_op = disc_to_op.get(&curr_disc).copied()?;
                Some(((prev_op, curr_op), count))
            })
            .collect();

        // 按频率降序排序（高频在前）
        //
        // Why Reverse?
        // - 用户通常最关心 Top-1 最高频相邻对（用于优先融合）
        // - 降序排列可直接 .take(n) 获取最高频的 N 个
        pairs.sort_by_key(|&(_, count)| std::cmp::Reverse(count));

        // 截取 Top-N 并返回
        pairs.into_iter().take(n).collect()
    }

    /// 清除所有数据（重置为初始状态）
    ///
    /// 用于测试或 VM 重置场景。
    ///
    /// # Superinstruction 统计器重置
    ///
    /// 除了清除基础字段外，还会：
    /// - 清空 pair_counter（相邻对频率数据）
    /// - 重置 last_opcode 为 None（重新开始相邻对追踪）
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hash_index.clear();
        self.total_profiled = 0;
        self.total_hits = 0;
        self.active_traces = 0;
        self.events.clear();
        // 重置 Superinstruction 统计器
        self.pair_counter.clear();
        self.last_opcode = None;
    }

    // ========================================================================
    // µPIC Cache 失效联动
    // ========================================================================

    /// 获取指定 IP 的融合入口
    ///
    /// 当 VM 主循环检测到 Hot 状态时调用此方法，
    /// 查询是否存在对应的融合微操作序列。
    ///
    /// # 当前实现
    ///
    /// 始终返回 `None`（保守策略，不执行融合路径）。
    /// 未来当 JIT/AOT 编译器生成 `FusedLoopEntry` 后，
    /// 此方法将返回对应的融合入口，启用超级指令执行路径。
    ///
    /// # Parameters
    ///
    /// - `ip`: 当前指令指针
    ///
    /// # Returns
    ///
    /// - `Some(&FusedLoopEntry)` 如果存在融合入口
    /// - `None` 否则（当前始终返回 None）
    #[inline]
    pub fn get_fused_entry(&self, _ip: usize) -> Option<&FusedLoopEntry> {
        // 当前实现：保守策略，始终返回 None
        // 未来：查询 fused_entries XxHashMap 或 Vec
        None
    }

    /// 标记所有依赖指定 shape 的融合缓存为 stale
    ///
    /// 当 µPIC 检测到 shape 变化时调用，清除 pair_counter 和 last_opcode，
    /// 强制 HotTrace 在下一轮 profile() 中重新学习 opcode 相邻模式。
    ///
    /// **设计决策**：当前采用简单策略——全局清除相邻对统计。
    /// 未来精细化方案：追踪每个 fused entry 依赖的 shape 集合，仅清除受影响条目。
    ///
    /// # Parameters
    ///
    /// * `_shape_id` - 发生变化的 shape 标识符（预留参数，供未来精细化失效使用）
    pub fn invalidate_fused_cache_for_shape(&mut self, _shape_id: usize) {
        self.pair_counter.clear();
        self.last_opcode = None;
    }

    /// CIGC 缓存失效时通知 HotTrace
    ///
    /// 当 CIGC miss 导致缓存更新时调用，清除融合缓存。
    /// 当前采用保守策略：清除所有融合缓存。
    /// 未来精细化方案：追踪每个 fused entry 依赖的 name_idx 集合，仅清除受影响条目。
    ///
    /// # Parameters
    ///
    /// * `_name_idx` - 发生 CIGC miss 的常量池索引（预留参数，供未来精细化失效使用）
    pub fn invalidate_fused_cache_for_cigc(&mut self, _name_idx: usize) {
        self.pair_counter.clear();
        self.last_opcode = None;
    }

    /// CSTS 缓存失效时通知 HotTrace
    ///
    /// 当 CSTS 快照被填充或更新时调用，清除融合缓存。
    /// 当前采用保守策略：清除所有融合缓存。
    /// 未来精细化方案：追踪每个 fused entry 依赖的 call_site 集合，仅清除受影响条目。
    ///
    /// # Parameters
    ///
    /// * `_call_site_idx` - 发生 CSTS 更新的调用点索引（预留参数，供未来精细化失效使用）
    pub fn invalidate_fused_cache_for_csts(&mut self, _call_site_idx: usize) {
        self.pair_counter.clear();
        self.last_opcode = None;
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // 修复：原 `use std::assert_matches;` 引用 unstable feature (#82775)，
    // 阻断 test 编译。改用 stable 的 `matches!` 宏（Rust 1.42+）+ `assert!`
    // 等价表达，避免依赖未稳定的 `assert_matches!` 宏。
    // 这是预先存在的 broken import，本次为 S1-S4 回归测试取数被迫顺手修复。

    #[test]
    fn test_default_config() {
        let config = HotTraceConfig::default();
        assert_eq!(config.warming_threshold, 500);
        assert_eq!(config.hot_threshold, 2000);
        assert_eq!(config.max_trace_length, 16);
        assert_eq!(config.max_traces, 128);
    }

    #[test]
    fn test_new_table() {
        let table = HotTraceTable::new();
        let (_, _, len, hot) = table.stats();
        assert_eq!(len, 0);
        assert_eq!(hot, 0);
    }

    #[test]
    fn test_custom_config() {
        let config = HotTraceConfig {
            warming_threshold: 100,
            hot_threshold: 500,
            max_trace_length: 8,
            max_traces: 32,
            min_batch_length: 5,
        };
        let table = HotTraceTable::with_config(config);
        assert_eq!(table.config().warming_threshold, 100);
    }

    // ========================================================================
    // min_batch_length 阈值测试
    // ========================================================================

    /// 辅助函数：构造一段由 LoadK 组成的字节码序列，后跟一个 Jmp。
    /// LoadK 的 instruction_size = 5（opcode + u16 dest + u16 const_idx）。
    /// Jmp 的 instruction_size = 3（opcode + i16 offset）。
    fn make_bytecode(loadk_count: usize) -> Vec<u8> {
        let mut code = Vec::new();
        // LoadK opcode = 1 (假设)，每个 LoadK 占 5 字节
        for _ in 0..loadk_count {
            code.push(1); // LoadK opcode
            code.extend_from_slice(&0u16.to_le_bytes()); // dest
            code.extend_from_slice(&0u16.to_le_bytes()); // const_idx
        }
        code.push(15); // Jmp opcode (假设)
        code.extend_from_slice(&(-10i16).to_le_bytes()); // backward offset
        code
    }

    #[test]
    fn test_min_batch_length_default() {
        let config = HotTraceConfig::default();
        assert_eq!(config.min_batch_length, 5);
    }

    #[test]
    fn test_short_trace_skipped() {
        // 3 条 LoadK 指令 -> length=3 < min_batch_length=5 -> 不应标记为 Hot
        let code = make_bytecode(3);
        let config = HotTraceConfig {
            warming_threshold: 10,
            hot_threshold: 20,
            max_trace_length: 16,
            max_traces: 128,
            min_batch_length: 5,
        };
        let mut table = HotTraceTable::with_config(config);

        // 预热：先让 IP 0 的 hit_count 达到 warming_threshold
        for _ in 0..15 {
            table.profile(0, Opcode::LoadK);
        }

        // 尝试注册 hot trace
        table.try_register_at_ip(&code, 0);

        // 不应该被标记为 Hot（因为 length=3 < 5）
        assert!(!table.is_hot_trace(0), "trace with 3 instructions should not be marked hot");
    }

    #[test]
    fn test_threshold_trace_accepted() {
        // 5 条 LoadK 指令 -> length=5 >= min_batch_length=5 -> 应标记为 Hot
        let code = make_bytecode(5);
        let config = HotTraceConfig {
            warming_threshold: 10,
            hot_threshold: 20,
            max_trace_length: 16,
            max_traces: 128,
            min_batch_length: 5,
        };
        let mut table = HotTraceTable::with_config(config);

        for _ in 0..15 {
            table.profile(0, Opcode::LoadK);
        }

        table.try_register_at_ip(&code, 0);

        assert!(table.is_hot_trace(0), "trace with 5 instructions should be marked hot");
    }

    #[test]
    fn test_long_trace_accepted() {
        // 6 条 LoadK 指令 -> length=6 >= 5 -> 应标记为 Hot
        let code = make_bytecode(6);
        let config = HotTraceConfig {
            warming_threshold: 10,
            hot_threshold: 20,
            max_trace_length: 16,
            max_traces: 128,
            min_batch_length: 5,
        };
        let mut table = HotTraceTable::with_config(config);

        for _ in 0..15 {
            table.profile(0, Opcode::LoadK);
        }

        table.try_register_at_ip(&code, 0);

        assert!(table.is_hot_trace(0), "trace with 6 instructions should be marked hot");
    }

    #[test]
    fn test_empty_trace_skipped() {
        // 0 条 LoadK -> length=0 < 5 -> 不应标记
        let code = make_bytecode(0);
        let config = HotTraceConfig {
            warming_threshold: 10,
            hot_threshold: 20,
            max_trace_length: 16,
            max_traces: 128,
            min_batch_length: 5,
        };
        let mut table = HotTraceTable::with_config(config);

        for _ in 0..15 {
            table.profile(0, Opcode::LoadK);
        }

        table.try_register_at_ip(&code, 0);

        assert!(!table.is_hot_trace(0), "empty trace should not be marked hot");
    }

    #[test]
    fn test_check_cold_path() {
        let mut table = HotTraceTable::new();

        // 未初始化的 IP 返回 None
        assert!(table.check(999).is_none());

        // 已 profile 但未 mark_hot 的 IP 也返回 None
        table.profile(42, Opcode::LoadK);
        assert!(table.check(42).is_none());
    }

    #[test]
    fn test_check_hot_path() {
        let mut table = HotTraceTable::new();

        // 先注册 IP
        table.profile(100, Opcode::LoadK);

        // 标记为热路径
        table.mark_hot(100, 0xABCDEF, 8, 108);

        // 现在 check 应该命中
        let entry = table.check(100).expect("Should hit hot trace");
        assert_eq!(entry.start_ip(), 100);
        assert_eq!(entry.end_ip(), 108);
        assert_eq!(entry.length(), 8);
        assert_eq!(entry.hash(), 0xABCDEF);
    }

    #[test]
    fn test_profile_cold_to_warming() {
        let mut table = HotTraceTable::new();

        // 执行 499 次（刚好低于阈值）
        for _ in 0..499 {
            table.profile(10, Opcode::Add);
        }

        let entry = &table.entries[10];
        assert_eq!(entry.hit_count, 499);
        assert!(matches!(entry.status, TraceStatus::Cold));

        // 第 500 次触发转换
        table.profile(10, Opcode::Add);
        assert!(matches!(table.entries[10].status, TraceStatus::Warming(_)));
    }

    #[test]
    fn test_profile_warming_to_hot() {
        let config = HotTraceConfig {
            warming_threshold: 3, // 小阈值便于测试
            ..Default::default()
        };
        let mut table = HotTraceTable::with_config(config);

        // 执行恰好 3 次触发 Cold -> Warming 转换
        // 第 3 次时 hit_count=3 达到阈值，状态变为 Warming(3)
        for _ in 0..3 {
            table.profile(20, Opcode::LoadK);
        }

        // 现在应该在 Warming 状态，remaining 应该为 3
        if let TraceStatus::Warming(remaining) = table.entries[20].status {
            assert_eq!(remaining, 3); // 初始值为 warming_threshold
        } else {
            panic!("Expected Warming state");
        }

        // 继续采样 3 次：3 -> 2 -> 1 -> 0 (Hot)
        for _ in 0..3 {
            table.profile(20, Opcode::LoadK);
        }

        assert_eq!(table.entries[20].status, TraceStatus::Hot);
    }

    #[test]
    fn test_profile_hot_increments_hits() {
        let mut table = HotTraceTable::new();

        // 先注册 IP（让 entries 包含这个位置）
        table.profile(50, Opcode::LoadK);

        // 然后标记为热路径
        table.mark_hot(50, 0, 4, 54);

        // 再执行 10 次
        for _ in 0..10 {
            table.profile(50, Opcode::Add);
        }

        let (_, hits, _, _) = table.stats();
        assert_eq!(hits, 10);
    }

    #[test]
    fn test_max_traces_limit() {
        let config = HotTraceConfig {
            max_traces: 2, // 极小限制
            ..Default::default()
        };
        let mut table = HotTraceTable::with_config(config);

        // 注册 2 个 IP（达到上限）
        table.profile(0, Opcode::LoadK);
        table.profile(1, Opcode::Add);
        // 第 3 个 IP 应该被忽略
        table.profile(2, Opcode::Mul);

        let (_, _, len, _) = table.stats();
        assert_eq!(len, 2); // 仍然只有 2 个
    }

    #[test]
    fn test_compute_sequence_hash() {
        let code = [0x01, 0x02, 0x03, 0x04, 0x05];

        // 测试不同范围
        let h1 = HotTraceTable::compute_sequence_hash(&code, 0, 5);
        let h2 = HotTraceTable::compute_sequence_hash(&code, 1, 3);
        let h3 = HotTraceTable::compute_sequence_hash(&code, 0, 5);

        // 相同输入应产生相同输出
        assert_eq!(h1, h3);

        // 不同输入应产生不同输出（大概率）
        assert_ne!(h1, h2);

        // 测试越界保护
        let h4 = HotTraceTable::compute_sequence_hash(&code, 3, 100); // 超出长度
        let h5 = HotTraceTable::compute_sequence_hash(&code, 3, 2); // 实际有效部分
        assert_eq!(h4, h5); // 应该相等（都被截断）
    }

    #[test]
    fn test_mark_hot_updates_index() {
        let mut table = HotTraceTable::new();
        table.profile(77, Opcode::LoadK);

        let hash = 0xBEEF_CAFE_DEAD;
        table.mark_hot(77, hash, 10, 87);

        // 通过 hash 反查应该找到
        assert_eq!(table.find_by_hash(hash), Some(77));
    }

    #[test]
    fn test_stats_tracking() {
        let mut table = HotTraceTable::new();

        // 采样多个 IP
        for i in 0..100 {
            table.profile(i, Opcode::LoadK);
        }

        let (profiled, hits, len, hot) = table.stats();
        assert_eq!(profiled, 100);
        assert_eq!(hits, 0);
        assert_eq!(len, 100);
        assert_eq!(hot, 0);
    }

    #[test]
    fn test_clear_resets_state() {
        let mut table = HotTraceTable::new();

        // 填充一些数据
        table.profile(0, Opcode::LoadK);
        table.profile(1, Opcode::Add);
        table.mark_hot(0, 123, 4, 4);

        // 清除
        table.clear();

        let (_, _, len, hot) = table.stats();
        assert_eq!(len, 0);
        assert_eq!(hot, 0);
        assert!(table.check(0).is_none());
    }

    #[test]
    fn test_entry_default_values() {
        let entry = HotTraceEntry::default();
        assert_eq!(entry.hash, 0);
        assert_eq!(entry.start_ip, 0);
        assert_eq!(entry.length, 0);
        assert_eq!(entry.end_ip, 0);
        assert_eq!(entry.hit_count, 0);
        assert_eq!(entry.status, TraceStatus::Cold);
    }

    #[test]
    fn test_lazy_allocation() {
        let mut table = HotTraceTable::new();

        // 初始不应有分配
        assert_eq!(table.entries.len(), 0);

        // 第一次访问远距离 IP
        table.profile(1000, Opcode::LoadK);

        // 应该精确扩容到 1001
        assert_eq!(table.entries.len(), 1001);

        // 中间应该是默认值
        assert_eq!(table.entries[500].status, TraceStatus::Cold);
    }

    // ---- 新增测试：覆盖未测试的 pub fn ----

    #[test]
    fn test_hot_trace_entry_start_ip() {
        let mut table = HotTraceTable::new();
        table.profile(42, Opcode::LoadK);
        table.mark_hot(42, 0xABCD, 5, 47);
        let entry = table.check(42).expect("Should be hot");
        assert_eq!(entry.start_ip(), 42);
    }

    #[test]
    fn test_hot_trace_entry_end_ip() {
        let mut table = HotTraceTable::new();
        table.profile(10, Opcode::LoadK);
        table.mark_hot(10, 0, 4, 14);
        let entry = table.check(10).expect("Should be hot");
        assert_eq!(entry.end_ip(), 14);
    }

    #[test]
    fn test_hot_trace_entry_hit_count() {
        let mut table = HotTraceTable::new();
        table.profile(5, Opcode::LoadK);
        table.mark_hot(5, 0, 2, 7);
        // After mark_hot, profile increments hit_count
        for _ in 0..5 {
            table.profile(5, Opcode::Add);
        }
        let entry = table.check(5).expect("Should be hot");
        // hit_count includes the initial profile call before mark_hot
        assert!(entry.hit_count() >= 5);
    }

    #[test]
    fn test_hot_trace_entry_is_hot_true() {
        let mut table = HotTraceTable::new();
        table.profile(1, Opcode::LoadK);
        table.mark_hot(1, 0, 2, 3);
        let entry = table.check(1).expect("Should be hot");
        assert!(entry.is_hot());
    }

    #[test]
    fn test_find_by_hash_existing() {
        let mut table = HotTraceTable::new();
        table.profile(77, Opcode::LoadK);
        let hash = 0xDEAD_BEEF;
        table.mark_hot(77, hash, 4, 81);
        assert_eq!(table.find_by_hash(hash), Some(77));
    }

    #[test]
    fn test_find_by_hash_nonexistent() {
        let table = HotTraceTable::new();
        assert_eq!(table.find_by_hash(0xFFFF_FFFF), None);
    }

    #[test]
    fn test_get_fused_entry_returns_none() {
        let table = HotTraceTable::new();
        // Current implementation: conservative, always returns None
        assert!(table.get_fused_entry(0).is_none());
    }

    #[test]
    fn test_hot_trace_end_existing() {
        let mut table = HotTraceTable::new();
        table.profile(10, Opcode::LoadK);
        table.mark_hot(10, 0, 3, 13);
        assert_eq!(table.hot_trace_end(10), 13);
    }

    #[test]
    fn test_hot_trace_end_nonexistent() {
        let table = HotTraceTable::new();
        // IP not in table: returns ip+1 (fallback: advance by 1)
        assert_eq!(table.hot_trace_end(999), 1000);
    }

    #[test]
    fn test_invalidate_fused_cache_for_cigc_no_crash() {
        let mut table = HotTraceTable::new();
        table.profile(1, Opcode::LoadK);
        table.profile(2, Opcode::Add);
        table.invalidate_fused_cache_for_cigc(0);
        // Should not panic, pair_counter cleared
    }

    #[test]
    fn test_invalidate_fused_cache_for_csts_no_crash() {
        let mut table = HotTraceTable::new();
        table.profile(1, Opcode::LoadK);
        table.invalidate_fused_cache_for_csts(0);
    }

    #[test]
    fn test_invalidate_fused_cache_for_shape_no_crash() {
        let mut table = HotTraceTable::new();
        table.profile(1, Opcode::LoadK);
        table.invalidate_fused_cache_for_shape(0);
    }

    #[test]
    fn test_is_hot_trace_true() {
        let mut table = HotTraceTable::new();
        table.profile(5, Opcode::LoadK);
        table.mark_hot(5, 0, 2, 7);
        assert!(table.is_hot_trace(5));
    }

    #[test]
    fn test_is_hot_trace_false() {
        let mut table = HotTraceTable::new();
        table.profile(5, Opcode::LoadK);
        // Not marked hot yet
        assert!(!table.is_hot_trace(5));
    }

    #[test]
    fn test_top_pairs_basic() {
        let mut table = HotTraceTable::new();
        // Profile some pairs
        table.profile(0, Opcode::LoadK);
        table.profile(1, Opcode::Add);
        let pairs = table.top_pairs(10);
        // May be empty or contain pairs; just ensure no panic
        let _ = pairs.len();
    }

    #[test]
    fn test_try_register_at_ip_below_threshold() {
        let mut table = HotTraceTable::new();
        let code = [0x01, 0x02, 0x03];
        // Only 1 profile call, below warming_threshold (500)
        table.profile(0, Opcode::LoadK);
        table.try_register_at_ip(&code, 0);
        // Should not register as hot
        assert!(!table.is_hot_trace(0));
    }
}
