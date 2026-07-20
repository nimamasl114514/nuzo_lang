//! # 槽位与连接管理模块
//!
//! 本模块定义了观察者模式中的**观察者（Observer）**表示和**绑定关系（Connection）**。
//!
//! ## 核心抽象
//!
//! ### 1. SlotEntry（槽位条目）
//! 观察者模式中的**具体观察者**，封装了：
//! - 唯一标识符（ConnectionId）
//! - 回调函数（用户提供的业务逻辑，Arc 包装以支持快照发射）
//! - 优先级（决定执行顺序）
//! - 分组标签（支持批量管理）
//! - 连接状态标志（Arc<AtomicBool>，与 Connection 共享）
//! - 一次性标志（once: bool，首次调用后自动断开）
//!
//! ### 2. Connection（连接句柄）
//! 观察者与主题之间的**绑定契约**，提供：
//! - RAII 式自动断开（可选）
//! - 显式断开控制（&self，非消费型）
//! - 连接状态查询
//!
//! ## 设计决策
//!
//! ### 为什么将 SlotEntry 和 Connection 分离？
//! - **单一职责**：SlotEntry 是数据结构，Connection 是行为接口
//! - **生命周期解耦**：Connection 可以独立于 SlotEntry 存在
//! - **性能优化**：SlotEntry 紧凑存储在 Vec 中，Connection 可按需创建
//!
//! ## 内存布局
//!
//! ```text
//! SlotEntry<Args> {
//!     id: ConnectionId,          // 8 字节
//!     callback: Arc<dyn Fn>,     // 16 字节（指针 + vtable）
//!     priority: Priority,        // 2 字节（枚举）
//!     group: Option<String>,     // 24 字节（Option 的 discriminant + String）
//!     connected: Arc<AtomicBool>,// 16 字节（Arc 指针）
//!     once: bool,                // 1 字节
//! } // 总计 ~67 字节（64 位系统，对齐后）
//!
//! Connection<Args> {
//!     id: ConnectionId,           // 8 字节
//!     signal_name: &str,          // 8 字节
//!     connected: Arc<AtomicBool>, // 16 字节
//!     disconnect_fn: Arc<dyn Fn>, // 16 字节
//!     _marker: PhantomData,       // 0 字节（ZST）
//! } // 总计 ~48 字节
//! ```
//!
//! ## 线程安全性
//!
//! - `SlotEntry`: 本身不是线程安全的，但存储在 `RwLock<Vec<SlotEntry>>` 中
//! - `Connection`: 通过 `Arc<AtomicBool>` 实现无锁状态同步
//! - 断开操作：原子地更新状态标志，然后获取写锁修改槽位列表

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::types::{ConnectionId, Priority};

