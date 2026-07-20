//! # 信号机制模块（核心实现）
//!
//! 本模块实现了**观察者模式（Observer Pattern）**的信号-槽（Signal-Slot）机制。
//!
//! ## 架构定位
//!
//! `Signal<Args>` 是整个系统的核心抽象，对应经典观察者模式中的 **Subject（主题/被观察者）**：
//! - **Signal**: Subject，维护观察者列表并负责通知广播
//! - **Slot (槽位)**: Observer，注册回调函数以响应信号变化
//! - **Connection**: 观察者与主题之间的绑定关系句柄
//!
//! ## 设计理念
//!
//! ### 1. 类型安全的泛型信号
//! 与 Qt 的元对象系统或 C++ 的模板不同，本实现利用 Rust 的泛型系统
//! 在编译期保证类型安全：`Signal<i32>` 只能发射 `i32` 数据，
//! 连接的槽位也必须接受 `&i32` 参数。任何类型不匹配都会在编译期报错。
//!
//! ### 2. RAII 式连接管理
//! 通过 `Connection` 对象实现自动断开：当 Connection 被 drop 时，
//! 可选择自动从信号的槽位列表中移除自己。这避免了忘记断开导致的内存泄漏和悬垂回调。
//!
//! ### 3. 故障隔离的发射机制
//! 使用 `panic::catch_unwind` 包裹每个槽位的执行，确保：
//! - 单个槽位的 panic 不会导致其他槽位无法执行
//! - 错误信息会被收集到 `EmitResult.errors` 中供调用者检查
//! - 可通过 `ErrorPolicy` 控制遇到错误时是否继续执行剩余槽位
//!
//! ### 4. 快照发射（Snapshot Emit）
//! emit 时先在读锁下克隆回调引用快照，然后释放锁再执行回调。
//! 这避免了长时间运行的回调阻塞 connect/disconnect 操作，
//! 显著提升了并发性能。
//!
//! ### 5. 递归发射保护
//! 通过每线程的 thread_local `EMITTING` 标志防止**同一线程**在槽位回调中
//! 再次触发同一信号的 emit 导致无限递归。检测到递归时立即返回带警告的 EmitResult，
//! 而非 panic 或死循环。**不同线程**的 emit 互不干扰（修复旧版 `Arc<AtomicBool>`
//! 误伤跨线程并发 emit 的 bug，详见 [`EMITTING`] 注释）。
//!
//! ## 线程安全性模型
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                  Signal<Args>                       │
//! │                                                     │
//! │  ┌──────────────┐    ┌──────────────┐               │
//! │  │ slots: Arc   │    │ next_id: Arc │               │
//! │  │ <RwLock<Vec>>│    │ <AtomicU64>  │               │
//! │  └──────┬───────┘    └──────────────┘               │
//! │         │                                            │
//! │  ┌──────┴───────┐    ┌──────────────┐               │
//! │  │ 并发访问策略  │    │ EMITTING:    │               │
//! │  │              │    │ thread_local │               │
//! │  │ emit(): 快照  │ ←  │ 递归保护     │               │
//! │  │ connect(): 写锁│ ←  仅同线程可见  │               │
//! │  │ disconnect(): 写锁│ ← 需要互斥访问 │               │
//! │  └──────────────┘    └──────────────┘               │
//! │                                                     │
//! │  Clone 行为: 浅拷贝（共享 slots, next_id, stats 等）│
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! ## 性能特征
//!
//! | 操作           | 时间复杂度 | 锁竞争     | 内存分配 |
//! |----------------|-----------|------------|----------|
//! | connect()      | O(n log n)| 写锁       | 1 次     |
//! | emit()         | O(m)      | 读锁(快照) | m 次*    |
//! | disconnect()   | O(n)      | 写锁       | 0 次     |
//! | clone_handle() | O(1)      | 无锁       | 0 次     |
//!
//! > *emit() 快照阶段为每个槽位克隆一次 Arc 回调引用（O(1) 原子操作）
//!
//! > n = 当前槽数量, m = 快照中的槽数量

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};
use web_time::Instant;

use crate::error::SignalError;
use crate::log::{LogEntry, LogEvent, SignalLog};
use crate::slot::{Connection, SlotEntry};
use crate::slot_stats::SlotStats;
use crate::types::{ConnectionId, EmitOptions, EmitResult, ErrorPolicy, Priority, SlotError};

/// Snapshot entry for emit: (id, callback, priority, once, connected_flag)
type SlotSnapshot<Args> =
    (ConnectionId, Arc<dyn Fn(&Args) + Send + Sync>, Priority, bool, Arc<AtomicBool>);

// ── 递归发射保护：每线程的 emit 状态标记 ──────────────────────────────
//
// 设计目标：
// - 用于防止同一线程在槽位回调中再次触发同一信号的 emit 导致无限递归
// - 不同线程的 emit 互不干扰（修复旧版 `Arc<AtomicBool>` 误伤跨线程并发 emit 的 bug）
//
// 为什么用 thread_local 而非 `Arc<AtomicBool>`？
// 旧版用 `emitting: Arc<AtomicBool>` 共享于所有 Signal 句柄与所有线程之间，
// 但 `swap(true, SeqCst)` 在跨线程并发 emit 时会误判为"递归"，
// 导致并发 emit 实际只有约 21% 成功（详见 bench_signal::bench_concurrent_emit）。
// 递归本质上是单线程语义（回调在同一调用栈中再次 emit），thread_local 完美匹配。
thread_local! {
    static EMITTING: Cell<bool> = const { Cell::new(false) };
}

/// RAII guard：emit 期间设置 EMITTING=true，drop 时恢复为 false。
///
/// # Panic 安全
/// 若 emit 期间发生 panic（即使被 catch_unwind 捕获，或未来新增的 unwrap 路径），
/// guard 的 Drop 仍会执行，确保 EMITTING 不被卡在 true 状态导致同线程后续 emit 误判为递归。
struct EmitGuard;

impl Drop for EmitGuard {
    fn drop(&mut self) {
        EMITTING.with(|f| f.set(false));
    }
}

/// 触发全局统计衰减的 emit 计数阈值
///
/// 每经过此数量次 emit 后，对所有 SlotStats 执行一次 `decay()`，
/// 让长期未调用的槽位逐渐降温。
const DECAY_INTERVAL: u64 = 1000;

/// SlotStats 批量更新间隔
///
/// 每 N 次 emit 才更新一次 SlotStats，分摊 100 次 record_call 的开销。
/// SlotStats 仅用于未来"热槽优先调度"的统计数据，非功能正确性依赖，
/// 周期性更新不影响语义，仅降低统计精度（可接受）。
/// 100 槽场景下省去 99% 的 record_call 开销（~600ns/emit → ~6ns/emit）。
const STATS_UPDATE_INTERVAL: u64 = 100;

/// 路径选择阈值：槽数 ≤ 此值时走栈上直接路径
const DIRECT_PATH_THRESHOLD: usize = 4;

/// 路径选择阈值：槽数 ≤ 此值时走快照路径；超过则走批量路径
const SNAPSHOT_PATH_THRESHOLD: usize = 64;

