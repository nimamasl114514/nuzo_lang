//! # 日志系统模块
//!
//! 本模块提供信号槽系统的可观测性支持，记录所有关键操作（连接、断开、发射）。
//!
//! ## 设计目标
//!
//! ### 1. 零开销默认禁用
//! 日志系统默认处于**关闭状态**，通过 `AtomicBool` 的快速路径检查，
//! 在未启用时仅消耗几条 CPU 指令（一次原子加载 + 条件跳转）。
//!
//! ### 2. 可插拔的输出后端
//! 支持自定义日志写入器（`LogWriter`），可将日志输出到：
//! - 标准错误流（默认）
//! - 文件系统
//! - 网络服务
//! - 内存缓冲区（用于测试）
//!
//! ### 3. 灵活的过滤机制
//! 通过 `LogFilter` 闭包实现细粒度的日志过滤：
//! - 按信号名称过滤（如只记录 "vm:*" 相关信号）
//! - 按事件类型过滤（如只记录发射事件）
//! - 按时间窗口过滤（如只记录最近 5 分钟的日志）
//! - 自定义复杂条件组合
//!
//! ## 架构设计
//!
//! ```text
//! Signal/Bus 操作
//!       │
//!       ▼
//! ┌─────────────┐     enabled?      ┌──────────────┐
//! │  LogEntry   │ ──── No ──────▶  │   (丢弃)      │
//! │  (创建)     │                   └──────────────┘
//! └──────┬──────┘
//!        │ Yes
//!        ▼
//! ┌─────────────┐     filter?       ┌──────────────┐
//! │  过滤器检查  │ ──── No ──────▶  │   (丢弃)      │
//! └──────┬──────┘                   └──────────────┘
//!        │ Yes (通过)
//!        ▼
//! ┌─────────────┐
//! │  写入器输出  │ ──▶ stderr / 文件 / 网络 ...
//! └─────────────┘
//! ```
//!
//! ## 线程安全性
//!
//! - **全局单例**：使用 `once_cell::sync::Lazy` 实现线程安全的懒初始化
//! - **启用/禁用标志**：`AtomicBool` 无锁操作，支持高并发读写
//! - **过滤器**：`RwLock` 保护，读多写少场景优化
//! - **写入器**：`RwLock` 保护，每次写入时短暂持有写锁
//!
//! ## 性能影响评估
//!
//! | 操作           | 开销（未启用） | 开销（已启用无过滤器） | 开销（已启用有过滤器） |
//! |----------------|---------------|----------------------|----------------------|
//! | 创建 LogEntry  | ~50ns         | ~50ns                | ~50ns                |
//! | enabled 检查   | ~2-5ns        | ~2-5ns               | ~2-5ns               |
//! | 过滤器执行     | 0             | 0                    | ~100ns-1us          |
//! | 写入操作       | 0             | ~10-100us            | ~10-100us           |
//!
//! > 注：以上为粗略估计，实际性能取决于硬件和负载。

use std::fmt;
use std::io::{self, Write};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use web_time::{Duration, Instant};

use once_cell::sync::Lazy;

use crate::types::ConnectionId;

/// 日志事件类型枚举
///
/// # 观察者模式中的审计追踪
/// 每种事件类型对应观察者模式的一个生命周期阶段：
/// - `Connect`: 观察者注册（Subject.attach(observer)）
/// - `Disconnect`: 观察者注销（Subject.detach(observer)）
/// - `Emit`: 通知广播（Subject.notify(args)）
///
/// # 使用场景
/// - **调试**：追踪信号的连接/断开时机，排查内存泄漏或重复订阅
/// - **性能分析**：通过 Emit 事件的耗时和槽数量识别热点信号
/// - **监控告警**：检测异常高的错误率或超长延迟
#[derive(Debug, Clone)]
pub enum LogEvent {
    /// 信号发射完成
    ///
    /// # 字段说明
    /// - `slot_count`: 当前连接的总槽数量（用于计算成功率）
    /// - `error_count`: 本次发射中失败的槽数量
    /// - `elapsed`: 发射总耗时（包含锁等待、排序、执行等全部时间）
    ///
    /// # 性能指标示例
    /// 如果 `error_count > 0 且 error_count/slot_count > 0.5`，
    /// 可能意味着信号的业务逻辑存在系统性问题。
    Emit {
        /// 连接的槽位总数
        slot_count: usize,
        /// 执行失败的槽位数
        error_count: usize,
        /// 发射耗时
        elapsed: Duration,
    },

    /// 新槽位连接建立
    ///
    /// # 使用场景
    /// 追踪动态订阅模式：某些槽位可能在运行时根据条件才连接。
    /// 通过 Connect 事件可以验证订阅逻辑是否按预期执行。
    Connect {
        /// 新分配的连接 ID
        slot_id: ConnectionId,
    },

