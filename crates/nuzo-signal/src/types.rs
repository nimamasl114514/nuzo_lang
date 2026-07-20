//! # 类型定义模块
//!
//! 本模块定义了 `nuzo_signal` 信号槽系统的核心数据类型。
//!
//! ## 设计哲学
//!
//! 采用**强类型 + Newtype 模式**，通过类型系统在编译期消除非法状态：
//! - `ConnectionId` 使用 Newtype 包装，避免与普通 u64 混淆
//! - `Priority` 使用枚举穷举所有优先级状态，防止非法值
//! - `ErrorPolicy` 明确错误处理策略，拒绝隐式默认行为
//!
//! ## 线程安全性
//!
//! 所有类型均实现 `Send + Sync`，支持跨线程安全传递。
//! 原子操作和锁的使用确保无数据竞争（详见各字段文档）。

use std::fmt;
use std::marker::PhantomData;

/// 连接标识符（Newtype 包装）
///
/// # 角色
/// 唯一标识信号与槽位之间的连接关系。每次调用 `Signal::connect()` 时，
/// 系统会自动分配一个全局递增的 ID。
///
/// # 为什么用 Newtype？
/// - **类型安全**：防止将连接 ID 误用为其他 u64 值（如用户 ID、时间戳）
/// - **语义清晰**：`ConnectionId(42)` 比 `42u64` 更具表达力
/// - **可扩展性**：未来可添加验证逻辑而不破坏 API
///
/// # 线程安全
/// ID 分配使用 `AtomicU64::fetch_add`，保证全局唯一且无竞争。
///
/// # 示例
/// ```ignore
/// let conn = signal.connect(|args| println!("{:?}", args));
/// println!("连接 ID: {}", conn.id()); // 输出: conn#1
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    /// 获取底层 u64 值
    ///
    /// # 使用场景
    /// 需要将 ID 序列化、存储到数据库、或与外部系统交互时使用。
    /// 内部代码应优先使用 `ConnectionId` 类型以保持类型安全。
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn#{}", self.0)
    }
}

/// 槽位执行优先级
///
/// # 观察者模式中的排序策略
/// 当信号触发时，槽位按优先级从高到低依次执行。
/// 这实现了**责任链模式**的变体：高优先级槽位可以"拦截"或预处理事件。
///
/// # 优先级数值映射
/// | 变体       | order_value() 范围   | 含义                     |
/// |------------|----------------------|--------------------------|
/// | High(n)    | [1000, 1008]         | 高优先级，n 越小越优先     |
/// | Normal     | 0                    | 默认优先级                 |
/// | Low(n)     | [-1008, -1000]       | 低优先级，n 越小越滞后     |
///
/// # 设计决策
/// - **为什么 High/Low 带 u8 参数？**
///   允许同一优先级层级内的细粒度排序（如日志系统：ERROR > WARN > INFO）。
/// - **为什么 gap 是 1000？**
///   确保 High(255) > Normal > Low(255)，避免边界交叉。
///
/// # 线程安全
/// 优先级在连接时确定并不可变，无需同步机制。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Priority {
    /// 高优先级（参数越小越优先）
    ///
    /// 适用场景：认证检查、输入验证、关键业务逻辑
    High(u8),
    /// 标准优先级（默认值）
    #[default]
    Normal,
    /// 低优先级（参数越小越滞后）
    ///
    /// 适用场景：日志记录、统计指标、清理工作
    Low(u8),
}

/// 优先级排序的缩放因子
///
/// 用于将 High/Normal/Low 三个区间隔开，确保 `High(u8::MAX) > Normal > Low(u8::MAX)`。
/// 值为 1000，远大于 u8 的范围 [0, 255]，因此不会发生区间交叉。
const PRIORITY_SCALE: i32 = 1000;

