//! # 错误处理模块
//!
//! 本模块定义了信号槽系统中的所有错误类型。
//!
//! ## 设计原则
//!
//! ### 1. 错误可追溯性
//! 每个 `SignalError` 变体都携带足够的上下文信息（信号名称、类型信息、连接 ID），
//! 使得错误消息可以直接用于日志记录和用户反馈，无需额外的上下文查找。
//!
//! ### 2. 错误分类清晰
//! 采用枚举穷举所有可能的失败模式，编译器会强制调用者处理每种情况，
//! 避免运行时出现未预期的错误路径。
//!
//! ### 3. 与 Rust 生态集成
//! 实现 `std::error::Error` trait，可与 `anyhow`、`thiserror`、
//! `eyre` 等错误处理库无缝配合，也支持 `?` 操作符传播。
//!
//! ## 错误分类图谱
//!
//! ```text
//! SignalError
//! ├── 注册阶段错误
//! │   ├── AlreadyRegistered   (重复注册)
//! │   └── LockPoisoned        (锁中毒)
//! ├── 查找阶段错误
//! │   ├── NotFound            (信号不存在)
//! │   ├── TypeMismatch        (类型不匹配)
//! │   └── LockPoisoned        (锁中毒)
//! ├── 发射阶段错误
//! │   ├── SlotPanicked        (槽位 panic)
//! │   └── EmitFailed          (发射部分失败)
//! └── 系统级错误
//!     └── BusNotInitialized   (已废弃，迁移至 scoped 后不再触发)
//! ```
//!
//! ## 线程安全说明
//!
//! `SignalError` 本身是 `Send + Sync` 的（因为只包含基本类型和 String），
//! 可安全跨线程传递。但注意：某些变体（如 `LockPoisoned`）的出现
//! 通常意味着并发编程存在 bug，需要立即排查。

use crate::types::{ConnectionId, SlotError};
use std::fmt;

/// 信号槽系统的统一错误类型
///
/// # 观察者模式中的错误语义
/// 在经典的观察者模式中，错误处理往往被忽视或简单吞掉。
/// 本系统通过显式的错误类型强制开发者面对以下问题：
/// - **注册冲突**：同一名称+类型的信号不能重复注册（单一真实来源）
/// - **类型安全**：防止在运行时将错误类型的数据传递给信号（编译期保障）
/// - **故障隔离**：单个槽位的 panic 不应导致整个系统崩溃（防御性编程）
///
/// # 错误恢复策略
/// | 错误类型           | 推荐恢复方式                     | 严重程度 |
/// |--------------------|----------------------------------|----------|
/// | AlreadyRegistered  | 复用已有信号或使用不同名称         | 低       |
/// | NotFound           | 先注册再使用                      | 中       |
/// | TypeMismatch       | 检查类型参数是否与注册时一致       | 高       |
/// | SlotPanicked       | 检查槽位代码逻辑，修复后重连       | 高       |
/// | EmitFailed         | 根据 ErrorPolicy 决定是否重试      | 中       |
/// | LockPoisoned       | **紧急**：检查并发访问逻辑         | 致命     |
/// | BusNotInitialized  | *(已废弃)* 迁移至 `SignalBus::scoped()` 后不再触发 | —        |
#[derive(Debug)]
pub enum SignalError {
    /// 信号已注册（名称+类型组合重复）
    ///
    /// # 触发条件
    /// 对同一个 `(name, TypeId)` 组合调用两次 `SignalBus::register()`。
    ///
    /// # 为什么这是错误？
    /// 遵循**单一真实来源（SSOT）**原则：全局总线中每个信号应该有唯一的定义点。
    /// 重复注册通常意味着模块初始化顺序混乱或配置错误。
    ///
    /// # 解决方案
    /// - 使用 `SignalBus::find()` 获取已有信号句柄
    /// - 或使用不同的信号名称区分用途
    AlreadyRegistered {
        /// 已存在的信号名称
        name: &'static str,
    },

    /// 请求的信号未找到
    ///
    /// # 触发条件
    /// 调用 `SignalBus::find()` 时，指定名称的信号尚未注册。
    ///
    /// # 常见原因
    /// 1. 忘记调用 `register()`
    /// 2. 模块加载顺序问题（依赖模块未初始化）
    /// 3. 名称拼写错误
    NotFound {
        /// 未找到的信号名称
        name: &'static str,
    },

    /// 信号类型参数与注册时不匹配
    ///
    /// # 触发条件
    /// 注册时使用 `Signal<i32>`，但查找时使用 `Signal<&str>`。
    /// 虽然名称相同，但 `TypeId` 不同导致匹配失败。
    ///
    /// # 为什么需要这个错误？
    /// Rust 的泛型擦除机制使得运行时必须通过 `TypeId` 进行类型检查。
    /// 此错误在编译期无法捕获（因为是动态查找），但在运行时能快速定位问题。
    ///
    /// # 示例场景
    /// ```ignore
    /// bus.register(&key, &Signal::<i32>::named("data")); // 注册 i32 类型
    /// bus.get::<&str>(&wrong_key)?;                      // 错误！期望 &str
    /// ```
    TypeMismatch {
        /// 信号名称
        name: &'static str,
        /// 注册时的正确类型名（type_name 输出）
        expected: &'static str,
        /// 实际提供的错误类型名
        actual: &'static str,
    },