    /// 槽位断开连接
    ///
    /// # 触发时机
    /// - 显式调用 `Connection::disconnect()`
    /// - 调用 `Signal::disconnect_by_group()` 批量断开
    /// - 调用 `Signal::disconnect_all()` 清空所有连接
    /// - Connection 被 drop 时自动断开（如果实现了 Drop）
    Disconnect {
        /// 断开的连接 ID
        slot_id: ConnectionId,
    },
}

/// 单条日志条目
///
/// # 结构化日志设计
/// 采用结构化字段而非格式化字符串，便于：
/// - **机器解析**：JSON 序列化后可直接被 ELK、Splunk 等日志平台消费
/// - **上下文关联**：通过 `signal_name` 快速筛选特定信号的日志
/// - **时间线重建**：`timestamp` 支持精确的事件顺序还原
///
/// # 时间戳语义
/// `timestamp` 使用 `Instant::now()` 记录的是**相对时间**
///（自程序启动以来的单调时钟），而非墙钟时间。
/// 这避免了系统时间调整导致的乱序问题。
///
/// # Display 格式
/// 输出格式：`[+{elapsed_ms}ms] signal '{name}' {event_type}: {details}`
///
/// 示例：
/// ```text
/// [+1234ms] signal 'vm:execute' emit: 5 slots, 0 errors, 0.52ms
/// [+1235ms] signal 'vm:execute' connect: conn#42
/// ```
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// 关联的信号名称（静态字符串，零拷贝）
    pub signal_name: &'static str,

    /// 事件发生的时间点（单调时钟）
    pub timestamp: Instant,

    /// 具体的事件类型和数据
    pub event: LogEvent,
}

impl fmt::Display for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = self.timestamp.elapsed().as_millis();
        match &self.event {
            LogEvent::Emit { slot_count, error_count, elapsed } => {
                write!(
                    f,
                    "[+{}ms] signal '{}' emit: {} slots, {} errors, {:.2}ms",
                    ts,
                    self.signal_name,
                    slot_count,
                    error_count,
                    elapsed.as_secs_f64() * 1000.0
                )
            }
            LogEvent::Connect { slot_id } => {
                write!(f, "[+{}ms] signal '{}' connect: {}", ts, self.signal_name, slot_id)
            }
            LogEvent::Disconnect { slot_id } => {
                write!(f, "[+{}ms] signal '{}' disconnect: {}", ts, self.signal_name, slot_id)
            }
        }
    }
}

/// 日志过滤器类型别名
///
/// # 设计意图
/// 将复杂的泛型签名封装为易读的类型别名，
/// 降低 API 使用者的认知负担。
///
/// # 线程安全约束
/// 过滤器必须实现 `Send + Sync`，因为：
/// - 可能在多个线程中并发调用（不同信号可能在不同线程发射）
/// - 存储在全局静态变量中，必须满足 `'static` 生命周期
type LogFilter = Box<dyn Fn(&LogEntry) -> bool + Send + Sync>;

/// 日志写入器类型别名
///
/// # 实现要求
/// 必须实现 `Write` trait + `Send + Sync`，
/// 常见选择包括：
/// - `io::Stderr`（默认）
/// - `io::BufWriter<File>`（文件输出）
/// - `Vec<u8>`（内存缓冲，用于测试）
/// - 自定义网络写入器（需处理部分写入和重连逻辑）
type LogWriter = Box<dyn Write + Send + Sync>;