impl Priority {
    /// 计算用于排序的数值
    ///
    /// # 返回值语义
    /// - 数值越大 → 执行顺序越靠前
    /// - 保证：`High(0) > Normal > Low(0)`
    ///
    /// # 实现细节
    /// 使用固定偏移量（±[`PRIORITY_SCALE`]）隔离三个区间，防止溢出和交叉。
    pub fn order_value(&self) -> i32 {
        match self {
            Priority::High(v) => PRIORITY_SCALE + *v as i32,
            Priority::Normal => 0,
            Priority::Low(v) => -PRIORITY_SCALE - *v as i32,
        }
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.order_value().cmp(&other.order_value())
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 信号发射时的错误处理策略
///
/// # 观察者模式的容错设计
/// 当某个槽位执行失败（panic）时，此策略决定是否继续执行剩余槽位。
///
/// # 策略对比
/// | 策略     | 行为                           | 适用场景               |
/// |----------|--------------------------------|------------------------|
/// | Continue | 跳过失败的槽位，继续执行剩余的   | 日志、监控、非关键路径   |
/// | Stop     | 立即终止发射，返回已收集的错误   | 事务性操作、关键流程     |
///
/// # 示例场景
/// - **Continue**：UI 更新信号，一个监听器崩溃不应阻止其他 UI 组件更新
/// - **Stop**：数据库事务提交信号，任何验证失败都应中止整个提交
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ErrorPolicy {
    /// 遇到错误后继续执行剩余槽位（默认）
    #[default]
    Continue,
    /// 遇到第一个错误立即停止发射
    Stop,
}

/// 信号发射选项
///
/// # 用途
/// 控制信号发射的行为，目前仅支持错误处理策略。
/// 设计为可扩展结构体，未来可添加超时、取消令牌等选项。
///
/// # 典型用法
/// ```ignore
/// signal.emit_with_options(&data, EmitOptions {
///     on_error: ErrorPolicy::Stop, // 严格模式
/// });
/// ```
#[derive(Debug, Clone, Default)]
pub struct EmitOptions {
    /// 错误处理策略
    pub on_error: ErrorPolicy,
}

/// 单个槽位执行错误
///
/// # 观察者模式中的错误传播
/// 当槽位回调函数 panic 时，系统捕获该 panic 并包装为 `SlotError`。
/// 这实现了**故障隔离**：一个槽位的崩溃不会导致整个进程崩溃。
///
/// # 字段说明
/// - `slot_id`: 标识哪个槽位出错，用于调试定位
/// - `message`: panic 的原始消息，保留完整上下文
///
/// # 线程安全
/// 此结构仅在信号发射线程中创建，无需额外同步。
#[derive(Debug, Clone)]
pub struct SlotError {
    /// 出错的槽位连接 ID
    pub slot_id: ConnectionId,
    /// Panic 消息内容
    pub message: String,
}

impl fmt::Display for SlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "slot {} panicked: {}", self.slot_id, self.message)
    }
}

impl std::error::Error for SlotError {}

/// 信号发射结果
///
/// # 观察者模式的执行报告
/// 每次 `Signal::emit()` 调用都会返回此结构，提供完整的执行审计信息。
/// 这是**命令模式**的体现：发射操作不仅产生副作用，还返回可观测的结果。
///
/// # 关键指标
/// - `invoked_count`: 成功执行的槽数量（用于性能监控）
/// - `errors`: 失败详情列表（用于错误追踪）
/// - `elapsed`: 总耗时（用于性能分析）
///
/// # 使用示例
/// ```ignore
/// let result = signal.emit(&event);
/// if !result.is_ok() {
///     for err in &result.errors {
///         eprintln!("槽位 {} 失败: {}", err.slot_id, err.message);
///     }
/// }
/// println!("成功执行 {}/{} 个槽位，耗时 {:?}", result.invoked_count, signal.slot_count(), result.elapsed);
/// ```
#[derive(Debug, Clone)]
pub struct EmitResult {
    /// 成功调用的槽位数量
    pub invoked_count: usize,
    /// 总槽位数量（含成功和失败）
    pub total_count: usize,
    /// 收集到的错误列表（可能为空）
    pub errors: Vec<SlotError>,
    /// 发射总耗时（包含锁等待、排序、执行等全部开销）
    pub elapsed: std::time::Duration,
}