/// 槽位条目（观察者的内部表示）
///
/// # 角色
/// 这是信号内部存储的观察者实例。每个已连接的回调函数都会被包装为
/// 一个 `SlotEntry` 并存储在信号的槽位列表中。
///
/// # 字段设计说明
///
/// ## id（连接标识符）
/// 全局唯一的 ID，用于：
/// - 在断开时精确定位要移除的条目
/// - 在错误消息中标识出问题的槽位
/// - 在日志中追踪特定槽位的生命周期
///
/// ## callback（回调函数）
/// 使用 `Arc<dyn Fn>` 而非 `Box<dyn Fn>`，原因：
/// - **快照发射**：emit 时需要克隆回调引用到快照中，Arc 克隆是 O(1) 的原子操作
/// - **零拷贝共享**：多个快照可以共享同一个回调实例，无需复制闭包
/// - **动态分发**：通过 vtable 调用，有轻微性能开销（~2-5ns）
///
/// ## priority（优先级）
/// 决定执行顺序的字段。详见 [Priority] 文档。
///
/// ## group（分组标签）
/// 可选的字符串标识，用于批量管理。
/// 使用 `Option<String>` 而非空字符串，因为：
/// - 明确语义："无分组" vs "空名字的分组"
/// - 节省内存：None 不分配堆空间
///
/// ## connected（连接状态标志）
/// 与 `Connection` 共享的 `Arc<AtomicBool>`，用于：
/// - 快速判断槽位是否仍处于活跃状态
/// - 支持在 emit 快照中跳过已断开的槽位
/// - 支持在 disconnect_all/disconnect_by_group 中批量更新状态
///
/// ## once（一次性标志）
/// 当为 `true` 时，该槽位在首次成功调用后自动断开。
/// 用于一次性事件监听（如初始化完成通知）。
///
/// # 排序不变量
/// 在信号的槽位列表中，条目始终按 `priority` 降序排列。
/// 这保证了发射时的确定性行为。
///
/// # 相等性语义
/// 两个 SlotEntry 相等当且仅当它们的 `id` 相同。
/// 这是因为 ID 是全局唯一的，可以唯一标识一个连接。
pub struct SlotEntry<Args: 'static> {
    /// 全局唯一的连接标识符
    pub id: ConnectionId,

    /// 用户提供的回调函数（Arc 包装，支持快照发射时低成本克隆）
    ///
    /// # 线程安全约束
    /// 必须实现 `Send + Sync`，因为：
    /// - 可能从不同的线程调用 emit()
    /// - 回调可能在任何线程池中执行
    pub callback: Arc<dyn Fn(&Args) + Send + Sync>,

    /// 执行优先级（影响发射顺序）
    pub priority: Priority,

    /// 可选的分组标签（用于批量管理）
    ///
    /// # 典型值
    /// - `None`: 无分组（默认）
    /// - `Some("ui")`: UI 更新组
    /// - `Some("validators")`: 验证器组
    pub group: Option<String>,

    /// 连接状态标志（与 Connection 共享的 Arc<AtomicBool>）
    ///
    /// # 共享语义
    /// 同一个 Arc 被 SlotEntry 和 Connection 持有，
    /// 任何一方更新状态，另一方都能立即可见（Release-Acquire 语义）。
    pub connected: Arc<AtomicBool>,

    /// 一次性标志：首次成功调用后自动断开
    ///
    /// # 行为
    /// - `false`（默认）：普通槽位，每次 emit 都会调用
    /// - `true`：首次成功调用后，emit 循环会收集其 ID 并在发射结束后移除
    pub once: bool,
}

impl<Args: 'static> SlotEntry<Args> {
    /// 创建新的槽位条目
    ///
    /// # 参数
    /// - `id`: 由 Signal 分配的全局唯一 ID
    /// - `callback`: 用户提供的回调闭包（Arc 包装）
    /// - `priority`: 执行优先级
    /// - `group`: 可选的分组名称
    /// - `connected`: 与 Connection 共享的连接状态标志
    /// - `once`: 是否为一次性槽位
    ///
    /// # 返回值
    /// 一个完全初始化的槽位条目，可直接插入到信号的槽位列表中
    ///
    /// # 注意事项
    /// 此方法通常只由 `Signal::connect_internal()` 内部调用，
    /// 应用层代码不应直接使用。
    pub fn new(
        id: ConnectionId,
        callback: Arc<dyn Fn(&Args) + Send + Sync>,
        priority: Priority,
        group: Option<String>,
        connected: Arc<AtomicBool>,
        once: bool,
    ) -> Self {
        Self { id, callback, priority, group, connected, once }
    }
}

/// 按优先级排序的实现（用于维护排序不变量）
impl<Args: 'static> Ord for SlotEntry<Args> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority.cmp(&other.priority)
    }
}

impl<Args: 'static> PartialOrd for SlotEntry<Args> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 完全相等比较（基于 ID 的唯一性）
impl<Args: 'static> Eq for SlotEntry<Args> {}