/// 泛型信号实现（观察者模式中的 Subject）
///
/// # 类型参数
/// - `Args`: 信号携带的数据载荷类型
///   - 必须是 `'static`（不能包含非静态引用）
///   - 必须实现 `Send + Sync`（支持跨线程传递）
///   - 典型示例：`i32`, `String`, `MyEventData`, `Vec<u8>`
///
/// # 观察者模式实现细节
///
/// ## 1. 注册阶段（connect）
/// 调用者提供回调闭包，系统将其包装为 `SlotEntry` 并插入有序列表。
/// 每次插入后都会重新排序（按优先级降序），保证发射顺序确定。
///
/// ## 2. 通知阶段（emit）
/// 当信号被触发时，按优先级顺序遍历所有已连接的槽位：
/// ```text
/// 信号发射
///     │
///     ▼
/// ┌──────────────┐
/// │ 检查递归标志  │ ──── 已在发射中 ──▶ 返回警告
/// └──────┬───────┘
///        │ 未在发射
///        ▼
/// ┌──────────────┐    成功    ┌──────────┐
/// │ 快照(读锁)   │ ────────▶ │ 释放锁    │
/// └──────────────┘            └────┬─────┘
///                                   │
///                         ┌─────────┴─────────┐
///                         │                   │
///                    正常返回             Panic
///                         │                   │
///                         ▼                   ▼
///                   invoked++          收集 SlotError
///                         │                   │
///                         │          ┌─────────┴─────────┐
///                         │          │ ErrorPolicy?       │
///                         │          │                   │
///                         │     Continue              Stop
///                         │          │                   │
///                         │          ▼                   ▼
///                         │    继续下一个          终止循环
///                         │
///                         ▼
///                   once? ──▶ 收集 ID 待移除
///                         │
///                         ▼
///                   移除 once 槽位(写锁)
///                         │
///                         ▼
///                   清除递归标志
/// ```
///
/// ## 3. 注销阶段（disconnect）
/// 通过 `Connection` 句柄或批量操作移除槽位。
/// 使用 RAII 模式确保资源正确释放。
///
/// # 内存布局
/// ```text
/// Signal<Args> {
///     name: &str,                          // 8 字节（指针）
///     slots: Arc<RwLock<Vec<SlotEntry>>>,  // 16 字节（Arc 指针）
///     next_id: Arc<AtomicU64>,             // 16 字节（Arc 指针）
///     stats: Arc<RwLock<Vec<SlotStats>>>, // 16 字节（Arc 指针）
///     emit_count: Arc<AtomicU64>,          // 16 字节（Arc 指针）
///     snapshot_cache: Arc<RwLock<Option<..>>>, // 16 字节
///     // 注：emitting 不再是结构体字段，改为 thread_local 静态变量 EMITTING
/// } // 总计 ~88 字节（64 位系统，不含 thread_local）
/// ```
///
/// # Clone 语义
/// `Signal` 实现了 `Clone`，但这是**浅拷贝**：
/// - 克隆后的实例共享相同的 `slots`、`next_id`、`stats`、`emit_count`、`snapshot_cache`
/// - 递归保护标志 `EMITTING` 是 thread_local，**所有 Signal 实例共享同一份**（但每线程独立）
/// - 对克隆体的 connect/disconnect/emit 操作会反映到原始实例上
/// - 用途：将同一个信号传递给多个模块而不需要 `Arc`
pub struct Signal<Args: 'static + Send + Sync> {
    /// 信号的唯一名称（用于日志、调试、总线注册）
    ///
    /// 使用 `'static` 生命周期确保字符串在程序运行期间始终有效。
    /// 通常使用字符串字面量（如 `"vm:execute"`）。
    name: &'static str,

    /// 已连接的槽位列表（线程安全共享状态）
    ///
    /// # 为什么用 Arc<RwLock<Vec>>？
    /// - **RwLock**: 允许多个并发 reader（emit 快照），但 writer（connect）独占
    /// - **Arc**: 支持多个 Signal 句柄共享同一组槽位（Clone 语义）
    /// - **Vec**: 提供连续内存布局和缓存友好的遍历性能
    ///
    /// # 排序不变量
    /// 列表始终按 `priority` 降序排列（高优先级在前）。
    /// 每次 insert 后立即排序，保证 emit 时无需额外排序开销。
    slots: Arc<RwLock<Vec<SlotEntry<Args>>>>,

    /// 下一个可用的连接 ID（全局原子计数器）
    ///
    /// # 为什么用 Arc<AtomicU64>？
    /// - **AtomicU64**: 无锁原子递增，高并发下性能优异
    /// - **Arc**: 与 slots 一起被 Clone 共享
    ///
    /// # ID 分配策略
    /// 从 1 开始递增（0 保留为特殊值表示"无效 ID"）。
    /// 理论上不会溢出（u64 最大值约为 1.8e19）。
    next_id: Arc<AtomicU64>,

    /// ASB: 每个槽位的热度统计（与 `slots` 一一对应，同序）
    ///
    /// # 设计说明
    /// - 与 `slots` 共享同一把 `RwLock` 之外的独立锁，避免污染快照路径
    /// - 在 connect/disconnect 时同步增删，保持与 slots 的一致性
    /// - emit 时按路径更新：>64 槽路径批量更新，其他路径惰性跳过
    ///
    /// # 性能权衡
    /// 此字段为未来"热槽优先调度"提供数据基础，当前实现中
    /// 仅在 >64 槽批量路径下更新，避免污染热路径。
    stats: Arc<RwLock<Vec<SlotStats>>>,

    /// ASB: 全局 emit 计数器，用于触发定期统计衰减
    ///
    /// 每经过 [`DECAY_INTERVAL`] 次 emit 后，对所有 SlotStats 执行一次 `decay()`。
    /// 使用 `Relaxed` 序：仅需计数，不参与同步。
    ///
    /// 修复（P2 BUG-signal-emit_count-clone）：旧版为 `AtomicU64`，clone_handle 时
    /// 复制独立计数器，导致克隆体与原信号衰减/统计脱节。改为 `Arc<AtomicU64>`
    /// 共享同一计数器，符合 clone_handle 文档承诺"克隆体与原实例共享状态"。
    emit_count: Arc<AtomicU64>,

    /// 快照缓存：emit 时优先复用，避免每次重建 100 元素快照
    ///
    /// # 缓存内容
    /// `Arc<[SlotSnapshot<Args>]>`——一次性克隆整个快照数组（O(1) 原子操作），
    /// 替代逐元素 `Arc::clone`（O(n)）。100 槽场景下将快照构建从 ~200 次 Arc 原子操作
    /// 降为 1 次，是 S4 性能提升的核心。
    ///
    /// # 缓存失效时机
    /// - `connect_internal`: 新槽位插入后
    /// - `disconnect_fn` 闭包: 单个槽位断开后
    /// - `disconnect_all` / `disconnect_by_group`: 批量断开后
    /// - `cleanup_once_slots`: once 槽位移除后
    ///
    /// # 并发安全
    /// 缓存命中时 `Arc::clone` 是 O(1) 原子操作，不阻塞 connect/disconnect。
    /// 缓存 miss 时用 `try_write` 避免阻塞，失败则跳过缓存（下次 emit 再尝试）。
    /// connected flag 仍在执行循环里实时 `load(Acquire)`，缓存的是"快照列表"而非"连接状态"。
    #[allow(clippy::type_complexity)]
    snapshot_cache: Arc<RwLock<Option<Arc<[SlotSnapshot<Args>]>>>>,
}

