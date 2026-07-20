//! # Nuzo Signal — Nuzo 类型安全的信号槽系统
//!
//! **层级**: L2（事件总线与可观测性基础设施层）—— 为 GC、编译器、Builtin 等子系统提供类型安全的信号槽通信与日志机制。
//!
//! **主要入口**: [`SignalBus`], [`Signal`], [`Connection`], [`BusScope`], [`SignalKey`], [`SignalLog`]
//!
//! 本 crate 实现了**观察者模式（Observer Pattern）**的 Rust 惯用版本，
//! 提供线程安全、类型安全、高性能的信号-槽（Signal-Slot）机制。
//!
//! ## 核心特性
//!
//! ### 1. 编译期类型安全
//! - 泛型参数 `Args` 确保信号只能发射其声明类型的数据
//! - 连接的槽位必须接受匹配的类型参数
//! - 任何类型不匹配都会在编译期被捕获，而非运行时 panic
//!
//! ### 2. 线程安全设计
//! - 所有核心数据结构实现 `Send + Sync`
//! - 使用 `RwLock` 支持多读者单写者并发模型
//! - 使用 `AtomicBool` / `AtomicU64` 实现无锁快速路径
//! - 故障隔离：单个槽位的 panic 不会影响其他槽位或系统稳定性
//!
//! ### 3. 高性能架构
//! - **零开销抽象**：未启用日志时，emit() 仅消耗几次原子操作
//! - **缓存友好**：槽位存储在连续内存的 Vec 中
//! - **无锁 ID 分配**：使用 `AtomicU64::fetch_add` 避免锁竞争
//! - **延迟格式化**：只在确认需要输出时才执行字符串格式化
//!
//! ### 4. 可观测性支持
//! - 内置日志系统记录所有关键操作（连接、断开、发射）
//! - 可插拔的输出后端（stderr、文件、网络等）
//! - 灵活的过滤机制（按信号名称、事件类型、自定义条件）
//! - 结构化日志条目便于机器解析和监控集成
//!
//! ## 架构概览
//!
//! ```text
//! +-------------------------------------------------------------+
//! |                        应用层代码                             |
//! +-------------------------------------------------------------+
//! |                                                               |
//! |   +----------+    +----------+    +----------+               |
//! |   | Module A |    | Module B |    | Module C |               |
//! |   +----+-----+    +----+-----+    +----+-----+               |
//! |        |               |               |                      |
//! |        v               v               v                      |
//! |   connect()       emit()          find()                     |
//! |        |               |               |                     |
//! +--------+---------------+---------------+---------------------+
//!         v               v               v                     |
//! |   +---------------------------------------------------+     |
//! |   |         SignalBus (作用域限定的信号总线)           |     |
//! |   |  scoped(BusScope::Gc) / scoped(BusScope::Compiler)  |     |
//! |   |  +-----------------------------------------+     |     |
//! |   |  |    HashMap<SignalKey<Args>, Signal>      |     |     |
//! |   |  +-----------------------------------------+     |     |
//! |   +---------------------------------------------------+     |
//! |                                                           |
//! |   +---------------------------------------------------+     |
//! |   |            Signal<Args> (观察者主题)              |     |
//! |   |  +-----------------------------------------+     |     |
//! |   |  |     Vec<SlotEntry> (有序槽位列表)        |     |     |
//! |   |  |  [High Priority] -> [Normal] -> [Low]    |     |     |
//! |   |  +-----------------------------------------+     |     |
//! |   +---------------------------------------------------+     |
//! |                                                           |
//! |   +---------------------------------------------------+     |
//! |   |          SignalLog (可观测性层)                    |     |
//! |   |  [启用标志] -> [过滤器] -> [写入器] -> 输出      |     |
//! |   +---------------------------------------------------+     |
//! |                                                           |
//! +-------------------------------------------------------------+
//! ```
//!
//! ## 模块结构
//!
//! | 模块 | 文件 | 职责 | 核心类型 |
//! |------|------|------|----------|
//! | `types` | types.rs | 基础类型与事件负载 | `ConnectionId`, `Priority`, `EmitResult`, `BusScope`, `SignalKey` |
//! | `error` | error.rs | 错误处理 | `SignalError` |
//! | `log` | log.rs | 日志与可观测性 | `SignalLog`, `LogEvent` |
//! | `signal` | signal.rs | 信号机制（Subject） | `Signal<Args>` |
//! | `slot` | slot.rs | 槽位与连接（Observer） | `SlotEntry`, `Connection` |
//! | `bus` | bus.rs | 作用域限定的事件总线 | `SignalBus` |
//!
//! ## 快速开始
//!
//! ### 1. 创建作用域总线并注册信号
//! ```ignore
//! use nuzo_signal::*;
//!
//! // 定义信号载荷类型
//! #[derive(Debug, Clone)]
//! struct UserData { name: String, age: u32 }
//!
//! // 创建作用域限定的信号总线
//! let gc_bus = SignalBus::scoped(BusScope::Gc);
//! let compiler_bus = SignalBus::scoped(BusScope::Compiler);
//!
//! // 创建信号实例
//! let user_changed = Signal::<UserData>::named("user:changed");
//!
//! // 注册到作用域总线
//! let key: SignalKey<UserData> = SignalKey::new("user:changed", BusScope::Gc);
//! gc_bus.register(&key, &user_changed).unwrap();
//! ```
//!
//! ### 2. 连接槽位（订阅）
//! ```ignore
//! // 从总线获取信号并连接槽位
//! let key: SignalKey<UserData> = SignalKey::new("user:changed", BusScope::Gc);
//! let signal = gc_bus.get(&key).unwrap();
//!
//! // 连接标准优先级槽位
//! signal.connect(|data| {
//!     println!("用户变更: {} ({}岁)", data.name, data.age);
//! });
//!
//! // 连接高优先级验证器
//! signal.connect_with_priority(|data| {
//!     assert!(data.age > 0, "年龄必须大于0");
//! }, Priority::High(0));
//!
//! // 连接带分组的槽位
//! signal.connect_with_group(|data| {
//!     log::info!("审计日志: 用户 {}", data.name);
//! }, "audit");
//! ```
//!
//! ### 3. 发射信号（发布）
//! ```ignore
//! let signal = gc_bus.get(&key).unwrap();
//!
//! let result = signal.emit(&UserData {
//!     name: "Alice".to_string(),
//!     age: 30,
//! });
//!
//! if !result.is_ok() {
//!     eprintln!("{} 个槽位执行失败", result.errors.len());
//! }
//! ```
//!
//! ### 4. 启用日志（可选）
//! ```ignore
//! use nuzo_signal::*;
//!
//! // 启用全局日志
//! SignalLog::global().enable();
//!
//! // 设置过滤器：只记录 GC 相关信号
//! SignalLog::global().with_filter(|entry| {
//!     entry.signal_name.starts_with("gc:");
//! });
//!
//! // 设置自定义写入器（如文件）
//! // SignalLog::global().with_writer(file);
//! ```
//!
//! ## 作用域限定 (Scoped SignalBus)
//!
//! 每条 `SignalBus` 都通过 `BusScope` 标记其所属子系统，
//! 避免不同子系统的同名信号产生冲突。
//!
//! ### BusScope 变体
//!
//! | 变体 | 用途 | 典型事件负载 |
//! |------|------|-------------|
//! | `Gc` | 垃圾回收 | `GcWillCollectInfo`, `GcDidCollectInfo` |
//! | `Compiler` | 编译器 | `CompileStartedInfo`, `CompileFinishedInfo`, `ScopeEnteredInfo`, `ScopeExitedInfo` |
//! | `Builtin` | 内置函数 | `BuiltinCallInfo` |
//! | `Custom(&'static str)` | 用户自定义 | 任意 |
//!
//! ### SignalKey
//!
//! `SignalKey<Args>` 将 **名称 + 作用域 + 参数类型** 绑定为编译期可验证的整体：
//! - 同名信号在不同作用域下互不干扰
//! - 泛型参数确保发射与连接的类型一致性
//! - `Copy` 语义，零成本传递
//!
//! ## 设计模式说明
//!
//! ### 观察者模式（Observer Pattern）
//! 经典的发布-订阅实现：
//! - **Subject**: `Signal<Args>` - 维护观察者列表并通知变化
//! - **Observer**: 通过闭包实现的槽位回调
//! - **绑定关系**: `Connection<Args>` - 管理 Observer 的生命周期
//!
//! ### 中介者模式（Mediator Pattern）
//! `SignalBus` 作为中介者解耦信号的创建者和使用者：
//! - 生产者不需要知道消费者的存在
//! - 消费者不需要持有生产者的引用
//! - 通过作用域限定的总线进行松耦合通信
//!
//! ### RAII 资源管理
//! `Connection` 对象遵循 Rust 的所有权语义：
//! - 创建时建立绑定
//! - 显式 disconnect() 或 drop 时自动清理
//! - 防止资源泄漏和悬垂回调
//!
//! ## 线程安全性保证
//!
//! ### 并发操作矩阵
//! | 操作 A | 操作 B | 安全性 | 说明 |
//! |--------|--------|--------|------|
//! | emit (读锁) | emit (读锁) | 安全 | 多个线程可同时发射 |
//! | emit (读锁) | connect (写锁) | 安全 | 会短暂阻塞 |
//! | connect (写锁) | connect (写锁) | 安全 | 串行化执行 |
//! | disconnect | emit | 安全 | 原子状态更新 |
//!
//! ### 内存序保证
//! - **Release-Acquire**: 用于连接/断开状态的同步
//! - **Relaxed**: 用于 ID 分配（仅要求唯一性）
//! - **SeqCst**: 未使用（避免不必要的性能开销）
//!
//! ## 性能基准（参考值）
//!
//! 在典型硬件（Intel i7-12700K）上的粗略测量：
//!
//! | 操作 | 耗时 | 条件 |
//! |------|------|------|
//! | connect() | ~1-5us | 包含排序 |
//! | emit() (无槽位) | ~10-50ns | 快速路径 |
//! | emit() (1个槽位) | ~50-100ns | 直接调用 |
//! | emit() (100个槽位) | ~5-20us | 取决于槽位复杂度 |
//! | find() | ~100-500ns | HashMap 查找 + downcast |
//!
//! > 注：实际性能取决于具体使用场景和硬件配置。
//!
//! ## 错误处理策略
//!
//! 本 crate 遵循**显式错误处理**原则：
//! - 不使用 unwrap()/expect()（除非逻辑上不可能失败）
//! - 所有错误通过 `Result<T, SignalError>` 传播
//! - 提供详细的错误上下文（信号名称、类型信息、连接 ID）
//! - 实现 `std::error::Error` trait 以便与其他错误处理库集成
//!
//! ## 适用场景
//!
//! - **GUI 事件系统**: UI 组件间的松耦合通信
//! - **游戏开发**: 实体组件系统（ECS）的事件总线
//! - **微服务架构**: 进程内的事件驱动架构
//! - **插件系统**: 主程序与插件之间的通信
//! - **VM/解释器**: 指令执行事件的追踪和钩子