impl<Args: 'static> PartialEq for SlotEntry<Args> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// 连接句柄（观察者与主题之间的绑定关系）
///
/// # RAII 设计模式
/// `Connection` 实现了资源获取即初始化（RAII）模式：
/// - **创建时**：建立信号与槽位的绑定关系
/// - **使用时**：可查询状态、显式断开
/// - **销毁时**：（如果实现了 Drop）自动清理资源
///
/// # 所有权模型
/// `Connection` 支持非消费型断开：
/// - `disconnect(&self)` 不消费 self，断开后仍可查询状态
/// - `is_connected()` 在断开后返回 false
/// - 防止重复断开的竞态条件（通过 CAS 原子操作）
///
/// # 内部状态机
/// ```text
///        connect()              disconnect()
/// ┌──────────┐            ┌──────────────┐
/// │ Created  │ ──────────▶│ Disconnected │
/// │ (active) │            │ (inactive)   │
/// └──────────┘            └──────────────┘
///      ▲                        │
///      │                        │
///      └── 重复 disconnect() ───┘
///         （no-op，幂等操作）
/// ```
///
/// # 线程安全保证
///
/// ## 连接状态（connected flag）
/// 使用 `Arc<AtomicBool>` 实现：
/// - 多个线程可同时读取状态（`is_connected()`）
/// - 只有一个线程能成功执行断开（`disconnect()` 的 CAS 操作）
/// - Release-Acquire 语义确保可见性
///
/// ## 断开操作的原子性
/// `disconnect()` 方法通过两步保证原子性：
/// 1. **CAS 操作**：原子地将 connected 从 true 改为 false
/// 2. **条件执行**：只有 CAS 成功才执行实际的移除操作
///
/// 这防止了多个线程同时断开同一连接的问题。
///
/// # PhantomData 的作用
/// `_marker: PhantomData<Args>` 用于：
/// - 将类型参数 `Args` 与结构体关联起来
/// - 使得编译器检查 Args 的生命周期和 trait bounds
/// - 不占用运行时内存（零尺寸类型 ZST）
pub struct Connection<Args: 'static> {
    /// 此连接的唯一标识符
    id: ConnectionId,

    /// 关联的信号名称（用于调试和日志）
    signal_name: &'static str,

    /// 连接状态标志（Arc 共享，支持跨线程状态同步）
    ///
    /// # 为什么用 Arc？
    /// 因为 Connection 和 Signal 的 disconnect_fn 都需要访问这个标志。
    /// Arc 允许多个所有者共享同一个 AtomicBool。
    connected: Arc<AtomicBool>,

    /// 断开操作的闭包（延迟绑定的清理逻辑）
    ///
    /// # 为什么用 Option？
    /// - `Some(fn_)`: 连接活跃，可以断开
    /// - `None`: 已经断开，防止重复操作
    ///
    /// # 为什么用 Arc？
    /// 同上，允许多处持有引用（虽然通常只有 Connection 持有）
    disconnect_fn: Option<Arc<dyn Fn(ConnectionId) + Send + Sync>>,

    /// 类型标记（零成本抽象）
    _marker: std::marker::PhantomData<Args>,
}