impl EmitResult {
    /// 快速判断发射是否完全成功
    ///
    /// # 返回值
    /// - `true`: 所有槽位均成功执行
    /// - `false`: 至少有一个槽位出错或发生异常
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// GC 即将回收信息（事件负载）
///
/// # 使用场景
/// 用于垃圾回收相关的信号通知，告知监听者当前存活对象数量和回收阈值。
#[derive(Debug, Clone)]
pub struct GcWillCollectInfo {
    /// 当前存活的对象数量
    pub live_count: usize,
    /// 触发回收的阈值
    pub threshold: usize,
}

/// GC 已完成回收信息（事件负载）
///
/// # 使用场景
/// 垃圾回收完成后的事件通知，包含本次回收的统计数据。
#[derive(Debug, Clone)]
pub struct GcDidCollectInfo {
    /// 本次释放的对象数量
    pub freed_count: usize,
    /// 回收操作的耗时
    pub elapsed: std::time::Duration,
    /// 新的计算阈值（动态调整后的值）
    pub new_threshold: usize,
}

/// VM 执行错误信息（事件负载）
///
/// # 使用场景
/// 虚拟机运行时错误的通知，包含完整的错误上下文以便诊断。
#[derive(Debug, Clone)]
pub struct VmErrorInfo {
    /// 可读的错误消息
    pub error_message: String,
    /// 出错时的 opcode（如果可用）
    pub opcode: Option<u8>,
    /// 出错时的指令指针
    pub ip: usize,
    /// 当前调用栈深度（辅助定位递归问题）
    pub call_depth: usize,
}

/// 编译开始信息（事件负载）
///
/// # 使用场景
/// 编译流程的生命周期管理，标记编译阶段的开始点。
#[derive(Debug, Clone)]
pub struct CompileStartedInfo {
    /// 待编译的源码长度（字节）
    pub source_len: usize,
}

/// 编译完成信息（事件负载）
///
/// # 使用场景
/// 编译结果通知，可用于构建系统集成、IDE 反馈、增量编译优化。
///
/// # 分阶段计时
/// `lex_duration` / `parse_duration` / `codegen_duration` 提供编译流水线各阶段的独立耗时，
/// 便于定位性能瓶颈。当无人订阅 `COMPILE_FINISHED` 信号时，计时开销为零（惰性求值）。
#[derive(Debug, Clone)]
pub struct CompileFinishedInfo {
    /// 编译是否成功
    pub success: bool,
    /// 生成的 chunk 大小（字节），失败时为 None
    pub chunk_size: Option<usize>,
    /// 编译总耗时
    pub duration: std::time::Duration,
    /// 词法分析阶段耗时（Lexer: 源码 → Token 流）
    pub lex_duration: std::time::Duration,
    /// 语法分析阶段耗时（Parser: Token 流 → AST）
    pub parse_duration: std::time::Duration,
    /// 字节码生成阶段耗时（Compiler: AST → Chunk）
    pub codegen_duration: std::time::Duration,
}

impl Default for CompileFinishedInfo {
    /// 默认值：失败状态，所有耗时为零
    ///
    /// 便于使用 `CompileFinishedInfo { success: true, ..Default::default() }` 部分构造。
    fn default() -> Self {
        Self {
            success: false,
            chunk_size: None,
            duration: std::time::Duration::ZERO,
            lex_duration: std::time::Duration::ZERO,
            parse_duration: std::time::Duration::ZERO,
            codegen_duration: std::time::Duration::ZERO,
        }
    }
}

/// 内置函数调用信息（事件负载）
///
/// # 使用场景
/// 追踪内置函数调用，用于性能分析、权限审计、调用统计。
#[derive(Debug, Clone)]
pub struct BuiltinCallInfo {
    /// 函数名称（静态字符串，零拷贝）
    pub name: &'static str,
    /// 传入的参数数量
    pub arg_count: usize,
}

// ── 寄存器分配器事件负载 ──────────────────────────────────────────────

/// 槽位所有者标识 -- 标识哪个编译阶段预订了该寄存器范围
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotOwner {
    /// 函数调用参数区 [func_reg, func_reg+argc]
    CallArg,
    /// 数组/字典字面量构造区域 [dest, dest+elem_count]
    ArrayConstruct,
    /// 局部变量（与 Scope 深度绑定）
    LocalVar,
    /// 临时表达式求值结果
    TempExpr,
    /// 内置函数调用上下文
    Builtin,
    /// 闭包对象
    Closure,
}

impl fmt::Display for SlotOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotOwner::CallArg => write!(f, "CallArg"),
            SlotOwner::ArrayConstruct => write!(f, "ArrayConstruct"),
            SlotOwner::LocalVar => write!(f, "LocalVar"),
            SlotOwner::TempExpr => write!(f, "TempExpr"),
            SlotOwner::Builtin => write!(f, "Builtin"),
            SlotOwner::Closure => write!(f, "Closure"),
        }
    }
}

