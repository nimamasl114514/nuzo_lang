//! # ErrorSink — VM → ErrorCollector 反向事件流通道
//!
//! 设计动机：nuzo_error 不能依赖 nuzo_vm（分层约束），但 VM 需要把
//! 运行时错误事件传递给 ErrorCollector。ErrorSink trait 定义在 nuzo_error，
//! nuzo_vm 实现 adapter（ErrorSinkObserver）将 VmErrorInfo 转为 ErrorEvent
//! 后调用 sink_error。
//!
//! 事件流方向：
//! ```text
//! nuzo_vm::VmErrorInfo ──(adapter)──▶ ErrorEvent ──▶ ErrorSink::sink_error
//!                                                         │
//!                                          ErrorCollector (impl ErrorSink)
//!                                                         │
//!                                          drain_sunk() ──▶ DiagnosticError
//! ```
//!
//! 注意：ErrorEvent → DiagnosticError 的完整转换在 ErrorCollector::drain_sunk
//! 中执行，因为该转换需要 ErrorCollector 持有的 id 计数器、ExecutionContext
//! 等上下文，ErrorEvent 本身不携带这些信息。

/// 错误事件载体 - VM 运行时产生的错误事件抽象表示
///
/// 不依赖 nuzo_signal::VmErrorInfo，避免跨 crate 类型耦合。
/// nuzo_vm 的 ErrorSinkObserver adapter 负责 VmErrorInfo → ErrorEvent 转换。
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorEvent {
    /// 错误消息（已格式化）
    pub message: String,
    /// 触发错误的 opcode（若可识别）
    pub opcode: Option<u8>,
    /// 触发错误的指令位置
    pub ip: usize,
    /// 调用栈深度
    pub call_depth: usize,
}

impl ErrorEvent {
    /// 创建一个仅包含消息的最小错误事件
    ///
    /// 其余字段使用默认值，可通过 Builder 方法补充：
    ///
    /// ```rust,ignore
    /// let event = ErrorEvent::new("type mismatch".into())
    ///     .with_opcode(0x42)
    ///     .with_ip(128)
    ///     .with_call_depth(3);
    /// ```
    pub fn new(message: String) -> Self {
        Self { message, opcode: None, ip: 0, call_depth: 0 }
    }

    /// 设置触发错误的 opcode（Builder 链式）
    pub fn with_opcode(mut self, opcode: u8) -> Self {
        self.opcode = Some(opcode);
        self
    }

    /// 设置触发错误的指令位置（Builder 链式）
    pub fn with_ip(mut self, ip: usize) -> Self {
        self.ip = ip;
        self
    }

    /// 设置调用栈深度（Builder 链式）
    pub fn with_call_depth(mut self, call_depth: usize) -> Self {
        self.call_depth = call_depth;
        self
    }
}

/// 错误接收器 trait - 接收 VM 错误事件
///
/// 实现者：
/// - `ErrorCollector`（在 collector.rs 中实现，用 crossbeam_queue::SegQueue 无锁缓冲）
/// - 测试用 mock sink
///
/// # 设计要点
///
/// - `&self` 而非 `&mut self`，允许在共享引用下接收事件（配合内部可变性）
/// - `Send + Sync` 约束，允许跨线程传递
/// - 不返回 `Result`，错误接收总是成功（队列满时丢弃，由实现者记录）
pub trait ErrorSink: Send + Sync {
    /// 接收一个错误事件
    ///
    /// 实现者应将事件存入内部缓冲，供后续 `drain_sunk` 提取并转为 `DiagnosticError`。
    fn sink_error(&self, event: ErrorEvent);
}