// Crate 元数据——外层属性形式（`#![inner_attr]` 在 stable Rust 不稳定）
#[nuzo_proc::crate_meta(layer = 2, description = "信号总线基础设施", entry_type = "SignalBus")]
const _NUZO_CRATE_META_ANCHOR: () = ();

// 内部模块声明
pub mod bus; // 全局事件总线（SignalBus）
pub mod error; // 错误类型（SignalError）
pub mod log;
pub mod signal; // 核心信号机制（Signal<Args>）
pub mod slot; // 槽位与连接（SlotEntry, Connection）
pub mod slot_stats; // ASB: 槽位热度统计与批量发射缓冲
pub mod types; // 基础类型定义（ConnectionId, Priority 等） // 日志系统（SignalLog, LogEvent）

// 公共导出：显式列出每个模块的公共 API，避免通配符重导出
//
// 设计原则：显式导出让 crate 的公共 API 表面积一目了然，
// 新增/删除导出项时必须在此处同步更新，防止意外泄露内部实现。

// ── types: 信号基础设施 ──────────────────────────────────────────
pub use types::ConnectionId;
pub use types::EmitOptions;
pub use types::EmitResult;
pub use types::ErrorPolicy;
pub use types::Priority;
pub use types::SlotError;

// ── types: 总线作用域 ────────────────────────────────────────────
pub use types::BusScope;
pub use types::SignalKey;