/// 全局信号日志管理器
///
/// # 单例模式
/// 使用 `once_cell::sync::Lazy` 实现线程安全的懒初始化单例。
/// 全局唯一实例确保所有信号的日志集中管理，避免配置分散。
///
/// # 观察者模式中的角色
/// `SignalLog` 是观察者模式的**横切关注点（Cross-Cutting Concern）**：
/// 它不参与信号-槽的核心逻辑，但为整个系统提供可观测性。
/// 类似于 AOP（面向切面编程）中的"日志切面"。
///
/// # 典型工作流程
/// ```ignore
/// // 1. 启动时配置（可选）
/// SignalLog::global().enable();
/// SignalLog::global().with_filter(|entry| {
///     entry.signal_name.starts_with("vm:") // 只记录 VM 相关信号
/// });
///
/// // 2. 运行时自动记录（由 Signal/Bus 内部调用）
/// let result = signal.emit(&data); // 自动生成 LogEvent::Emit
///
/// // 3. 运行时调整（可选）
/// SignalLog::global().disable(); // 动态关闭日志
/// ```
///
/// # 线程安全性保证
///
/// ## 并发访问矩阵
/// | 操作              | 内部机制         | 阻塞风险 |
/// |-------------------|------------------|----------|
/// | enable/disable    | AtomicBool       | 无       |
/// | is_enabled        | AtomicBool       | 无       |
/// | with_filter       | RwLock (write)   | 低       |
/// | clear_filter      | RwLock (write)   | 低       |
/// | with_writer       | RwLock (write)   | 低       |
/// | log()             | RwLock (read×2)  | 中       |
///
/// ## 锁顺序规则
/// 为防止死锁，`log()` 方法内部严格按照以下顺序获取锁：
/// 1. `filter.read()` （先读取过滤器）
/// 2. `writer.write()` （再获取写入器写锁）
///
/// **禁止**在其他位置以相反顺序获取这两把锁！
///
/// # 性能优化策略
///
/// ## 1. 快速路径优化
/// ```ignore
/// if !self.enabled.load(Ordering::Acquire) { return; } // 未启用直接返回
/// ```
/// 这行代码在大多数情况下（日志关闭）仅需 ~3ns。
///
/// ## 2. 延迟格式化
/// 只有在确认日志会被输出后才执行 `writeln!()` 格式化，
/// 避免了不必要的字符串分配。
///
/// ## 3. 错误静默处理
/// 写入失败时使用 `let _ = ...` 忽略错误，
/// 因为日志写入失败不应影响业务逻辑执行。
pub struct SignalLog {
    /// 日志启用标志（原子布尔值，无锁快速路径）
    enabled: AtomicBool,

    /// 可选的日志过滤器（None 表示不过滤）
    ///
    /// 使用 RwLock 因为：
    /// - 读操作远多于写操作（每次 log() 都要读取）
    /// - 写操作仅在配置变更时发生（低频）
    filter: RwLock<Option<LogFilter>>,

    /// 日志输出目标（默认为 stderr）
    ///
    /// 使用 RwLock 写锁因为 Write trait 需要 &mut self。
    /// 每次写入都会短暂阻塞其他写入操作，但持续时间极短（微秒级）。
    writer: RwLock<LogWriter>,
}

/// 全局日志单例实例
///
/// # 初始化时机
/// 在首次调用 `SignalLog::global()` 时惰性创建。
/// 初始化过程是线程安全的（`once_cell` 保证）。
///
/// # 默认配置
/// - **启用状态**：关闭（`enabled = false`）
/// - **过滤器**：无（接受所有日志）
/// - **输出目标**：标准错误流（`io::stderr()`）
static GLOBAL_LOG: Lazy<SignalLog> = Lazy::new(|| SignalLog {
    enabled: AtomicBool::new(false),
    filter: RwLock::new(None),
    writer: RwLock::new(Box::new(io::stderr())),
});