impl<Args: 'static + Send + Sync> Signal<Args> {
    /// 创建一个命名的新信号实例
    ///
    /// # 参数
    /// `name`: 信号的唯一标识符（建议使用 `"module:event"` 格式）
    ///
    /// # 返回值
    /// 一个空的信号实例（无已连接的槽位）
    ///
    /// # 命名规范建议
    /// 采用反向域名风格或层级命名：
    /// - `"vm:execute"` - VM 指令执行事件
    /// - `"gc:will_collect"` - GC 即将开始
    /// - `"ui:button:click"` - UI 按钮点击
    /// - `"net:request:complete"` - 网络请求完成
    ///
    /// # 示例
    /// ```ignore
    /// let data_changed = Signal::<DataPayload>::named("app:data_changed");
    /// let user_login = Signal::<UserInfo>::named("auth:user_login");
    /// ```
    pub fn named(name: &'static str) -> Self {
        Self {
            name,
            slots: Arc::new(RwLock::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            stats: Arc::new(RwLock::new(Vec::new())),
            emit_count: Arc::new(AtomicU64::new(0)),
            snapshot_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// 连接一个标准优先级的槽位
    ///
    /// # 这是便捷方法
    /// 等价于 `connect_with_priority(slot, Priority::Normal)`
    ///
    /// # 参数
    /// `slot`: 接收 `&Args` 参数的回调闭包
    ///   - 必须实现 `Send + Sync`（可能在不同线程执行）
    ///   - 必须是 `'static`（不能捕获局部变量引用）
    ///
    /// # 返回值
    /// - `Ok(Connection<Args>)`: 连接成功，返回句柄
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 所有权模型
    /// 闭包的所有权转移给信号内部，调用者不再持有它。
    /// 如果需要在多处使用同一逻辑，请使用闭包工厂或函数指针。
    ///
    /// # 示例
    /// ```ignore
    /// let conn = signal.connect(|data| {
    ///     println!("收到数据: {:?}", data);
    /// })?;
    /// // conn.disconnect() 可随时断开
    /// ```
    pub fn connect<F: Fn(&Args) + Send + Sync + 'static>(
        &self,
        slot: F,
    ) -> Result<Connection<Args>, SignalError> {
        self.connect_internal(slot, Priority::Normal, None, false)
    }

    /// 连接一个带自定义优先级的槽位
    ///
    /// # 何时需要指定优先级？
    /// - 认证/验证逻辑应在业务逻辑之前执行（High）
    /// - 日志/统计应在所有处理完成后执行（Low）
    /// - 大多数情况使用默认 Normal 即可
    ///
    /// # 优先级排序保证
    /// 高优先级的槽位总是先于低优先级槽位执行。
    /// 相同优先级的槽位按连接顺序执行（FIFO）。
    ///
    /// # 返回值
    /// - `Ok(Connection<Args>)`: 连接成功
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 示例
    /// ```ignore
    /// // 验证器最先执行
    /// signal.connect_with_priority(|data| validate(data), Priority::High(0))?;
    /// // 业务逻辑正常执行
    /// signal.connect(|data| process(data))?;
    /// // 日志记录最后执行
    /// signal.connect_with_priority(|data| log(data), Priority::Low(0))?;
    /// ```
    pub fn connect_with_priority<F: Fn(&Args) + Send + Sync + 'static>(
        &self,
        slot: F,
        priority: Priority,
    ) -> Result<Connection<Args>, SignalError> {
        self.connect_internal(slot, priority, None, false)
    }

    /// 连接一个属于特定分组的槽位
    ///
    /// # 分组的用途
    /// 实现批量管理相关槽位的能力：
    /// - 按 `disconnect_by_group("validators")` 一次性移除所有验证器
    /// - 按功能模块分组（"ui", "network", "persistence"）
    /// - 按生命周期分组（"session", "request"）
    ///
    /// # 分组名规范
    /// 建议使用小写字母和连字符：`"ui-updaters"`, `"log-handlers"`
    ///
    /// # 注意事项
    /// 分组不影响执行顺序（仅优先级影响顺序）。
    /// 同一分组的槽位可以有不同的优先级。
    ///
    /// # 返回值
    /// - `Ok(Connection<Args>)`: 连接成功
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 示例
    /// ```ignore
    /// // 连接一组 UI 更新器
    /// signal.connect_with_group(|data| update_ui(data), "ui")?;
    /// signal.connect_with_group(|data| update_status_bar(data), "ui")?;
    /// // 后续可一次性断开所有 UI 相关槽位
    /// signal.disconnect_by_group("ui");
    /// ```
    pub fn connect_with_group<F: Fn(&Args) + Send + Sync + 'static>(
        &self,
        slot: F,
        group: &str,
    ) -> Result<Connection<Args>, SignalError> {
        self.connect_internal(slot, Priority::Normal, Some(group.to_string()), false)
    }

    /// 连接一个一次性槽位，首次成功调用后自动断开
    ///
    /// # 行为
    /// 槽位在首次成功执行后会被自动从槽位列表中移除，
    /// 后续的 emit 不会再调用此槽位。
    ///
    /// # 适用场景
    /// - 初始化完成通知（只需监听一次）
    /// - 首次事件处理（如首次连接建立）
    /// - 一次性回调（如 Promise 的 resolve）
    ///
    /// # 返回值
    /// - `Ok(Connection<Args>)`: 连接成功
    /// - `Err(SignalError::LockPoisoned)`: 内部锁被污染
    ///
    /// # 注意事项
    /// - 如果槽位在首次调用时 panic，不会被自动移除（仅成功调用触发移除）
    /// - 返回的 Connection 仍可用于手动提前断开
    /// - 自动移除发生在所有槽位执行完毕之后（非即时移除）
    ///
    /// # 示例
    /// ```ignore
    /// // 只在首次数据到达时处理
    /// signal.connect_once(|data| {
    ///     println!("首次数据: {:?}", data);
    /// })?;
    /// ```
    pub fn connect_once<F: Fn(&Args) + Send + Sync + 'static>(
        &self,
        slot: F,
    ) -> Result<Connection<Args>, SignalError> {
        self.connect_internal(slot, Priority::Normal, None, true)
    }

    /// 内部连接实现（所有 connect 方法的统一入口）
    ///
    /// # 执行流程
    /// 1. **原子分配 ID**：`fetch_add` 保证全局唯一且无竞争
    /// 2. **创建连接状态**：`AtomicBool` 标记是否仍处于连接状态
    /// 3. **构建断开闭包**：捕获 slots 引用和 connected 标志，用于后续断开
    /// 4. **包装回调为 Arc**：支持快照发射时低成本克隆
    /// 5. **写入槽位列表**：获取写锁，追加新条目，重新排序
    /// 6. **记录日志**：生成 LogEvent::Connect 条目
    /// 7. **返回 Connection 句柄**
    ///
    /// # 断开闭包的设计
    /// 断开闭包通过 `Arc` 共享，使得：
    /// - Connection 和 Signal 都可以触发断开操作
    /// - 即使 Signal 在其他地方被 clone，断开操作仍然生效
    /// - 闭包内更新 `connected` 标志防止重复断开
    ///
    /// # 时间复杂度
    /// - ID 分配：O(1)（原子操作）
    /// - 排序：O(n log n)（n 为当前槽数量）
    /// - 总体：受排序主导
    fn connect_internal<F: Fn(&Args) + Send + Sync + 'static>(
        &self,
        slot: F,
        priority: Priority,
        group: Option<String>,
        once: bool,
    ) -> Result<Connection<Args>, SignalError> {
        // 步骤 1：原子分配全局唯一的连接 ID
        // Relaxed ordering 足够，因为不需要与其他操作同步（仅用于唯一性）
        let id_u64 = self.next_id.fetch_add(1, AtomicOrdering::Relaxed);
        let id = ConnectionId(id_u64);

        // 步骤 2：创建连接状态标志
        // Arc 共享使得 Connection 和 SlotEntry 可以同步状态
        let connected = Arc::new(AtomicBool::new(true));

        // 步骤 3：构建断开闭包（延迟执行的清理逻辑）
        // 这个闭包会在 Connection::disconnect() 时被调用
        let disconnect_fn = {
            let slots = Arc::clone(&self.slots);
            let connected_flag = Arc::clone(&connected);
            let snapshot_cache = Arc::clone(&self.snapshot_cache);
            Arc::new(move |conn_id: ConnectionId| {
                // 先标记为未连接（防止重复断开）
                connected_flag.store(false, AtomicOrdering::Release);
                // 再从槽位列表中移除
                if let Ok(mut guard) = slots.write() {
                    guard.retain(|s| s.id != conn_id);
                }
                // 失效快照缓存（断开使快照过期）
                if let Ok(mut cache) = snapshot_cache.write() {
                    *cache = None;
                }
            }) as Arc<dyn Fn(ConnectionId) + Send + Sync>
        };

        // 步骤 4：将闭包包装为 Arc（支持快照发射时低成本克隆）
        let callback_arc: Arc<dyn Fn(&Args) + Send + Sync> = Arc::new(slot);

        // 步骤 5：获取写锁并插入新槽位
        // 锁失败时返回错误而非静默丢弃（防御性编程）
        let mut guard =
            self.slots.write().map_err(|_| SignalError::LockPoisoned { name: self.name })?;

        // ASB: 优化：用 partition_point 二分查找插入位置（O(log n)）+ insert（O(n) 平移）
        // 替代 push + sort_by_key（O(n log n) 全排序）。
        //
        // 排序不变量：slots 始终按 priority 降序排列（High 在前）。
        // partition_point 谓词 `|s| s.priority >= priority` 找到第一个 `< priority` 的位置，
        // 该位置正是新元素应插入处：新元素排在所有 >= priority 的元素之后（保持稳定），
        // 在所有 < priority 的元素之前。
        //
        // 注意：原 sort_by_key(Reverse) 是稳定排序，新元素在所有同 priority 元素之后；
        // partition_point 找的位置同样位于所有同 priority 元素之后（因为谓词是 >=），
        // 行为与原 sort_by_key 完全一致，但复杂度从 O(n log n) 降至 O(n)。
        let new_slot_index = guard.partition_point(|s| s.priority >= priority);
        guard.insert(
            new_slot_index,
            SlotEntry::new(id, callback_arc, priority, group, Arc::clone(&connected), once),
        );
        debug_assert!(
            guard.get(new_slot_index).is_some_and(|s| s.id == id),
            "partition_point must locate the inserted slot by priority"
        );
        if let Ok(mut stats_guard) = self.stats.write() {
            // 在对应位置插入，保持与 slots 同序
            let stats_entry = SlotStats::new();
            if new_slot_index <= stats_guard.len() {
                stats_guard.insert(new_slot_index, stats_entry);
            } else {
                // 防御性回退：stats_guard 短于 slots 时 append
                stats_guard.push(stats_entry);
            }
        }

        // 失效快照缓存（槽位列表变更使缓存过期）
        drop(guard); // 释放 slots 写锁，避免与 snapshot_cache 写锁形成锁序
        self.invalidate_snapshot_cache();

        SignalLog::global().log(LogEntry {
            signal_name: self.name,
            timestamp: Instant::now(),
            event: LogEvent::Connect { slot_id: id },
        });

        Ok(Connection::new(id, self.name, connected, disconnect_fn))
    }

    /// 发射信号（通知所有已连接的槽位）
    ///
    /// # 这是主要的通知方法
    /// 等价于 `emit_with_options(args, EmitOptions::default())`
    ///
    /// # 参数
    /// `args`: 要传递给所有槽位的数据引用
    ///
    /// # 返回值
    /// `EmitResult` 包含：
    /// - 成功执行的槽位数（invoked_count）
    /// - 快照中的槽位总数（total_count）
    /// - 收集到的错误（如果有）
    /// - 总耗时
    ///
    /// # 执行保证
    /// - **顺序性**：按优先级从高到低依次执行（单线程顺序）
    /// - **完整性**：除非遇到 ErrorPolicy::Stop，否则所有槽位都会被执行
    /// - **故障隔离**：单个槽位的 panic 不会阻止其他槽位执行
    /// - **递归保护**：槽位回调中再次 emit 同一信号会被检测并跳过
    ///
    /// # 性能说明
    /// - 快照阶段：短暂获取读锁，克隆 Arc 引用（O(1) per slot）
    /// - 执行阶段：无锁，不阻塞 connect/disconnect
    /// - 一次性清理：短暂获取写锁移除 once 槽位
    ///
    /// # 示例
    /// ```ignore
    /// let result = signal.emit(&my_data);
    /// if !result.is_ok() {
    ///     eprintln!("{} 个槽位执行失败", result.errors.len());
    /// }
    /// ```
    pub fn emit(&self, args: &Args) -> EmitResult {
        self.emit_with_options(args, EmitOptions::default())
    }

    /// 带选项的信号发射（高级控制）
    ///
    /// # 与 emit() 的区别
    /// 允许控制错误处理行为（继续 vs 终止）。
    /// 未来可扩展支持超时、取消令牌等选项。
    ///
    /// # 快照发射流程
    /// ```text
    /// 1. 递归检测（thread_local EMITTING.replace(true)）
    ///    │
    ///    ├─ 同线程已在发射 → 返回带警告的 EmitResult
    ///    │
    ///    └─ 首次发射（创建 EmitGuard RAII）↓
    ///
    /// 2. 快照构建（读锁）
    ///    │  收集 (id, Arc<callback>, priority, once, connected)
    ///    │  释放读锁
    ///    │
    /// 3. 回调执行（无锁）
    ///    │  跳过已断开的槽位（connected == false）
    ///    │  catch_unwind 隔离 panic
    ///    │  收集 once 槽位 ID
    ///    │
    /// 4. 一次性槽位清理（写锁）
    ///    │  移除成功调用的 once 槽位
    ///    │  更新 connected 标志
    ///    │
    /// 5. EmitGuard drop 自动恢复 EMITTING=false（panic 安全）
    /// ```
    ///
    /// # AssertUnwindSafe 的使用
    /// 闭包外部的 `&Args` 和 `callback` 被包装为 `AssertUnwindSafe`，
    /// 因为：
    /// - 我们信任 Args 是 UnwindSafe 的（基本类型通常是）
    /// - callback 的 panic 不会破坏数据一致性（只影响当前调用栈）
    /// - 这是 Rust 处理跨 unwind 边界的标准做法
    ///
    /// # Panic 消息提取
    /// 尝试按以下顺序提取 panic 消息：
    /// 1. `&str`（最常见的 panic!() 形式）
    /// 2. `String`（panic!("format {}", value)）
    /// 3. 回退到 "unknown panic"（其他类型）
    pub fn emit_with_options(&self, args: &Args, options: EmitOptions) -> EmitResult {
        let start = Instant::now();

        // ── 递归发射保护（thread_local）──────────────────────────────
        // EMITTING.replace(true)：如果当前为 false，设为 true 并返回 false（首次进入）
        //                       如果当前为 true，保持 true 并返回 true（递归检测）
        //
        // 关键：thread_local 仅在**同一线程**内共享，跨线程 emit 互不干扰。
        // 修复旧版 `Arc<AtomicBool>` 误伤跨线程并发 emit 的 bug（详见模块顶部注释）。
        let was_emitting = EMITTING.with(|f| f.replace(true));
        if was_emitting {
            return EmitResult {
                invoked_count: 0,
                total_count: 0,
                errors: vec![SlotError {
                    slot_id: ConnectionId(0),
                    message: format!(
                        "recursive emit detected for signal '{}', skipping",
                        self.name
                    ),
                }],
                elapsed: start.elapsed(),
            };
        }

        // ── RAII guard：确保 EMITTING 在所有出口（含 panic）恢复 false ──
        // 必须在 was_emitting 检查通过后才创建，避免对递归调用做多余重置。
        let _emit_guard = EmitGuard;

        // ── ASB: emit 计数 + 定期统计衰减 ─────────────────────
        // Relaxed 序：仅用于计数触发衰减，不参与 release/acquire 同步
        let emit_n = self.emit_count.fetch_add(1, AtomicOrdering::Relaxed);
        if emit_n > 0 && emit_n.is_multiple_of(DECAY_INTERVAL) {
            self.decay_all_stats();
        }

        // ── ASB: 路径选择 ─────────────────────────────────────
        // 根据当前槽数选择最优执行路径：
        //   0 槽       → 快速返回（仅一次读锁检查）
        //   ≤4 槽      → 栈上快照路径（避免 Vec 分配）
        //   5-64 槽    → Vec 快照路径（标准快照发射）
        //   >64 槽     → 批量执行路径（预分配 + SlotStats 更新）
        //
        // 注：曾尝试用 try_read 非阻塞获取读锁以避免写锁阻塞，但实测在
        // 无竞争热路径下 try_read 引入的额外分支与 Result 开销反而导致
        // emit 吞吐下降约 50%（S2/S3/S4/S7 全部回归）。RwLock 在 Windows
        // SRWLOCK 实现下 read 已是快速路径，无需 try_read 优化。故回退。
        //
        // 优化：合并读锁——emit_with_options 获取一次读锁做 slot_count 检查后，
        // 将 guard 所有权传递给 emit_direct 复用（避免 emit_direct 再次获取读锁）。
        // 其他路径（snapshot/batch）需先 drop guard 再调用，避免与内部读锁重入死锁。
        let slots_guard = match self.slots.read() {
            Ok(guard) => guard,
            Err(_) => {
                // 锁中毒：返回错误（EMITTING 由 _emit_guard 的 Drop 恢复）
                return EmitResult {
                    invoked_count: 0,
                    total_count: 0,
                    errors: vec![SlotError {
                        slot_id: ConnectionId(0),
                        message: format!("lock poisoned for signal '{}'", self.name),
                    }],
                    elapsed: start.elapsed(),
                };
            }
        };
        let slot_count = slots_guard.len();

        let result = if slot_count == 0 {
            // 快速路径：无已连接槽位（显式释放读锁）
            drop(slots_guard);
            EmitResult {
                invoked_count: 0,
                total_count: 0,
                errors: vec![],
                elapsed: start.elapsed(),
            }
        } else if slot_count <= DIRECT_PATH_THRESHOLD {
            // 栈上快照路径：传递 guard 所有权，复用读锁避免重复获取
            self.emit_direct(slots_guard, args, &options, &start)
        } else if slot_count <= SNAPSHOT_PATH_THRESHOLD {
            // Vec 快照路径：释放读锁，emit_snapshot 内部通过 snapshot_cache 获取
            drop(slots_guard);
            self.emit_snapshot(args, &options, &start)
        } else {
            // 批量执行路径：释放读锁，emit_batch 内部通过 snapshot_cache 获取
            drop(slots_guard);
            self.emit_batch(args, &options, &start, slot_count)
        };

        // ── 清除递归发射保护标志 ──────────────────────────────────
        // 由 _emit_guard 的 Drop 自动处理（panic 安全）

        // ── 记录日志 ──────────────────────────────────────────
        SignalLog::global().log(LogEntry {
            signal_name: self.name,
            timestamp: Instant::now(),
            event: LogEvent::Emit {
                slot_count: result.total_count,
                error_count: result.errors.len(),
                elapsed: result.elapsed,
            },
        });

        result
    }

    /// ASB 路径 1：栈上直接执行（槽数 ≤ 4）
    ///
    /// # 优化点
    /// - 使用栈上数组 `[Option<SlotSnapshot>; 4]` 避免 Vec 堆分配
    /// - 适用于高频小数量槽位场景（如单槽回调）
    /// - 复用 emit_with_options 传入的读锁，避免重复获取（合并读锁优化）
    ///
    /// # 语义保证
    /// 与 [`Self::emit_snapshot`] 完全一致：故障隔离、once 清理、优先级顺序。
    ///
    /// # 读锁所有权
    /// `slots_guard` 由 emit_with_options 传入，本方法负责释放：
    /// - 降级路径（stack_len 超阈值）：drop 后调用 emit_snapshot
    /// - 正常路径：填充完 stack_slots 后立即 drop，再执行回调（避免持锁回调）
    /// - 必须在调用 cleanup_once_slots 前释放（cleanup_once_slots 获取写锁）
    fn emit_direct(
        &self,
        slots_guard: std::sync::RwLockReadGuard<'_, Vec<SlotEntry<Args>>>,
        args: &Args,
        options: &EmitOptions,
        start: &Instant,
    ) -> EmitResult {
        // 栈上快照数组：4 个槽位 + 1 个溢出标记
        // 使用 Option 表示空槽，初始化为 None
        let mut stack_slots: [Option<SlotSnapshot<Args>>; DIRECT_PATH_THRESHOLD] =
            [const { None }; DIRECT_PATH_THRESHOLD];
        let mut stack_len = 0usize;

        // 复用 emit_with_options 传入的读锁填充栈上快照，避免重复获取
        for s in slots_guard.iter() {
            if stack_len >= DIRECT_PATH_THRESHOLD {
                // 槽数超过阈值：降级到快照路径（防御性降级，正常分派下不触发）
                // 需先释放读锁，避免与 emit_snapshot 内部读锁重入死锁
                drop(slots_guard);
                return self.emit_snapshot(args, options, start);
            }
            stack_slots[stack_len] =
                Some((s.id, Arc::clone(&s.callback), s.priority, s.once, Arc::clone(&s.connected)));
            stack_len += 1;
        }
        // 显式释放读锁，避免持有锁执行回调（回调可能触发 connect/disconnect）
        drop(slots_guard);

        let total_count = stack_len;

        // ── 回调执行（无锁）──────────────────────────────────
        let mut invoked = 0usize;
        let mut errors = Vec::new();
        let mut once_ids_to_remove = Vec::new();

        for slot in stack_slots.iter_mut().take(stack_len) {
            // 安全：take 只在已填充位置调用
            let (id, callback, _priority, once, connected) =
                slot.take().expect("stack slot must be Some");

            // 跳过已断开的槽位
            if !connected.load(AtomicOrdering::Relaxed) {
                continue;
            }

            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                callback(args);
            })) {
                Ok(()) => {
                    invoked += 1;
                    if once {
                        once_ids_to_remove.push(id);
                    }
                }
                Err(panic_payload) => {
                    let message = extract_panic_message(panic_payload);
                    errors.push(SlotError { slot_id: id, message });
                    if options.on_error == ErrorPolicy::Stop {
                        break;
                    }
                }
            }
        }

        // ── once 槽位清理 ─────────────────────────────────────
        if !once_ids_to_remove.is_empty() {
            self.cleanup_once_slots(&once_ids_to_remove);
        }

        EmitResult { invoked_count: invoked, total_count, errors, elapsed: start.elapsed() }
    }

    /// ASB 路径 2：Vec 快照执行（4 < 槽数 ≤ 64）
    ///
    /// 这是优化后的现有快照路径：
    /// - 预分配 Vec 容量，避免 realloc
    /// - 释放锁后执行回调，不阻塞 connect/disconnect
    fn emit_snapshot(&self, args: &Args, options: &EmitOptions, start: &Instant) -> EmitResult {
        // ── 快照构建：优先用缓存（O(1) Arc::clone），miss 时重建 ──
        // 100 槽场景下，命中缓存时省去 100 次 Arc::clone + Vec 分配，
        // 是 S4 性能提升的核心。
        let snapshot: Arc<[SlotSnapshot<Args>]> = match self.get_or_build_snapshot() {
            Ok(s) => s,
            Err(_) => {
                return EmitResult {
                    invoked_count: 0,
                    total_count: 0,
                    errors: vec![SlotError {
                        slot_id: ConnectionId(0),
                        message: format!("lock poisoned for signal '{}'", self.name),
                    }],
                    elapsed: start.elapsed(),
                };
            }
        };
        let total_count = snapshot.len();

        // ── 回调执行（无锁，不阻塞 connect/disconnect）──────────
        let mut invoked = 0usize;
        let mut errors = Vec::new();
        let mut once_ids_to_remove = Vec::new();

        for (id, callback, _priority, once, connected) in snapshot.iter() {
            // 跳过在快照构建后、回调执行前已断开的槽位
            if !connected.load(AtomicOrdering::Relaxed) {
                continue;
            }

            // 故障隔离：catch_unwind 防止单个槽位崩溃影响整体
            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                callback(args);
            })) {
                Ok(()) => {
                    invoked += 1;
                    if *once {
                        once_ids_to_remove.push(*id);
                    }
                }
                Err(panic_payload) => {
                    let message = extract_panic_message(panic_payload);
                    errors.push(SlotError { slot_id: *id, message });
                    if options.on_error == ErrorPolicy::Stop {
                        break;
                    }
                }
            }
        }

        // ── once 槽位清理 ─────────────────────────────────────
        if !once_ids_to_remove.is_empty() {
            self.cleanup_once_slots(&once_ids_to_remove);
        }

        EmitResult { invoked_count: invoked, total_count, errors, elapsed: start.elapsed() }
    }

    /// ASB 路径 3：批量执行（槽数 > 64）
    ///
    /// # 优化点
    /// - 快照缓存命中时 O(1) Arc::clone，避免 100 次逐元素 clone
    /// - 直接迭代快照执行（单次 emit 内无需 EmitBatch 分批，避免 push/drain 开销）
    /// - emit 结束时批量更新 [`SlotStats`]（record_call），用 `try_write` 非阻塞
    ///
    /// # 语义保证
    /// 与 [`Self::emit_snapshot`] 一致：故障隔离、once 清理、优先级顺序。
    /// SlotStats 更新是"尽力而为"的，失败不影响功能正确性。
    ///
    /// # 关于 EmitBatch
    /// [`EmitBatch`] 结构定义保留在 `slot_stats` 模块，供未来跨 emit 延迟归并使用。
    /// 当前单次 emit 内直接迭代更高效（100 槽 < [`BATCH_MAX_SIZE`] = 128，分批无意义）。
    fn emit_batch(
        &self,
        args: &Args,
        options: &EmitOptions,
        start: &Instant,
        _slot_count: usize,
    ) -> EmitResult {
        // ── 快照构建：优先用缓存（O(1) Arc::clone），miss 时重建 ──
        let snapshot: Arc<[SlotSnapshot<Args>]> = match self.get_or_build_snapshot() {
            Ok(s) => s,
            Err(_) => {
                return EmitResult {
                    invoked_count: 0,
                    total_count: 0,
                    errors: vec![SlotError {
                        slot_id: ConnectionId(0),
                        message: format!("lock poisoned for signal '{}'", self.name),
                    }],
                    elapsed: start.elapsed(),
                };
            }
        };
        let total_count = snapshot.len();

        // ── 回调执行（直接迭代，无锁，不阻塞 connect/disconnect）──
        let mut invoked = 0usize;
        let mut errors = Vec::new();
        let mut once_ids_to_remove = Vec::new();

        for (id, callback, _priority, once, connected) in snapshot.iter() {
            // 跳过已断开的槽位
            if !connected.load(AtomicOrdering::Relaxed) {
                continue;
            }

            // 故障隔离：catch_unwind 防止单个槽位崩溃影响整体
            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                callback(args);
            })) {
                Ok(()) => {
                    invoked += 1;
                    if *once {
                        once_ids_to_remove.push(*id);
                    }
                }
                Err(panic_payload) => {
                    let message = extract_panic_message(panic_payload);
                    errors.push(SlotError { slot_id: *id, message });
                    if options.on_error == ErrorPolicy::Stop {
                        break;
                    }
                }
            }
        }

        // ── once 槽位清理 ─────────────────────────────────────
        if !once_ids_to_remove.is_empty() {
            self.cleanup_once_slots(&once_ids_to_remove);
        }

        // ── ASB: 批量更新 SlotStats（每 STATS_UPDATE_INTERVAL 次 emit 更新一次）──
        // 尽力而为：失败不影响功能正确性
        // 周期性更新分摊 100 次 record_call 开销，SlotStats 仅用于统计，非功能正确性依赖。
        let should_update_stats =
            self.emit_count.load(AtomicOrdering::Relaxed).is_multiple_of(STATS_UPDATE_INTERVAL);
        if should_update_stats && let Ok(mut stats_guard) = self.stats.try_write() {
            // 同步 stats 长度与 slots（防御性：disconnect 可能未精确同步）
            if stats_guard.len() > total_count {
                stats_guard.truncate(total_count);
            }
            for stat in stats_guard.iter_mut() {
                stat.record_call();
            }
        }

        EmitResult { invoked_count: invoked, total_count, errors, elapsed: start.elapsed() }
    }

    /// once 槽位清理（共享辅助方法）
    ///
    /// 移除成功调用的 once 槽位，并同步更新 stats。
    /// 仅在存在 once 槽位时调用，避免不必要的写锁获取。
    ///
    /// # 性能优化
    /// - `once_ids` 长度 ≤ 4：直接线性查找（避免 HashSet 分配开销）
    /// - `once_ids` 长度 > 4：构建 HashSet 加速查找（O(n*k) → O(n)）
    ///   其中 n = slots 长度，k = once_ids 长度
    fn cleanup_once_slots(&self, once_ids: &[ConnectionId]) {
        if once_ids.is_empty() {
            return;
        }

        // 收集被移除的索引，用于同步更新 stats
        let mut removed_indices: Vec<usize> = Vec::new();
        let mut changed = false;

        // ASB: 优化：当 once_ids 较多时用 HashSet 加速 contains 查找
        // 阈值 4 来自经验值：小集合下线性扫描比 HashSet 分配更快
        // （HashSet 至少要分配 bucket 数组 + 哈希计算开销）
        let use_hashset = once_ids.len() > 4;
        let once_set: std::collections::HashSet<ConnectionId> = if use_hashset {
            once_ids.iter().copied().collect()
        } else {
            std::collections::HashSet::new()
        };

        if let Ok(mut guard) = self.slots.write() {
            let mut idx = 0;
            guard.retain(|s| {
                // 双路径：小集合线性查找，大集合 HashSet 查找
                let should_remove =
                    if use_hashset { once_set.contains(&s.id) } else { once_ids.contains(&s.id) };
                if should_remove {
                    s.connected.store(false, AtomicOrdering::Release);
                    SignalLog::global().log(LogEntry {
                        signal_name: self.name,
                        timestamp: Instant::now(),
                        event: LogEvent::Disconnect { slot_id: s.id },
                    });
                    removed_indices.push(idx);
                    changed = true;
                    false
                } else {
                    idx += 1;
                    true
                }
            });
        }

        // ASB: 按 index 降序移除 stats，保持与 slots 一一对应
        if !removed_indices.is_empty()
            && let Ok(mut stats_guard) = self.stats.write()
        {
            for &idx in removed_indices.iter().rev() {
                if idx < stats_guard.len() {
                    stats_guard.remove(idx);
                }
            }
        }

        // 快照缓存失效：slots 列表变更后必须重建快照
        if changed {
            self.invalidate_snapshot_cache();
        }
    }

    /// ASB: 对所有 SlotStats 执行一次衰减
    ///
    /// 每 [`DECAY_INTERVAL`] 次 emit 触发一次。
    /// 衰减让长期未调用的槽位分数逐渐降低，避免历史数据过期。
    fn decay_all_stats(&self) {
        if let Ok(mut stats_guard) = self.stats.write() {
            for stat in stats_guard.iter_mut() {
                stat.decay();
            }
        }
    }

    /// 获取 emit 快照：优先用缓存，miss 时构建并写回缓存
    ///
    /// # 性能特征（S4 优化核心）
    /// - 命中路径：1 次 `Arc::clone`（O(1) 原子操作），替代 100 次逐元素 clone
    /// - miss 路径：构建 `Vec` → 转为 `Arc<[T]>` → `try_write` 写回缓存
    ///
    /// # 锁序保证
    /// - 命中路径：仅持有 `snapshot_cache` 读锁（短暂，Arc::clone 后释放）
    /// - miss 路径：先读 `snapshot_cache`（已释放）→ 读 `slots`（构建快照，释放）→
    ///   `try_write` `snapshot_cache`（不阻塞，失败则跳过）
    ///
    /// 这保证了与 connect/disconnect 的锁序（slots 写 → snapshot_cache 写）不形成环。
    fn get_or_build_snapshot(&self) -> Result<Arc<[SlotSnapshot<Args>]>, SignalError> {
        // 1. 尝试读缓存（短暂持有读锁）
        if let Ok(cache) = self.snapshot_cache.read()
            && let Some(ref snap) = *cache
        {
            return Ok(Arc::clone(snap));
        }

        // 2. 缓存 miss：读 slots 构建新快照（slots 读锁在此块内获取并释放）
        let new_snapshot: Arc<[SlotSnapshot<Args>]> = {
            let guard =
                self.slots.read().map_err(|_| SignalError::LockPoisoned { name: self.name })?;
            let mut snap = Vec::with_capacity(guard.len());
            for s in guard.iter() {
                snap.push((
                    s.id,
                    Arc::clone(&s.callback),
                    s.priority,
                    s.once,
                    Arc::clone(&s.connected),
                ));
            }
            snap.into()
        };

        // 3. 尝试写缓存（try_write 避免阻塞，失败则跳过——下次 emit 再缓存）
        if let Ok(mut cache) = self.snapshot_cache.try_write() {
            *cache = Some(Arc::clone(&new_snapshot));
        }

        Ok(new_snapshot)
    }

    /// 显式失效快照缓存
    ///
    /// 在槽位列表变更（connect/disconnect/once 清理）后调用，
    /// 确保下次 emit 重建快照。使用 `try_write()` 非阻塞获取写锁：
    /// 若 emit 正持有读锁则跳过失效，下次 emit 会因 connected 标志
    /// （Arc<AtomicBool> 实时反映状态）正确跳过已断开槽位，
    /// 仅新增槽位可能延迟一次 emit 才被调用（可接受最终一致性）。
    fn invalidate_snapshot_cache(&self) {
        if let Ok(mut cache) = self.snapshot_cache.try_write() {
            *cache = None;
        }
    }

    /// 断开所有已连接的槽位
    ///
    /// # 效果
    /// 清空槽位列表，释放所有回调闭包。
    /// 已有的 `Connection` 句柄会标记为未连接状态。
    ///
    /// # 使用场景
    /// - 模块卸载时的清理工作
    /// - 重置系统到初始状态
    /// - 测试环境的 teardown
    ///
    /// # 注意事项
    /// 此操作不可逆！所有连接都需要重新建立。
    /// 如果只想断开部分槽位，请使用 `disconnect_by_group()`。
    /// 每个被断开的槽位都会更新其 connected 标志并记录日志。
    pub fn disconnect_all(&self) {
        if let Ok(mut guard) = self.slots.write() {
            // 先更新每个条目的 connected 标志并记录日志
            for entry in guard.iter() {
                entry.connected.store(false, AtomicOrdering::Release);
                SignalLog::global().log(LogEntry {
                    signal_name: self.name,
                    timestamp: Instant::now(),
                    event: LogEvent::Disconnect { slot_id: entry.id },
                });
            }
            guard.clear();
        }
        // ASB: 同步清空统计
        if let Ok(mut stats_guard) = self.stats.write() {
            stats_guard.clear();
        }
        // 快照缓存失效：slots 列表已清空
        self.invalidate_snapshot_cache();
    }

    /// 按分组名称批量断开槽位
    ///
    /// # 参数
    /// `group`: 要断开的分组名称（必须与 connect_with_group 中的名称完全匹配）
    ///
    /// # 效果
    /// 移除所有属于该分组的槽位，保留其他槽位不变。
    /// 每个被移除的槽位都会更新其 connected 标志并记录日志。
    ///
    /// # 时间复杂度
    /// O(n) 其中 n 为当前槽数量（retain 操作遍历一次）
    ///
    /// # 示例
    /// ```ignore
    /// // 移除所有临时调试槽位
    /// signal.disconnect_by_group("debug");
    /// // 移除所有 session 级别的监听器
    /// signal.disconnect_by_group("session");
    /// ```
    pub fn disconnect_by_group(&self, group: &str) {
        let mut changed = false;
        if let Ok(mut guard) = self.slots.write() {
            // ASB: 先收集要移除的索引（retain 之前），便于同步更新 stats
            let indices_to_remove: Vec<usize> = guard
                .iter()
                .enumerate()
                .filter(|(_, s)| s.group.as_deref() == Some(group))
                .map(|(i, _)| i)
                .collect();

            if !indices_to_remove.is_empty() {
                changed = true;
            }

            guard.retain(|s| {
                // 使用 as_deref() 避免 group.to_string() 的堆分配
                if s.group.as_deref() == Some(group) {
                    // 标记为未连接（与 Connection 共享的 AtomicBool）
                    s.connected.store(false, AtomicOrdering::Release);
                    // 记录断开日志
                    SignalLog::global().log(LogEntry {
                        signal_name: self.name,
                        timestamp: Instant::now(),
                        event: LogEvent::Disconnect { slot_id: s.id },
                    });
                    false // 从列表中移除
                } else {
                    true // 保留
                }
            });

            // ASB: 按 index 降序移除 stats，保持与 slots 一一对应
            if !indices_to_remove.is_empty()
                && let Ok(mut stats_guard) = self.stats.write()
            {
                for &idx in indices_to_remove.iter().rev() {
                    if idx < stats_guard.len() {
                        stats_guard.remove(idx);
                    }
                }
            }
        }

        // 快照缓存失效：slots 列表变更后必须重建快照
        if changed {
            self.invalidate_snapshot_cache();
        }
    }

    /// 检查信号是否没有任何已连接的槽位
    ///
    /// # 返回值
    /// - `true`: 无槽位连接（信号处于空闲状态）
    /// - `false`: 至少有一个槽位已连接
    ///
    /// # 使用场景
    /// - 条件优化：如果没有监听者，可以跳过昂贵的计算
    /// - 清理检测：判断是否需要进行资源回收
    pub fn is_empty(&self) -> bool {
        self.slots.read().map(|g| g.is_empty()).unwrap_or(true)
    }

    /// 获取信号的名称
    ///
    /// # 返回值
    /// 创建时传入的 `'static str` 标识符
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 获取当前已连接的槽位数量
    ///
    /// # 返回值
    /// 当前活跃的连接数（不包括已断开但尚未物理移除的）
    ///
    /// # 使用场景
    /// - 监控信号的健康状况
    /// - 检测可能的内存泄漏（槽数量持续增长）
    /// - 性能分析（槽数量过多可能导致发射延迟）
    pub fn slot_count(&self) -> usize {
        self.slots.read().map(|g| g.len()).unwrap_or(0)
    }

    /// 创建一个新的信号句柄（共享底层状态）
    ///
    /// # Clone 语义
    /// 这是**浅拷贝**：新的句柄与原实例共享：
    /// - 相同的槽位列表（Arc::clone）
    /// - 相同的 ID 计数器（Arc::clone）
    /// - 相同的递归保护标志（thread_local `EMITTING`，每线程独立）
    /// - 相同的名称
    ///
    /// # 使用场景
    /// - 将同一个信号传递给不同的模块/组件
    /// - 避免使用 `Arc<Signal<Args>>` 的语法噪音
    ///
    /// # 重要提示
    /// 对克隆体的任何修改（connect/disconnect）都会反映到原始实例上！
    pub fn clone_handle(&self) -> Self {
        Self {
            name: self.name,
            slots: Arc::clone(&self.slots),
            next_id: Arc::clone(&self.next_id),
            stats: Arc::clone(&self.stats),
            // 修复：emit_count 改为 Arc<AtomicU64> 共享，符合 clone_handle
            // 文档承诺"克隆体与原实例共享同一组状态"。
            emit_count: Arc::clone(&self.emit_count),
            snapshot_cache: Arc::clone(&self.snapshot_cache),
        }
    }
}