    /// 槽位回调函数执行时发生 panic
    ///
    /// # 故障隔离机制
    /// 使用 `std::panic::catch_unwind` 捕获槽位的 panic，
    /// 防止单个槽位的错误导致整个信号发射流程崩溃。
    ///
    /// # 何时触发？
    /// - 槽位代码中有 `unwrap()` / `expect()` 断言失败
    /// - 槽位代码显式调用 `panic!()`
    /// - 槽位代码触发了索引越界、空指针解引用等
    ///
    /// # 注意事项
    /// - 仅当槽位的 panic 可以被捕获时才会产生此错误
    /// - 如果 panic 跨越了 FFI 边界或使用了 `panic = "abort"`，进程会直接终止
    SlotPanicked {
        /// 出错的槽位连接 ID（可用于定位问题槽位）
        slot_id: ConnectionId,
        /// Panic 的原始消息（可能是 &str 或 String）
        message: String,
    },

    /// 信号发射过程中部分失败
    ///
    /// # 何时触发？
    /// 当 `EmitOptions.on_error == ErrorPolicy::Stop` 且某个槽位 panic 时，
    /// 或发射过程中遇到锁中毒等不可恢复错误时返回。
    ///
    /// # 字段含义
    /// - `partial_count`: 在失败前成功执行的槽数量
    /// - `errors`: 收集到的错误列表（可能包含多个）
    ///
    /// # 数据一致性考虑
    /// 部分执行的槽位已经产生了副作用（如修改了全局状态），
    /// 调用者需要根据业务逻辑决定是否进行补偿操作。
    EmitFailed {
        /// 成功执行的槽位数量（在第一个错误之前）
        partial_count: usize,
        /// 收集到的错误列表
        errors: Vec<SlotError>,
    },

    /// 全局信号总线未初始化（已废弃）
    ///
    /// # 废弃原因
    /// 迁移至 `SignalBus::scoped()` 后，总线由调用方按需创建，
    /// 不再存在全局单例，因此此变体不可构造。
    ///
    /// # 历史说明
    /// 旧版使用 `GLOBAL_BUS` + `once_cell::sync::Lazy` 全局单例，
    /// 此变体用于全局总线未初始化时的错误报告。
    /// 自 v0.2.0 起不再有触发场景，将在下一个主版本移除。
    #[deprecated(
        since = "0.2.0",
        note = "迁移至 SignalBus::scoped() 后不再有全局总线，此变体不可构造"
    )]
    BusNotInitialized,

    /// 内部锁被污染（Lock Poisoning）
    ///
    /// # 什么是锁污染？
    /// 当持有锁的线程 panic 且锁内部数据可能处于不一致状态时，
    /// Rust 的 `Mutex`/`RwLock` 会将锁标记为"已污染"（poisoned）。
    /// 后续的所有加锁操作都会立即返回 `PoisonError`。
    ///
    /// # 严重程度：**致命**
    /// 这通常意味着：
    /// 1. 并发访问逻辑存在 bug（如死锁、竞态条件）
    /// 2. 槽位代码在持有锁的情况下 panic 了
    /// 3. 系统处于不确定状态，数据完整性无法保证
    ///
    /// # 推荐措施
    /// 1. **立即记录完整堆栈跟踪**
    /// 2. **优雅关闭服务**（不要尝试恢复）
    /// 3. **审查所有持有该锁的代码路径**
    /// 4. **考虑添加 panic 保护**（`catch_unwind` 包裹临界区）
    LockPoisoned {
        /// 发生锁污染的信号名称（辅助定位问题锁）
        name: &'static str,
    },
}

impl fmt::Display for SignalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalError::AlreadyRegistered { name } => {
                write!(f, "signal '{}' is already registered", name)
            }
            SignalError::NotFound { name } => {
                write!(f, "signal '{}' not found", name)
            }
            SignalError::TypeMismatch { name, expected, actual } => {
                write!(f, "signal '{}' type mismatch: expected {}, got {}", name, expected, actual)
            }
            SignalError::SlotPanicked { slot_id, message } => {
                write!(f, "slot {} panicked: {}", slot_id, message)
            }
            SignalError::EmitFailed { partial_count, errors } => {
                write!(
                    f,
                    "emit failed after {}/{} slots: {} error(s)",
                    partial_count,
                    partial_count + errors.len(),
                    errors.len()
                )
            }
            #[allow(deprecated)] // BusNotInitialized 已废弃，保留 Display 实现以维持 API 兼容
            SignalError::BusNotInitialized => {
                write!(f, "signal bus is not initialized")
            }
            SignalError::LockPoisoned { name } => {
                write!(f, "lock poisoned for signal '{}'", name)
            }
        }
    }
}

impl std::error::Error for SignalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SignalError::EmitFailed { errors, .. } => {
                errors.first().map(|e| e as &dyn std::error::Error)
            }
            _ => None,
        }
    }
}