/// 寄存器槽位预订事件
///
/// 当 RegisterAllocator.reserve_slot() 成功分配一个连续寄存器范围时发射。
#[derive(Debug, Clone)]
pub struct SlotReservedInfo {
    /// 槽位所有者
    pub owner: SlotOwner,
    /// 起始寄存器编号（含）
    pub start: u16,
    /// 连续寄存器数量
    pub count: u16,
    /// 当前的 Scope 深度（用于作用域绑定释放）
    pub depth: usize,
}

/// 寄存器槽位释放事件
///
/// 当 RegisterAllocator.release_slot() 释放一个槽位时发射。
#[derive(Debug, Clone)]
pub struct SlotReleasedInfo {
    /// 起始寄存器编号（含）
    pub start: u16,
    /// 释放的连续寄存器数量
    pub count: u16,
}

/// 寄存器冲突检测事件
///
/// 当新预订请求与已有活跃槽位重叠时发射。
/// 监听者可用此事件实现调试可视化或自动冲突解决策略。
#[derive(Debug, Clone)]
pub struct SlotConflictedInfo {
    /// 已有槽位的所有者和范围
    pub existing_owner: SlotOwner,
    pub existing_range: (u16, u16),
    /// 新请求的所有者和期望范围
    pub requested_owner: SlotOwner,
    pub requested_range: (u16, u16),
}

// ── 编译流水线追踪事件负载 ──────────────────────────────────────────────

/// 作用域进入事件负载
///
/// # 使用场景
/// 编译器进入新的词法作用域时发射，用于作用域生命周期追踪、
/// 调试器作用域可视化、IDE 代码折叠提示。
///
/// # scope_type 常见值
/// - `"block"` — 块语句 `{ ... }`
/// - `"function"` — 函数体
/// - `"loop"` — 循环体 (while/for/loop)
#[derive(Debug, Clone)]
pub struct ScopeEnteredInfo {
    /// 进入后的作用域嵌套深度（0 = 全局）
    pub depth: usize,
    /// 作用域类型描述（"block" / "function" / "loop" 等）
    pub scope_type: String,
}

/// 作用域退出事件负载
///
/// # 使用场景
/// 编译器退出词法作用域时发射，与 [`ScopeEnteredInfo`] 配对使用。
/// `depth` 为退出**前**的深度，便于监听者识别退出的是哪一层。
#[derive(Debug, Clone)]
pub struct ScopeExitedInfo {
    /// 退出前的作用域嵌套深度
    pub depth: usize,
}

/// 函数编译完成事件负载
///
/// # 使用场景
/// 单个函数（含闭包）的字节码生成完成后发射，用于：
/// - 编译性能热点分析（哪个函数编译最慢）
/// - 字节码体积审计（instruction_count 过大可能需要拆分）
/// - IDE 增量编译（仅重编译变更的函数）
#[derive(Debug, Clone)]
pub struct FunctionCompileInfo {
    /// 函数名称（匿名函数为 `"<anonymous>"`）
    pub name: String,
    /// 形参数量
    pub arity: u8,
    /// 生成的字节码指令数（字节数，近似指标）
    pub instruction_count: usize,
    /// 函数体编译耗时
    pub duration: std::time::Duration,
}

// ── 信号总线作用域与类型化信号键 ──────────────────────────────────────────

/// 信号总线作用域标识
///
/// # 设计意图
/// 每条信号总线按作用域隔离，避免不同子系统之间的信号名冲突。
/// 作用域同时作为日志和调试的分片键，便于按子系统过滤信号事件。
///
/// # 变体说明
/// - `Gc` — 垃圾回收子系统信号
/// - `Compiler` — 编译器子系统信号
/// - `Builtin` — 内置函数子系统信号
/// - `Custom(&'static str)` — 用户或插件自定义作用域
///
/// # 示例
/// ```ignore
/// use nuzo_signal::BusScope;
///
/// assert_eq!(BusScope::Gc.to_string(), "gc");
/// assert_eq!(BusScope::Custom("plugin:x").to_string(), "custom:plugin:x");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusScope {
    /// 垃圾回收子系统
    Gc,
    /// 编译器子系统
    Compiler,
    /// 内置函数子系统
    Builtin,
    /// 自定义作用域（静态字符串，零分配）
    Custom(&'static str),
}