/// 自动实现 Clone trait（委托给 clone_handle）
impl<Args: 'static + Send + Sync> Clone for Signal<Args> {
    fn clone(&self) -> Self {
        self.clone_handle()
    }
}

/// 从 panic payload 中提取可读的错误消息
///
/// 尝试按以下顺序提取：
/// 1. `&str`（最常见的 `panic!()` 形式）
/// 2. `String`（`panic!("format {}", value)`）
/// 3. 回退到 `"unknown panic"`（其他类型）
///
/// 提取为独立函数以避免在三个 emit 路径中重复代码，
/// 并允许编译器更好地内联。
fn extract_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// ============================================================================
// 回归测试（P2 BUG-signal-emit_count-clone / P2 BUG-connection-no-drop）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    /// P2 BUG-signal-emit_count-clone 回归测试：
    ///
    /// clone_handle 必须通过 `Arc::clone` 共享 `emit_count`，确保克隆体与
    /// 原信号的统计衰减（DECAY_INTERVAL）与 SlotStats 批量更新
    /// （STATS_UPDATE_INTERVAL）保持同步。
    ///
    /// 旧版 bug：emit_count 为 `AtomicU64`（非 Arc），clone_handle 时
    /// 复制独立计数器，导致克隆体 emit 不计入原信号的衰减触发计数，
    /// 违反 clone_handle 文档承诺"克隆体与原实例共享同一组状态"。
    #[test]
    fn test_clone_handle_shares_emit_count() {
        let signal: Signal<i32> = Signal::named("test:clone_emit_count");
        let cloned = signal.clone_handle();

        // 1. 静态验证：emit_count 必须是同一个 Arc（指针相等）
        assert!(
            Arc::ptr_eq(&signal.emit_count, &cloned.emit_count),
            "clone_handle must share emit_count via Arc::clone"
        );

        // 2. 行为验证：原信号 emit 后，克隆体的 emit_count 也应增长
        assert_eq!(signal.emit_count.load(AtomicOrdering::Relaxed), 0);
        assert_eq!(cloned.emit_count.load(AtomicOrdering::Relaxed), 0);

        // 无槽位 emit 走快速返回路径，仍会递增 emit_count
        signal.emit(&42);
        signal.emit(&42);
        signal.emit(&42);

        assert_eq!(
            cloned.emit_count.load(AtomicOrdering::Relaxed),
            3,
            "cloned signal must observe emit_count increments from original"
        );

        // 3. 反向验证：在克隆体上 emit，原信号也增长
        cloned.emit(&42);
        assert_eq!(
            signal.emit_count.load(AtomicOrdering::Relaxed),
            4,
            "original signal must observe emit_count increments from clone"
        );
    }

    /// P2 BUG-signal-emit_count-clone 回归测试（补充）：
    ///
    /// clone_handle 共享其他所有 Arc 字段（slots/next_id/stats/snapshot_cache），
    /// 确保 clone 语义一致性。
    ///
    /// 注意：`emitting` 已改为 thread_local `EMITTING` 静态变量（不再属于结构体字段），
    /// 因此不在此处断言。详见 [`EMITTING`] 模块顶部注释。
    #[test]
    fn test_clone_handle_shares_all_arc_fields() {
        let signal: Signal<i32> = Signal::named("test:clone_all_arcs");
        let cloned = signal.clone_handle();

        assert!(Arc::ptr_eq(&signal.slots, &cloned.slots), "slots must be shared");
        assert!(Arc::ptr_eq(&signal.next_id, &cloned.next_id), "next_id must be shared");
        assert!(Arc::ptr_eq(&signal.stats, &cloned.stats), "stats must be shared");
        assert!(
            Arc::ptr_eq(&signal.snapshot_cache, &cloned.snapshot_cache),
            "snapshot_cache must be shared"
        );
    }

    /// P2 BUG-connection-no-drop 回归测试：
    ///
    /// Connection 被 drop 时必须自动调用 disconnect()，从信号的槽位列表中
    /// 物理移除对应条目（disconnect_fn 内 `slots.retain(|s| s.id != conn_id)`）。
    ///
    /// 旧版 bug：未实现 Drop，Connection 在 drop 时不清理槽位，导致已废弃的
    /// 槽位残留在 Signal::slots 中，造成内存泄漏和无效回调执行。
    #[test]
    fn test_connection_drop_auto_disconnects() {
        let signal: Signal<i32> = Signal::named("test:connection_drop");
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        // 1. 连接一个槽位，Connection 在内层作用域结束时 drop
        {
            let _conn = signal
                .connect(move |_data| {
                    counter_clone.fetch_add(1, AtomicOrdering::Relaxed);
                })
                .expect("connect must succeed");
            assert_eq!(signal.slot_count(), 1, "slot must be registered after connect");
        } // _conn 在此 drop

        // 2. Connection 已 drop，槽位应被自动物理移除
        assert_eq!(
            signal.slot_count(),
            0,
            "slot must be removed from Signal::slots after Connection drops"
        );

        // 3. emit 不应触发已 drop 的槽位（counter 保持 0）
        signal.emit(&42);
        assert_eq!(
            counter.load(AtomicOrdering::Relaxed),
            0,
            "dropped Connection's slot must not be invoked"
        );
    }

    /// P2 BUG-connection-no-drop 回归测试（补充）：
    ///
    /// 显式 disconnect() 后再 drop Connection 应是幂等的 no-op，
    /// 不会重复触发 disconnect_fn 或 panic。
    #[test]
    fn test_connection_disconnect_then_drop_is_idempotent() {
        let signal: Signal<i32> = Signal::named("test:disconnect_then_drop");
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let conn = signal
            .connect(move |_data| {
                counter_clone.fetch_add(1, AtomicOrdering::Relaxed);
            })
            .expect("connect must succeed");

        assert_eq!(signal.slot_count(), 1);
        assert!(conn.is_connected());

        // 显式断开
        conn.disconnect();
        assert!(!conn.is_connected(), "must be disconnected after explicit disconnect");
        assert_eq!(signal.slot_count(), 0, "slot must be removed after explicit disconnect");

        // 再次 drop：应是无害的 no-op（CAS 失败）
        drop(conn);

        // emit 不应触发槽位
        signal.emit(&42);
        assert_eq!(counter.load(AtomicOrdering::Relaxed), 0, "slot must not be invoked");
    }

    /// emit_direct 降级路径回归测试：
    ///
    /// 当 emit_direct 收到的 slots_guard 槽数 > DIRECT_PATH_THRESHOLD (4) 时，
    /// 必须降级到 emit_snapshot 路径（L757-762 的防御性代码），避免
    /// stack_slots 数组越界 panic。降级路径需先 `drop(slots_guard)` 释放读锁，
    /// 再调用 emit_snapshot，避免与 get_or_build_snapshot 内部读锁重入死锁。
    ///
    /// 本测试绕过 emit_with_options 分派，直接调用 emit_direct 传入 5 槽 guard，
    /// 验证：
    /// 1. 不死锁（drop + emit_snapshot 内部 read 不重入）
    /// 2. 降级到 emit_snapshot 后所有 5 个 slot 都被执行
    /// 3. 返回正确的 EmitResult（total_count=5, invoked_count=5, is_ok=true）
    #[test]
    fn test_asb_direct_path_degrade_to_snapshot() {
        let signal: Signal<i32> = Signal::named("test:direct_degrade");
        let counter = Arc::new(AtomicU32::new(0));

        // 连接 5 个槽位（超过 DIRECT_PATH_THRESHOLD=4）
        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(5);
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as u32, AtomicOrdering::Relaxed);
                    })
                    .unwrap(),
            );
        }
        assert_eq!(signal.slot_count(), 5);

        // 直接调用 emit_direct（绕过 emit_with_options 分派）
        // 传入 5 槽的 guard，触发 L757-762 的降级路径
        let start = Instant::now();
        let options = EmitOptions::default();
        let guard = signal.slots.read().expect("slots read lock must succeed");
        let result = signal.emit_direct(guard, &42, &options, &start);

        // 验证：降级到 emit_snapshot 后所有 5 个 slot 都被执行
        // 若未降级，stack_slots 数组大小为 4 会越界 panic，或只能填充 4 个
        assert_eq!(result.total_count, 5, "degraded path must execute all 5 slots");
        assert_eq!(result.invoked_count, 5, "all 5 slots must be invoked");
        assert!(result.is_ok(), "no errors expected in degrade path");

        // 验证：counter 累加了 5 次 * 42 = 210
        assert_eq!(
            counter.load(AtomicOrdering::Relaxed),
            5 * 42,
            "counter must reflect all 5 slot invocations"
        );
    }

    /// 长时间并发 emit 回归测试：
    ///
    /// 验证单槽 Signal 在高并发长时间 emit 下无死锁、无 panic。
    ///
    /// 测试场景：4 线程 × 100K 次 emit = 400K 次，验证：
    /// 1. 所有线程 join 成功返回（无死锁）
    /// 2. 无 panic（join().expect() 不触发）
    /// 3. counter > 0（证明槽位在并发下被成功调用）
    /// 4. counter <= 400K（不会过度计数）
    ///
    /// # 关于 thread_local EMITTING 修复后行为
    ///
    /// 旧版 `emitting: Arc<AtomicBool>` 在所有线程间共享，跨线程并发 emit
    /// 会被误判为"递归"提前返回，实测约 20-25% 成功率。
    /// 修复后改用 thread_local `EMITTING`，不同线程的 emit 互不干扰，
    /// 全部 400K 次 emit 都会成功执行槽位（counter == 400K）。
    #[test]
    fn test_concurrent_emit_long_running() {
        use std::thread;

        let signal: Signal<i32> = Signal::named("test:concurrent_long");
        let counter = Arc::new(AtomicU32::new(0));

        // 单槽 Signal
        let c = Arc::clone(&counter);
        let _conn = signal
            .connect(move |v| {
                c.fetch_add(*v as u32, AtomicOrdering::Relaxed);
            })
            .unwrap();

        // 4 线程 × 100K 次 = 400K 次
        const THREADS: usize = 4;
        const ITERS_PER_THREAD: u32 = 100_000;
        const EXPECTED_TOTAL: u32 = (THREADS as u32) * ITERS_PER_THREAD;

        let mut handles = vec![];
        for _ in 0..THREADS {
            let sig = signal.clone_handle();
            handles.push(thread::spawn(move || {
                for _ in 0..ITERS_PER_THREAD {
                    sig.emit(&1);
                }
            }));
        }

        // join：验证无死锁（死锁时 join 会无限阻塞，测试超时失败）
        for h in handles {
            h.join().expect("thread must not panic (no deadlock)");
        }

        // 验证：counter > 0（证明槽位在并发下被成功调用，非仅"不 panic"）
        let final_count = counter.load(AtomicOrdering::Relaxed);
        assert!(
            final_count > 0,
            "counter must be > 0 (slot must be invoked at least once under concurrency)"
        );
        // 验证：counter <= 400K（不会过度计数，证明 fetch_add 原子性正确）
        assert!(
            final_count <= EXPECTED_TOTAL,
            "counter {} must not exceed total emits {} (no over-counting)",
            final_count,
            EXPECTED_TOTAL
        );
    }

    /// 并发 emit 不应被递归保护标志误伤回归测试：
    ///
    /// 旧版 bug：`emitting: Arc<AtomicBool>` 在所有线程间共享，
    /// `swap(true, SeqCst)` 在跨线程并发 emit 时误判为"递归"，
    /// 导致 bench_concurrent_emit 实测仅 ~21% 成功率。
    ///
    /// 修复后：`emitting` 改为 thread_local `EMITTING`（`Cell<bool>`），
    /// 仅在**同一线程**内共享，跨线程 emit 互不干扰。
    ///
    /// 本测试验证：2 线程 × 100 次 = 200 次并发 emit，全部应成功调用槽位（counter == 200）。
    /// 通过比较"槽位实际被调用次数"与"emit 调用次数"判断是否被误阻挡，
    /// 而非依赖 emit 返回的 EmitResult（递归阻挡时返回的错误结构在并发场景下不一定可观察）。
    #[test]
    fn test_concurrent_emit_no_false_blocked() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::thread;

        let signal: Signal<i32> = Signal::named("test:concurrent_no_false_blocked");
        // 槽位计数器：每次槽位被成功调用时递增
        let slot_invoked = Arc::new(AtomicUsize::new(0));
        let rounds: usize = 100;

        // 连接 1 个槽位：递增 slot_invoked 计数器
        let slot_invoked_clone = Arc::clone(&slot_invoked);
        let _conn = signal
            .connect(move |_v| {
                slot_invoked_clone.fetch_add(1, Ordering::Relaxed);
            })
            .expect("connect must succeed");

        // 2 线程并发 emit
        let signal_clone = signal.clone_handle();
        let handle = thread::spawn(move || {
            for _ in 0..rounds {
                let _ = signal_clone.emit(&42);
            }
        });
        for _ in 0..rounds {
            let _ = signal.emit(&42);
        }
        handle.join().expect("worker thread must not panic");

        // 修复前：仅 ~21% 成功（emit 被 emitting 标志误判为递归而提前返回，槽位未执行）
        // 修复后：全部成功（slot_invoked == 2 * rounds = 200）
        assert_eq!(
            slot_invoked.load(Ordering::Relaxed),
            2 * rounds,
            "并发 emit 应全部执行槽位，不应被 emitting 标志阻挡（旧版仅 ~21% 成功）"
        );
    }
}