impl<Args: 'static> Connection<Args> {
    /// 创建新的连接句柄（内部构造方法）
    ///
    /// # 参数
    /// - `id`: 全局唯一的连接 ID
    /// - `signal_name`: 关联信号的名称
    /// - `connected`: 共享的连接状态标志
    /// - `disconnect_fn`: 执行实际断开操作的闭包
    ///
    /// # 访问级别
    /// `pub(crate)` 表示仅 crate 内部可调用。
    /// 外部代码应通过 `Signal::connect()` 获取 Connection。
    pub(crate) fn new(
        id: ConnectionId,
        signal_name: &'static str,
        connected: Arc<AtomicBool>,
        disconnect_fn: Arc<dyn Fn(ConnectionId) + Send + Sync>,
    ) -> Self {
        Self {
            id,
            signal_name,
            connected,
            disconnect_fn: Some(disconnect_fn),
            _marker: std::marker::PhantomData,
        }
    }

    /// 获取连接的 ID
    ///
    /// # 返回值
    /// 全局唯一的 `ConnectionId`，可用于：
    /// - 日志记录和调试
    /// - 在错误消息中定位问题连接
    /// - 与其他系统进行关联（如数据库外键）
    ///
    /// # 示例
    /// ```ignore
    /// let conn = signal.connect(|data| println!("{:?}", data))?;
    /// println!("连接 ID: {}", conn.id()); // 输出: conn#1
    /// ```
    pub fn id(&self) -> ConnectionId {
        self.id
    }

    /// 获取关联信号的名称
    ///
    /// # 返回值
    /// 创建信号时传入的 `'static str` 名称
    ///
    /// # 使用场景
    /// - 多信号环境下的上下文识别
    /// - 日志中的信号来源标注
    pub fn signal_name(&self) -> &'static str {
        self.signal_name
    }

    /// 检查连接是否仍处于活跃状态
    ///
    /// # 返回值
    /// - `true`: 连接有效，信号发射时会调用此槽位
    /// - `false`: 已断开或正在断开过程中
    ///
    /// # 线程安全
    /// 使用 Acquire 语义加载，确保能看到之前的 Release 写入。
    /// 这意味着：如果另一个线程调用了 disconnect()，
    /// 则当前线程一定能看到 `connected == false`。
    ///
    /// # 典型用法
    /// ```ignore
    /// if !conn.is_connected() {
    ///     println!("连接已断开，需要重新连接");
    /// }
    /// ```
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// 显式断开此连接
    ///
    /// # 行为
    /// 1. 通过 CAS 原子地将状态改为"未连接"
    /// 2. 如果 CAS 成功，调用断开闭包从信号中移除槽位
    /// 3. 不会清空断开闭包引用（因为 &self 不可变借用）
    ///
    /// # 为什么用 &self 而非 mut self？
    /// - **非消费语义**：断开后 Connection 仍可查询状态（is_connected() 返回 false）
    /// - **线程安全**：CAS 操作保证只有一个线程能成功断开
    /// - **便利性**：不需要 mutable 引用即可断开，减少调用者的约束
    ///
    /// # 幂等性保证
    /// 如果连接已经断开（connected == false），此方法是 no-op：
    /// - CAS 会失败（因为当前值已经是 false）
    /// - 不会调用断开闭包
    /// - 不会产生错误
    ///
    /// # 线程安全
    /// 使用 `AcqRel`（_acquire-release_）内存序：
    /// - **Release 部分**：确保断开操作对所有后续线程可见
    /// - **Acquire 部分**：确保能看到之前的所有写操作
    ///
    /// # 示例
    /// ```ignore
    /// let conn = signal.connect(|data| process(data))?;
    /// // ... 后续某个时刻
    /// conn.disconnect(); // 显式断开
    /// assert!(!conn.is_connected()); // 仍可查询状态
    /// ```
    pub fn disconnect(&self) {
        // CAS 操作：尝试原子地从 true 改为 false
        // 只有第一个调用者会成功，后续调用会看到 false 并跳过
        if self.connected.swap(false, Ordering::AcqRel) {
            // CAS 成功：执行实际的断开逻辑
            if let Some(fn_) = &self.disconnect_fn {
                fn_(self.id); // 调用断开闭包
            }
        }
        // CAS 失败：已经被其他地方断开了，什么也不做
    }
}

/// 自动断开实现（RAII）。
///
/// 修复（P2 BUG-connection-no-drop）：lib.rs:198 文档承诺"显式 disconnect()
/// 或 drop 时自动清理"，但旧版未实现 Drop，导致 Connection 在 drop 时
/// 不自动调用 disconnect()，可能造成槽位泄漏（slot 残留在 Signal::slots 中）。
///
/// 现在通过 Drop 调用 disconnect()，借由 CAS 操作保证幂等：
/// - 若已显式 disconnect 过，CAS 失败 → no-op
/// - 若连接仍活跃，CAS 成功 → 调用 disconnect_fn 清理槽位
///
/// # 注意
/// disconnect_fn 是 `Arc<dyn Fn(ConnectionId) + Send + Sync>`，Drop 中调用
/// 是安全的（Fn 不需要 &mut self）。也不会阻塞（disconnect_fn 内部仅做
/// RwLock 写锁 + Vec retain，无长时间运行）。
impl<Args: 'static> Drop for Connection<Args> {
    fn drop(&mut self) {
        self.disconnect();
    }
}