impl fmt::Display for BusScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BusScope::Gc => write!(f, "gc"),
            BusScope::Compiler => write!(f, "compiler"),
            BusScope::Builtin => write!(f, "builtin"),
            BusScope::Custom(s) => write!(f, "custom:{}", s),
        }
    }
}

/// 类型化信号键
///
/// # 设计哲学
/// `SignalKey<Args>` 将信号名称、作用域和参数类型绑定为一个编译期可验证的整体：
/// - **名称 + 作用域** 确定信号在总线中的唯一地址
/// - **泛型参数 `Args`** 确保发射和连接的类型一致性
/// - `PhantomData<fn(&Args)>` 选择协变标记，不引入所有权约束
///
/// # 协变性
/// 使用 `PhantomData<fn(&Args)>` 而非 `PhantomData<Args>` 的原因：
/// - `fn(&Args)` 对 `Args` 是**协变**的，允许 `SignalKey<&'a T>` 自动协变为 `SignalKey<&'static T>`
/// - 不会隐式要求 `Args: Owned`，避免对 `Args` 施加不必要的 `Drop` 约束
///
/// # Copy 语义
/// `SignalKey` 仅包含 `'static` 引用和 `Copy` 类型，本身也是 `Copy` 的，
/// 可以零成本地在函数间传递而无需克隆堆内存。
///
/// # 示例
/// ```ignore
/// use nuzo_signal::{SignalKey, BusScope};
///
/// // 手动构造
/// let key: SignalKey<String> = SignalKey::new("my_signal", BusScope::Custom("app"));
/// assert_eq!(key.name(), "my_signal");
/// assert_eq!(key.scope(), BusScope::Custom("app"));
///
/// // 使用 declare_signal! 宏
/// declare_signal!(MY_SIGNAL, String, BusScope::Builtin);
/// assert_eq!(MY_SIGNAL.name(), "MY_SIGNAL");
/// ```
pub struct SignalKey<Args: 'static + Send + Sync> {
    name: &'static str,
    scope: BusScope,
    _marker: PhantomData<fn(&Args)>,
}

impl<Args: 'static + Send + Sync> Clone for SignalKey<Args> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Args: 'static + Send + Sync> Copy for SignalKey<Args> {}

impl<Args: 'static + Send + Sync> fmt::Debug for SignalKey<Args> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalKey").field("name", &self.name).field("scope", &self.scope).finish()
    }
}

impl<Args: 'static + Send + Sync> PartialEq for SignalKey<Args> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.scope == other.scope
    }
}

impl<Args: 'static + Send + Sync> Eq for SignalKey<Args> {}

impl<Args: 'static + Send + Sync> std::hash::Hash for SignalKey<Args> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.scope.hash(state);
    }
}

impl<Args: 'static + Send + Sync> SignalKey<Args> {
    /// 创建一个新的类型化信号键
    ///
    /// # 参数
    /// - `name` — 信号的静态名称标识，通常使用 `stringify!` 宏自动生成
    /// - `scope` — 信号所属的总线作用域
    ///
    /// # const 安全
    /// 此函数为 `const fn`，可在编译期求值，支持声明 `static` 常量。
    pub const fn new(name: &'static str, scope: BusScope) -> Self {
        Self { name, scope, _marker: PhantomData }
    }

    /// 获取信号名称
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// 获取信号所属作用域
    pub fn scope(&self) -> BusScope {
        self.scope
    }
}

/// 声明类型化信号键常量的宏
///
/// # 用法
/// ```ignore
/// use nuzo_signal::{declare_signal, BusScope};
///
/// declare_signal!(GC_WILL_COLLECT, GcWillCollectInfo, BusScope::Gc);
/// declare_signal!(COMPILE_STARTED, CompileStartedInfo, BusScope::Compiler);
/// ```
///
/// # 展开结果
/// ```ignore
/// pub const GC_WILL_COLLECT: SignalKey<GcWillCollectInfo> =
///     SignalKey::new("GC_WILL_COLLECT", BusScope::Gc);
/// ```
#[macro_export]
macro_rules! declare_signal {
    ($name:ident, $args:ty, $scope:expr) => {
        pub const $name: $crate::SignalKey<$args> =
            $crate::SignalKey::new(stringify!($name), $scope);
    };
}