// ── types: 事件负载 — GC ────────────────────────────────────────
pub use types::GcDidCollectInfo;
pub use types::GcWillCollectInfo;

// ── types: 事件负载 — Compiler ──────────────────────────────────
pub use types::CompileFinishedInfo;
pub use types::CompileStartedInfo;
pub use types::FunctionCompileInfo;
pub use types::ScopeEnteredInfo;
pub use types::ScopeExitedInfo;

// ── types: 事件负载 — Builtin ──────────────────────────────────
pub use types::BuiltinCallInfo;

// ── types: 事件负载 — 寄存器分配 ────────────────────────────────
pub use types::SlotConflictedInfo;
pub use types::SlotOwner;
pub use types::SlotReleasedInfo;
pub use types::SlotReservedInfo;

// ── types: 事件负载 — VM（VmObserver 回调用）────────────────────
pub use types::VmErrorInfo;

// ── error 模块导出 ──────────────────────────────────────────────
pub use error::SignalError;

// ── signal 模块导出 ─────────────────────────────────────────────
pub use signal::Signal;

// ── slot 模块导出 ───────────────────────────────────────────────
pub use slot::Connection;
pub use slot::SlotEntry;
// 注：`SlotStats` 与 `SlotTier` 由 `slot_stats` 模块统一定义并导出，
// `slot` 模块内的同名定义为内部实现细节，不在顶级 re-export。

// ── slot_stats 模块导出（ASB: Adaptive Slot Batching）─────────
pub use slot_stats::BATCH_MAX_SIZE;
pub use slot_stats::BATCH_THRESHOLD;
pub use slot_stats::EmitBatch;
pub use slot_stats::SlotStats;
pub use slot_stats::SlotTier;

// ── bus 模块导出 ────────────────────────────────────────────────
pub use bus::SignalBus;

// ── log 模块导出 ────────────────────────────────────────────────
pub use log::LogEntry;
pub use log::LogEvent;
pub use log::SignalLog;