impl SignalLog {
    /// 获取全局日志单例的引用
    ///
    /// # 返回值
    /// 静态生命周期的引用，可在任何地方安全使用。
    ///
    /// # 线程安全
    /// 多次调用返回相同的引用，且初始化过程是原子的。
    pub fn global() -> &'static SignalLog {
        &GLOBAL_LOG
    }

    /// 启用日志记录
    ///
    /// # 效果
    /// 后续的所有信号操作（connect/disconnect/emit）都会产生日志条目。
    ///
    /// # 内存序保证
    /// 使用 `Release` 语义确保之前的所有写操作对其他线程可见。
    /// 这保证了：如果另一个线程看到 `enabled == true`，
    /// 则它一定能看到完整的日志配置（filter 和 writer）。
    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    /// 禁用日志记录
    ///
    /// # 效果
    /// 后续的 `log()` 调用会在第一条检查处立即返回，零额外开销。
    ///
    /// # 典型用法
    /// 生产环境默认关闭，仅在诊断问题时临时开启：
    /// ```ignore
    /// // 收到 SIGUSR1 信号时开启日志
    /// SignalLog::global().enable();
    /// // 30秒后自动关闭
    /// std::thread::spawn(|| {
    ///     std::thread::sleep(Duration::from_secs(30));
    ///     SignalLog::global().disable();
    /// });
    /// ```
    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    /// 检查日志是否已启用
    ///
    /// # 返回值
    /// - `true`: 日志系统正在运行，会记录后续操作
    /// - `false`: 日志系统已关闭，所有 log() 调用将被忽略
    ///
    /// # 内存序
    /// 使用 `Acquire` 语义确保能看到 `enable()` 时的 Release 写入。
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// 设置日志过滤器
    ///
    /// # 参数
    /// `filter`: 一个接收 `&LogEntry` 返回 bool 的闭包
    /// - 返回 `true`: 该条目会被输出
    /// - 返回 `false`: 该条目会被静默丢弃
    ///
    /// # 线程安全
    /// 此方法会获取写锁，如果在高频调用期间设置过滤器，
    /// 可能会造成短暂的 log() 阻塞（通常 < 1us）。
    ///
    /// # 示例：只记录错误
    /// ```ignore
    /// SignalLog::global().with_filter(|entry| {
    ///     matches!(entry.event, LogEvent::Emit { error_count: n, ..} if n > 0)
    /// });
    /// ```
    ///
    /// # 示例：只记录特定信号
    /// ```ignore
    /// SignalLog::global().with_filter(|entry| {
    ///     entry.signal_name.starts_with("gc:")
    /// });
    /// ```
    ///
    /// # 注意事项
    /// - 旧的过滤器会被立即替换（不支持链式组合）
    /// - 如果需要复杂过滤逻辑，请在闭包内部实现
    /// - 闭包必须捕获 `'static` 数据（不能借用局部变量）
    pub fn with_filter<F: Fn(&LogEntry) -> bool + Send + Sync + 'static>(&self, filter: F) {
        if let Ok(mut f) = self.filter.write() {
            *f = Some(Box::new(filter));
        }
    }

    /// 清除当前过滤器（恢复为接受所有日志）
    ///
    /// # 效果
    /// 清除后，所有启用的日志条目都会被输出（不再过滤）。
    pub fn clear_filter(&self) {
        if let Ok(mut f) = self.filter.write() {
            *f = None;
        }
    }

    /// 设置自定义日志写入器
    ///
    /// # 参数
    /// `writer`: 实现了 `Write + Send + Sync` 的任意类型
    ///
    /// # 常见用法
    /// ```ignore
    /// // 写入文件
    /// use std::fs::OpenOptions;
    /// let file = OpenOptions::new()
    ///     .create(true)
    ///     .append(true)
    ///     .open("signal.log")
    ///     .unwrap();
    /// SignalLog::global().with_writer(file);
    ///
    /// // 写入内存（用于测试）
    /// let buffer: Arc<RwLock<Vec<u8>>> = Default::default();
    /// // 需要实现一个包装类型...
    /// ```
    ///
    /// # 线程安全注意事项
    /// 写入器的 `write()` 方法可能在多个线程中并发调用。
    /// 如果底层的 Write 实现不是线程安全的（如 `File`），
    /// 外部必须自行添加同步机制（RwLock 已提供保护）。
    ///
    /// # 错误处理
    /// 写入失败时不会 panic 或返回错误，而是静默忽略。
    /// 这是因为日志丢失不应影响核心业务逻辑。
    pub fn with_writer<W: Write + Send + Sync + 'static>(&self, writer: W) {
        if let Ok(mut w) = self.writer.write() {
            *w = Box::new(writer);
        }
    }

    /// 记录一条日志条目
    ///
    /// # 这是核心方法
    /// 由 `Signal` 和 `SignalBus` 的各个方法在关键操作点调用。
    /// 应用层代码通常不需要直接调用此方法。
    ///
    /// # 执行流程
    /// 1. **快速路径检查**：如果日志未启用，立即返回（~3ns）
    /// 2. **过滤器评估**：如果有过滤器且条目不匹配，丢弃（~100ns-1us）
    /// 3. **格式化输出**：将 LogEntry 格式化为字符串并写入（~10-100us）
    ///
    /// # 性能特征
    /// - **最佳情况**（未启用）：~3ns（原子加载 + 分支预测）
    /// - **一般情况**（启用，无过滤器）：~10-100us（主要是 I/O 时间）
    /// - **最坏情况**（启用，复杂过滤器）：~100us-1ms（取决于过滤器复杂度）
    ///
    /// # 错误处理策略
    /// 所有 I/O 错误都被静默忽略（`let _ = writeln!(...)`)。
    /// 理由：
    /// 1. 日志是辅助功能，不应影响业务逻辑正确性
    /// 2. 写入失败通常是临时性的（磁盘满、网络抖动）
    /// 3. 向上传播错误会增加 API 复杂度（emit/connect 都需要返回 Result）
    ///
    /// # 死锁预防
    /// 此方法严格按顺序获取两把锁：
    /// 1. `filter.read()` （读锁）
    /// 2. `writer.write()` （写锁）
    ///
    /// **重要**：不要在任何其他方法中以相反顺序获取这些锁！
    pub fn log(&self, entry: LogEntry) {
        // 快速路径：未启用则直接返回（原子操作，无锁）
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }

        // 第二层过滤：应用用户定义的过滤器（读锁，允许并发读取）
        if let Ok(filter) = self.filter.read()
            && let Some(ref f) = *filter
            && !f(&entry)
        {
            return; // 过滤器拒绝此条目
        }

        // 最终输出：格式化并写入（写锁，短暂持有）
        if let Ok(mut writer) = self.writer.write() {
            let _ = writeln!(writer, "{}", entry); // 静默忽略 I/O 错误
        }
    }
}
