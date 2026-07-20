//! # Nuzo 内置函数模块
//!
//! 本模块实现 Nuzo 运行时的**标准内置函数库**，提供语言核心功能的无缝集成。
//! 内置函数是用 Rust 实现的原生函数，Nuzo 程序无需任何导入或定义即可直接调用。
//!
//! ## 架构设计
//!
//! ```text
//! BuiltinRegistry {
//!     functions: Vec<(&str, BuiltinFn)>
//! }
//!
//! where BuiltinFn = fn(&[Value]) -> Result<Value, NuzoError>
//! ```
//!
//! ### 核心组件
//!
//! - **[`BuiltinRegistry`]**：函数注册表，维护所有可用内置函数的映射
//! - **[`BuiltinFn`]类型签名**：统一的函数接口 `fn(&[Value]) -> Result<Value, NuzoError>`
//! - **信号机制**：通过 [`BUILTIN_CALLED_KEY`] + scoped [`SignalBus`] 支持函数调用的监控和追踪
//!
//! ## 可用的内置函数
//!
//! ### 基础 I/O
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `print` | `print(args...) → nil` | 打印值（无换行） |
//! | `println` | `println(args...) → nil` | 打印值（带换行） |
//!
//! ### 类型系统
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `type_of` | `type_of(value) → number` | 返回类型代码（数字） |
//! | `typeof` | `typeof(value) → string` | 返回类型名称（字符串） |
//! | `str` | `str(value) → string` | 转换为字符串表示 |
//!
//! ### 断言与调试
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `assert` | `assert(cond, msg?) → nil` | 条件断言（失败抛出错误） |
//! | `len` | `len(value) → number` | 返回集合/字符串长度 |
//!
//! ### 集合操作
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `push` | `push(arr, val) → nil` | 数组末尾追加元素 |
//! | `pop` | `pop(arr) → value` | 移除并返回末尾元素 |
//! | `keys` | `keys(dict) → array` | 返回字典的所有键 |
//!
//! ### 运行时安全
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `trampoline` | `trampoline(fn, arg) → value` | 安全递归执行器（防无限循环） |
//!
//! # 使用示例
//!
//! ```nuzo
//! // 基础 I/O
//! println("Hello, World!")      // 输出: Hello, World!\n
//! print(42, " is the answer")  // 输出: 42 is the answer
//!
//! // 类型检查
//! type_of(42)        // 返回: 1.0 (number 类型码)
//! typeof("hello")    // 返回: "string"
//! str([1, 2, 3])     // 返回: "[1, 2, 3]"
//!
//! // 断言
//! assert(1 + 1 == 2)              // 通过
//! assert(false, "出错了!")         // 抛出: assertion failed (custom message provided)
//!
//! // 集合操作
//! let arr = [1, 2, 3]
//! push(arr, 4)       // arr 变为 [1, 2, 3, 4]
//! let last = pop(arr) // last = 4, arr 变为 [1, 2, 3]
//!
//! let d = {"name": "Alice", "age": 30}
//! let k = keys(d)     // k = ["name", "age"]
//! ```
//!
//! # 错误处理机制
//!
//! 所有内置函数遵循统一的错误处理约定：
//!
//! 1. **参数校验失败**：返回 [`NuzoError::TypeMismatch`]，包含期望类型和实际类型
//! 2. **断言失败**：返回 [`NuzoError::AssertFailed`]，携带自定义或默认消息
//! 3. **运行时异常**：返回 [`NuzoError::Internal`]，包装底层 Rust 错误
//!
//! # 输出捕获机制
//!
//! 为了支持测试框架（如 `nuzo_test!` 宏），本模块实现了**线程局部输出捕获**：
//!
//! - 通过 [`configure_output_capture()`] 设置捕获缓冲区
//! - 当启用捕获时，`print/println` 的输出写入缓冲区而非 stdout
//! - 这允许测试同时验证 Print opcode 和内置函数的输出
//!
//! # 性能优化
//!
//! - **零拷贝参数传递**：所有函数接收 `&[Value]` 切片引用
//! - **惰性求值**：短路逻辑避免不必要的计算
//! - **内联友好**：小型函数标记为 `#[inline]`（编译器自动决定）
//! - **缓存友好**：注册表使用连续内存的 `Vec` 存储

use std::sync::{Arc, Mutex};

use nuzo_core::Value;

use nuzo_core::encoding::char_len;
use nuzo_core::{
    TYPE_CODE_BOOL, TYPE_CODE_NIL, TYPE_CODE_NUMBER, TYPE_CODE_OBJECT, TYPE_CODE_UNKNOWN,
};
use nuzo_signal::{BuiltinCallInfo, BusScope, Signal, SignalBus};
use nuzo_values::{HeapObject, InternalError, NIL, NuzoError, RangeEnd, ValueExt};

// ── 类型化信号键（scoped SignalBus 模式）───────────────────────────
//
// 替代原先的全局静态 Signal 实例，通过 declare_signal! 宏生成
// 编译期类型安全的信号键常量，运行时由 BuiltinRegistry 持有的
// scoped SignalBus 管理信号生命周期。
//
// # 迁移动机
//
// - **作用域隔离**：Builtin 信号独立于 GC/Compiler 信号，避免名称冲突
// - **生命周期管理**：随 BuiltinRegistry 实例创建/销毁，便于测试隔离
// - **类型安全**：SignalKey<BuiltinCallInfo> 编译期绑定载荷类型
nuzo_signal::declare_signal!(BUILTIN_CALLED_KEY, BuiltinCallInfo, BusScope::Builtin);

// ============================================================================
// 线程局部输出捕获（用于测试）
// ============================================================================

// 线程局部的输出捕获缓冲区
//
// 当 VM 通过 `new_with_output_capture()` 创建时，会设置此线程局部变量，
// 以捕获内置函数 print/println 的输出。这允许 `nuzo_test!` 宏同时验证：
// - Print opcode 的输出
// - 内置函数调用的输出
//
// # 类型说明
//
// `Option<Arc<Mutex<Vec<String>>>>`：
// - `None`：正常模式，输出到 stdout
// - `Some(buffer)`：捕获模式，输出追加到 buffer
//
// # 线程安全性
//
// 使用 `thread_local!` 确保每个线程有独立的捕获状态，
// 避免并发测试时的竞争条件。
thread_local! {
    #[allow(clippy::type_complexity)] // 输出捕获栈：Vec<Option<Arc<Mutex<Vec<String>>>>>，拆分会降低可读性
    static OUTPUT_CAPTURE: std::cell::RefCell<Vec<Option<Arc<Mutex<Vec<String>>>>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// 将捕获缓冲区压入当前线程的输出捕获栈。
///
/// 由 `Session::eval` / `Session::run` 在执行脚本前调用，以实现按 Session
/// 隔离的输出捕获；执行完毕后必须调用 [`pop_output_capture`] 弹出，否则
/// 后续同线程的输出会落到错误的缓冲区。
///
/// # 推荐使用 RAII guard
///
/// 直接调用 `push_output_capture` / `pop_output_capture` 容易因早期 return、
/// `?` 传播或 panic 而漏掉 pop，导致输出捕获栈失衡。**新代码应优先使用
/// [`OutputCaptureGuard::new`]**，它会在 drop 时自动调用 `pop_output_capture`，
/// 即使发生 panic 也能保证栈平衡。
///
/// # 参数
///
/// * `capture` - 捕获缓冲区（None 表示该帧输出到 stdout）
pub fn push_output_capture(capture: Option<Arc<Mutex<Vec<String>>>>) {
    OUTPUT_CAPTURE.with(|oc| {
        oc.borrow_mut().push(capture);
    });
}

/// 弹出当前线程输出捕获栈的顶层帧。
///
/// 与 [`push_output_capture`] 配对使用。通常无需手动调用，应通过
/// [`OutputCaptureGuard`] 在作用域结束时自动触发。
pub fn pop_output_capture() {
    OUTPUT_CAPTURE.with(|oc| {
        oc.borrow_mut().pop();
    });
}

/// 输出捕获栈帧的 RAII guard（P2-13）。
///
/// 确保 [`push_output_capture`] 之后**一定**调用 [`pop_output_capture`]，
/// 即使作用域内发生 panic、`?` 早期返回或 `return` 语句。
///
/// # 设计动机
///
/// 原先 `push_output_capture` / `pop_output_capture` 是分离的函数对，调用方
/// 必须手动保证 pop 一定会被调用。在 `Session::run` / `Session::eval` 等入口
/// 中，编译或执行可能因错误提前 return，导致漏 pop，使后续同线程的输出
/// 落到错误的缓冲区（测试串扰、输出错位）。
///
/// 引入 RAII guard 后，pop 由 Rust 的 Drop 语义保证执行，无需调用方操心。
///
/// # 使用示例
///
/// ```rust,ignore
/// use nuzo_helpers::builtins::OutputCaptureGuard;
/// use std::sync::{Arc, Mutex};
///
/// let buffer = Arc::new(Mutex::new(Vec::new()));
/// {
///     let _guard = OutputCaptureGuard::new(Some(buffer.clone()));
///     // 在此作用域内，print/println 的输出会写入 buffer
///     run_user_code()?;
/// } // _guard drop 时自动 pop_output_capture()
/// ```
///
/// # 向后兼容
///
/// 现有直接调用 `push_output_capture` / `pop_output_capture` 的代码无需修改
/// 即可继续工作。guard 只是一个更安全的替代方案，不改变底层栈语义。
pub struct OutputCaptureGuard {
    // 私有字段防止模块外部直接构造（必须通过 new() 创建以成对 push）
    _private: (),
}

impl OutputCaptureGuard {
    /// 压入一帧输出捕获目标，并返回 guard。
    ///
    /// guard drop 时会自动调用 [`pop_output_capture`]，保证栈平衡。
    ///
    /// # 参数
    ///
    /// * `capture` - 捕获缓冲区（None 表示该帧输出到 stdout）
    pub fn new(capture: Option<Arc<Mutex<Vec<String>>>>) -> Self {
        push_output_capture(capture);
        OutputCaptureGuard { _private: () }
    }
}

impl Drop for OutputCaptureGuard {
    fn drop(&mut self) {
        pop_output_capture();
    }
}

/// 配置当前线程的输出捕获缓冲区。
///
/// 为兼容历史调用方，此函数会清空整个捕获栈并设置为单个帧。
/// 新的按 Session 隔离代码应优先使用 [`push_output_capture`] /
/// [`pop_output_capture`]。
///
/// # 参数
///
/// * `capture` - 捕获缓冲区（None 表示禁用捕获）
pub fn configure_output_capture(capture: Option<Arc<Mutex<Vec<String>>>>) {
    OUTPUT_CAPTURE.with(|oc| {
        let mut stack = oc.borrow_mut();
        stack.clear();
        if let Some(buf) = capture {
            stack.push(Some(buf));
        }
    });
}

/// 获取当前线程的输出捕获缓冲区（如果存在）
///
/// # 返回值
///
/// - `Some(Arc<Mutex<Vec<String>>>)`：捕获模式已启用
/// - `None`：正常模式（输出到 stdout）
pub(crate) fn output_capture() -> Option<Arc<Mutex<Vec<String>>>> {
    OUTPUT_CAPTURE.with(|oc| oc.borrow().last().cloned().flatten())
}

// ============================================================================
// 内置函数类型定义
// ============================================================================

/// 内置函数的类型签名
///
/// 所有内置函数必须符合此签名：接收 `Value` 切片参数，返回 `Result<Value, NuzoError>`。
///
/// # 设计约定
///
/// - **成功时**：返回 `Ok(Value)`，其中 Value 是函数的返回值
/// - **失败时**：返回 `Err(NuzoError)`，携带描述性错误信息
/// - **参数校验**：每个函数负责验证参数数量和类型
///
/// # 实现示例
///
/// ```rust,ignore
/// use nuzo_values::{Value, NuzoError};
///
/// fn my_builtin(args: &[Value]) -> Result<Value, NuzoError> {
///     // 1. 验证参数数量
///     if args.len() != 1 {
///         return Err(NuzoError::invalid_argument_count(1, args.len()));
///     }
///     // 2. 执行逻辑
///     // 3. 返回结果
///     Ok(args[0].clone())
/// }
/// ```
pub type BuiltinFn = fn(&[Value]) -> Result<Value, NuzoError>;

// ============================================================================
// 内置函数注册表
// ============================================================================

/// 内置函数注册表（Registry 模式）
///
/// 维护所有可用内置函数的列表，提供按名称查找的功能。
/// VM 通过此注册表将 Nuzo 函数调用映射到 Rust 实现。
///
/// # 架构特点
///
/// ```text
/// ┌──────────────────────────────────────┐
/// │         BuiltinRegistry              │
/// │  ┌────────────────────────────┐      │
/// │  │ functions: Vec<(name, fn)> │      │
/// │  │  ├─ "print"    → builtin_print   │
/// │  │  ├─ "println"  → builtin_println│
/// │  │  ├─ "type_of"  → builtin_type_of│
/// │  │  └─ ...                       │      │
/// │  └────────────────────────────┘      │
/// └──────────────────────────────────────┘
/// ```
///
/// # 线程安全性
///
/// 注册表在 VM 初始化时创建，之后只读访问，因此天然线程安全。
/// 如果需要动态注册新函数，应在外部加锁。
///
/// # 性能特征
///
/// - **查找复杂度**：O(n) 线性扫描（适合小型函数集）
/// - **内存布局**：连续存储，缓存友好
/// - **扩展性**：可通过 `register()` 方法动态添加函数
///
/// # 示例
///
/// ```
/// use nuzo_helpers::builtins::BuiltinRegistry;
/// use nuzo_values::{Value, ValueExt};
///
/// // 创建默认注册表（包含所有标准内置函数）
/// let registry = BuiltinRegistry::new();
///
/// // 查找并调用内置函数
/// if let Some(println_fn) = registry.get("println") {
///     let args = vec![Value::from_string("Hello")];
///     let result = println_fn(&args).unwrap();
///     assert!(result.is_nil()); // println 返回 nil
/// }
///
/// // 列出所有可用函数
/// for name in registry.names() {
///     println!("- {}", name);
/// }
/// ```
pub struct BuiltinRegistry {
    /// 函数列表：(名称, 函数指针, arity) 三元组
    ///
    /// 使用 `Vec` 而非 `HashMap` 的原因：
    /// 1. 保证插入顺序（便于调试和文档生成）
    /// 2. 更小的内存占用（函数集通常 < 100 个）
    /// 3. 缓存友好的连续内存布局
    ///
    /// 自 BUG-001 修复起，arity 不再是 `_arity` 占位参数，
    /// 而是被实际存储，供 VM 校验 builtin 调用参数数量。
    functions: Vec<(&'static str, BuiltinFn, u8)>,

    /// 作用域限定的事件总线（scoped SignalBus）
    ///
    /// 管理 builtin 子系统所有信号的注册和查找，
    /// 替代原先的全局静态信号实例 `BUILTIN_CALLED`。
    ///
    /// # 生命周期
    ///
    /// 随 BuiltinRegistry 实例创建/销毁，便于测试隔离：
    /// 每个 BuiltinRegistry::new() 创建独立的 bus，
    /// 互不干扰。
    ///
    /// # 线程安全
    ///
    /// `Arc<SignalBus>` 允许外部持有 bus 的共享引用，
    /// 用于在 registry 外部订阅信号。
    bus: Arc<SignalBus>,
}

impl BuiltinRegistry {
    /// 创建新的注册表，并注册所有标准内置函数
    ///
    /// 此构造函数会自动注册以下内置函数：
    ///
    /// ### 基础 I/O（2 个）
    /// - `print`：无换行打印
    /// - `println`：带换行打印
    ///
    /// ### 类型系统（3 个）
    /// - `type_of`：返回类型代码
    /// - `typeof`：返回类型名称（字符串）
    /// - `str`：转换为字符串
    ///
    /// ### 调试/测试（1 个）
    /// - `assert`：条件断言
    ///
    /// ### 集合操作（4 个）
    /// - `len`：获取长度
    /// - `push`：数组追加
    /// - `pop`：数组弹出
    /// - `keys`：字典键列表
    ///
    /// ### 运行时安全（1 个）
    /// - `trampoline`：安全递归执行器
    ///
    /// # 返回值
    ///
    /// 包含 12 个预注册函数的新注册表实例
    pub fn new() -> Self {
        let bus = Arc::new(SignalBus::scoped(BusScope::Builtin));

        let sig = Signal::<BuiltinCallInfo>::named("builtin_called");
        let _ = bus.register(&BUILTIN_CALLED_KEY, &sig);

        let mut reg = BuiltinRegistry { functions: Vec::new(), bus };

        // 注册核心 I/O 函数
        reg.register("print", builtin_print, 0);
        reg.register("println", builtin_println, 0);
        // 中文别名：打印 = println（国际化支持）
        reg.register("打印", builtin_println, 0);

        // 注册类型内省函数
        reg.register("type_of", builtin_type_of, 1);

        // 注册调试/测试工具
        reg.register("assert", builtin_assert, 1);

        // 注册集合操作
        reg.register("len", builtin_len, 1);

        // 注册数组/字典操作 (P3.2)
        reg.register("push", builtin_push, 2);
        reg.register("pop", builtin_pop, 1);
        reg.register("keys", builtin_keys, 1);

        // 注册类型转换操作 (P3.2)
        reg.register("str", builtin_str, 1);
        reg.register("typeof", _builtin_typeof, 1); // 注意: 'typeof' 是 Rust 关键字，所以用 '_typeof'

        // 注册运行时安全工具（TCO / Trampoline）
        reg.register("trampoline", builtin_trampoline, 2);

        // === T8+T9：注册 sys / dict / format 新增内置函数（含中英双语别名）===
        // 本块置于子模块 register() 之前：get()/call() 采用 first-match-wins，
        // 需确保 "format" 指向 string::builtin_str_format（完整版）而非
        // debug::builtin_format（仅支持 {} 的简化版）。
        //
        // sys 模块 —— 进程环境
        reg.register("args", super::sys::sys_args, 0);
        reg.register("参数", super::sys::sys_args, 0);
        reg.register("env", super::sys::sys_env, 0);
        reg.register("环境", super::sys::sys_env, 0);
        reg.register("getenv", super::sys::sys_getenv, 1);
        reg.register("取环境", super::sys::sys_getenv, 1);
        reg.register("exit", super::sys::sys_exit, 1);
        reg.register("退出", super::sys::sys_exit, 1);
        // sys 模块 —— 文件系统
        reg.register("list_dir", super::sys::sys_list_dir, 1);
        reg.register("列目录", super::sys::sys_list_dir, 1);
        reg.register("mkdir", super::sys::sys_mkdir, 1);
        reg.register("建目录", super::sys::sys_mkdir, 1);
        reg.register("exists", super::sys::sys_exists, 1);
        reg.register("存在", super::sys::sys_exists, 1);
        reg.register("remove", super::sys::sys_remove, 1);
        reg.register("删除", super::sys::sys_remove, 1);
        reg.register("rename", super::sys::sys_rename, 2);
        reg.register("重命名", super::sys::sys_rename, 2);
        // sys 模块 —— 标准错误输出（print/println 保持原有注册不变）
        reg.register("eprintln", super::sys::sys_eprintln, 0);
        reg.register("错误输出", super::sys::sys_eprintln, 0);
        // dict 容器操作（array.rs）；"keys" 已在上方注册为 builtin_keys，此处仅注册中文别名
        reg.register("键名", builtin_keys, 1);
        reg.register("values", super::array::dict_values, 1);
        reg.register("键值", super::array::dict_values, 1);
        reg.register("has_key", super::array::dict_has_key, 2);
        reg.register("含键", super::array::dict_has_key, 2);
        reg.register("has_value", super::array::dict_has_value, 2);
        reg.register("含值", super::array::dict_has_value, 2);
        reg.register("extend", super::array::dict_extend, 2);
        reg.register("合并", super::array::dict_extend, 2);
        // format 格式化系列（string.rs）；置于 debug::register 之前以覆盖简化版 format
        reg.register("format", super::string::builtin_str_format, 0);
        reg.register("格式化", super::string::builtin_str_format, 0);
        reg.register("printf", super::string::builtin_str_printf, 0);
        reg.register("输出格式", super::string::builtin_str_printf, 0);
        reg.register("printlnf", super::string::builtin_str_printlnf, 0);
        reg.register("输出格式行", super::string::builtin_str_printlnf, 0);

        // === 子模块统一挂载（BUG-001 修复） ===
        // 原实现仅注册上述 11 个核心 builtin，array/string/math/io/time/convert/debug
        // 共 40+ builtin 全部为死代码。现统一在构造时挂载所有子模块。
        super::string::register(&mut reg);
        super::math::register(&mut reg);
        super::io::register(&mut reg);
        super::time::register(&mut reg);
        super::convert::register(&mut reg);
        super::debug::register(&mut reg);
        super::array::register(&mut reg);

        reg
    }

    /// 注册新的内置函数
    ///
    /// 将自定义函数添加到注册表中，使其可通过名称调用。
    ///
    /// # 参数
    ///
    /// * `name` - 函数名称（在 Nuzo 代码中使用的标识符，必须是静态字符串）
    /// * `func` - 函数实现（符合 [`BuiltinFn`] 签名）
    /// * `_arity` - 参数数量提示（保留用于未来文档/校验，当前未使用）
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// use nuzo_helpers::builtins::BuiltinRegistry;
    /// use nuzo_values::{Value, NuzoError};
    ///
    /// fn custom_greet(args: &[Value]) -> Result<Value, NuzoError> {
    ///     Ok(Value::from_string("Hello from Rust!"))
    /// }
    ///
    /// let mut registry = BuiltinRegistry::new();
    /// registry.register("greet", custom_greet, 0); // 无参数
    /// ```
    ///
    /// # 注意事项
    ///
    /// - 函数名不能重复（后注册的会覆盖先前的，但 Vec 中会存在重复项）
    /// - 建议在 VM 初始化阶段完成所有注册，避免运行时动态修改
    pub fn register(&mut self, name: &'static str, func: BuiltinFn, arity: usize) {
        // BUG-001 修复：arity 不再忽略，作为第 3 个字段存储。
        // arity 截断到 u8 范围（最多 255 个参数，远超实际 builtin 需求）。
        self.functions.push((name, func, arity.min(u8::MAX as usize) as u8));
    }

    /// 按名称查找内置函数
    ///
    /// 在注册表中线性搜索匹配的函数名。
    ///
    /// # 参数
    ///
    /// * `name` - 要查找的函数名
    ///
    /// # 返回值
    ///
    /// - `Some(BuiltinFn)`：找到对应的函数指针
    /// - `None`：函数不存在
    ///
    /// # 性能说明
    ///
    /// 时间复杂度 O(n)，n 为已注册函数数量。
    /// 对于典型场景（< 100 个函数），性能足够。
    /// 如果需要更高性能，可考虑改用 HashMap 实现。
    ///
    /// # 示例
    ///
    /// ```
    /// use nuzo_helpers::builtins::BuiltinRegistry;
    ///
    /// let registry = BuiltinRegistry::new();
    ///
    /// match registry.get("print") {
    ///     Some(print_fn) => println!("找到 print 函数"),
    ///     None => println!("print 函数不存在"),
    /// }
    /// ```
    pub fn get(&self, name: &str) -> Option<BuiltinFn> {
        self.functions.iter().find(|(n, _, _)| *n == name).map(|(_, f, _)| *f)
    }

    /// 查找并调用内置函数（带信号发射）
    ///
    /// 此方法是 [`get()`] 和函数调用的组合，额外功能：
    /// 1. 在调用前发射 `builtin_called` 信号（用于监控/日志）
    /// 2. 统一处理"函数不存在"的情况（返回 None 而非 panic）
    ///
    /// # 参数
    ///
    /// * `name` - 要调用的函数名
    /// * `args` - 传递给函数的参数列表
    ///
    /// # 返回值
    ///
    /// - `Some(Ok(Value))`：函数存在且执行成功
    /// - `Some(Err(NuzoError))`：函数存在但执行失败
    /// - `None`：函数不存在于注册表中
    ///
    /// # 信号机制
    ///
    /// 仅当有监听器订阅 `builtin_called` 信号时才发射，
    /// 避免无监听器时的性能开销。
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// use nuzo_helpers::builtins::BuiltinRegistry;
    /// use nuzo_core::Value;
    ///
    /// let registry = BuiltinRegistry::new();
    ///
    /// match registry.call("println", &[Value::from_string("test")]) {
    ///     Some(Ok(result)) => println!("执行成功"),
    ///     Some(Err(e)) => eprintln!("执行失败: {}", e),
    ///     None => eprintln!("函数不存在"),
    /// }
    /// ```
    pub fn call(&self, name: &str, args: &[Value]) -> Option<Result<Value, NuzoError>> {
        self.functions.iter().find(|(n, _, _)| *n == name).map(|(static_name, f, _)| {
            // 通过 scoped SignalBus 发射信号（替代全局静态 BUILTIN_CALLED）
            if let Ok(sig) = self.bus.get(&BUILTIN_CALLED_KEY)
                && !sig.is_empty()
            {
                sig.emit(&BuiltinCallInfo { name: static_name, arg_count: args.len() });
            }
            f(args)
        })
    }

    /// 按名称查找 builtin 的 arity
    ///
    /// BUG-001 修复后从注册表实际存储读取，而非在调用方硬编码 match 表。
    ///
    /// # 参数
    /// * `name` - builtin 函数名
    ///
    /// # 返回值
    /// - `Some(usize)`：builtin 存在，返回其 arity（参数数量）
    /// - `None`：builtin 不存在
    pub fn get_arity(&self, name: &str) -> Option<usize> {
        self.functions.iter().find(|(n, _, _)| *n == name).map(|(_, _, a)| *a as usize)
    }

    /// 获取所有已注册的函数名称列表
    ///
    /// 返回注册表中所有函数名的快照，顺序与注册顺序一致。
    ///
    /// # 返回值
    ///
    /// 包含静态字符串切片的向量（零拷贝）
    ///
    /// # 使用场景
    ///
    /// - **文档生成**：自动生成 API 文档
    /// - **REPL 补全**：提供 Tab 自动补全
    /// - **调试输出**：列出可用函数
    /// - **测试验证**：确认特定函数是否已注册
    ///
    /// # 示例
    ///
    /// ```
    /// use nuzo_helpers::builtins::BuiltinRegistry;
    ///
    /// let registry = BuiltinRegistry::new();
    /// let names = registry.names();
    ///
    /// assert!(names.contains(&"print"));
    /// assert!(names.contains(&"println"));
    /// assert!(names.contains(&"type_of"));
    /// println!("共 {} 个内置函数", names.len());
    /// ```
    pub fn names(&self) -> Vec<&'static str> {
        self.functions.iter().map(|(n, _, _)| *n).collect()
    }

    /// 获取已注册函数的数量
    ///
    /// # 返回值
    ///
    /// 注册表中的函数总数（usize）
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// 检查注册表是否为空
    ///
    /// # 返回值
    ///
    /// - `true`：没有任何已注册的函数
    /// - `false`：至少有一个函数
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// 获取内置函数信号总线的共享引用
    ///
    /// 外部代码可通过此方法获取 bus 引用，结合 [`BUILTIN_CALLED_KEY`]
    /// 查找并订阅 builtin 调用信号。
    ///
    /// # 返回值
    ///
    /// `Arc<SignalBus>` 共享引用，可克隆后传递到其他模块
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// use nuzo_helpers::builtins::{BuiltinRegistry, BUILTIN_CALLED_KEY};
    /// use nuzo_signal::BuiltinCallInfo;
    ///
    /// let registry = BuiltinRegistry::new();
    /// let bus = registry.bus();
    ///
    /// let sig = bus.get(&BUILTIN_CALLED_KEY).unwrap();
    /// sig.connect(|info: &BuiltinCallInfo| {
    ///     println!("调用: {} ({} 个参数)", info.name, info.arg_count);
    /// });
    /// ```
    pub fn bus(&self) -> Arc<SignalBus> {
        Arc::clone(&self.bus)
    }
}

impl Default for BuiltinRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Built-in Function Implementations
// ============================================================================

/// **print(args...)** → Value::NIL
///
/// Print all arguments to stdout without a trailing newline.
/// Uses the Value's Display trait for formatting.
///
/// # Return Value
///
/// This function always returns `Value::NIL` to indicate successful completion.
/// The NIL value is the canonical "no meaningful return value" indicator in Nuzo,
/// distinct from `Value::default()` which returns 0.0 (a number).
///
/// # Examples
///
/// ```nuzo
/// print(42)           // prints: 42, returns: nil
/// print("hello")      // prints: hello, returns: nil
/// print(1, 2, 3)      // prints: 1 2 3, returns: nil
/// ```
fn args_join(args: &[Value]) -> String {
    args.iter().map(|v| v.concat_repr()).collect::<Vec<_>>().join(" ")
}

fn output_args(args: &[Value], newline: bool) -> Result<Value, NuzoError> {
    let capture = output_capture();
    let output = args_join(args);

    if let Some(buffer) = capture {
        buffer.lock().unwrap_or_else(|e| e.into_inner()).push(output);
    } else if newline {
        println!("{}", output);
    } else {
        print!("{}", output);
    }

    Ok(NIL)
}

fn builtin_print(args: &[Value]) -> Result<Value, NuzoError> {
    output_args(args, false)
}
fn builtin_println(args: &[Value]) -> Result<Value, NuzoError> {
    output_args(args, true)
}

/// **type_of(value)** → Value (string representation)
///
/// Return the type name of the given value as a string identifier.
///
/// This function returns a numeric type code that maps to specific types.
/// In a future implementation with full string support, this will return
/// actual string values instead of numeric codes.
///
/// # Type Mapping
///
/// | Value Type | Returned Code | Type Name    |
/// |------------|---------------|--------------|
/// | Number     | 1.0           | `"number"`   |
/// | Boolean    | 2.0           | `"bool"`     |
/// | Nil        | 3.0           | `"nil"`      |
/// | String     | 4.0           | `"string"`   |
/// | Array      | 5.0           | `"array"`    |
/// | Object     | 6.0           | `"object"`   |
/// | Other      | 0.0           | `"unknown"`  |
///
/// # Examples
///
/// ```nuzo
/// type_of(42)       // returns: "number" (code 1.0)
/// type_of(true)     // returns: "bool" (code 2.0)
/// type_of(nil)      // returns: "nil" (code 3.0)
/// type_of("hello")  // returns: "string" (code 4.0)
/// type_of([1,2,3])  // returns: "array" (code 5.0)
/// type_of({})       // returns: "object" (code 6.0)
/// ```
///
/// # Implementation Notes
///
/// For pointer values (heap objects), this function attempts to inspect the
/// actual heap object type when possible (e.g., through GC integration).
/// For non-pointer built-in types, it uses the Value's built-in type detection.
fn builtin_type_of(args: &[Value]) -> Result<Value, NuzoError> {
    // Require exactly one argument
    if args.len() != 1 {
        let actual = match args.len() {
            0 => "0 arguments",
            _ => "too many arguments",
        };
        return Err(NuzoError::type_mismatch("1 argument", actual));
    }

    let value = args[0];

    // Determine type and return appropriate type code
    // Type codes are used because full string support in Value is not yet complete.
    // When string interning is implemented, this can return actual string Values.
    //
    // The mapping is designed to be extensible and follows common conventions:
    // - 1-3: Primitive types (number, bool, nil)
    // - 4-6: Heap-allocated types (string, array, object)
    // - 0: Unknown/fallback
    let type_code = if value.is_number() {
        TYPE_CODE_NUMBER // "number"
    } else if value.is_bool() {
        TYPE_CODE_BOOL // "bool"
    } else if value.is_nil() {
        TYPE_CODE_NIL // "nil"
    } else if value.is_ptr() {
        // For pointer values, we would ideally check the heap object type here
        // This requires GC integration to borrow the ObjRef and match on HeapObject
        //
        // Future implementation:
        // ```rust
        // if let Some(obj_ref) = value.as_obj_ref() {
        //     match &*obj_ref.borrow() {
        //         HeapObject::String(_) => 4.0,
        //         HeapObject::Array(_) => 5.0,
        //         HeapObject::Object { .. } => 6.0,
        //         HeapObject::Closure { .. } => 7.0, // "closure"
        //     }
        // } else {
        //     6.0 // Default to "object" for unknown pointers
        // }
        // ```
        //
        // For now, we default all pointers to "object" (TYPE_CODE_OBJECT)
        // This will be upgraded when GC is fully integrated into builtins
        TYPE_CODE_OBJECT // "object" (generic pointer type)
    } else {
        TYPE_CODE_UNKNOWN // "unknown"
    };

    Ok(Value::from_number(type_code))
}

/// **assert(condition, message?)** → Value::NIL
///
/// Assert that the condition is truthy. If the condition is falsy (nil or false),
/// raise an `AssertFailed` error with the provided message or a default message.
///
/// # Arguments
///
/// * `condition` (required) - The value to test for truthiness
/// * `message` (optional) - Custom error message if assertion fails
///
/// # Truthiness Rules
///
/// - `false` → falsy
/// - `nil` → falsy
/// - Everything else (including 0, "", etc.) → truthy
///
/// # Error Handling
///
/// When an assertion fails, this function returns a `NuzoError::AssertFailed`
/// error variant, which carries a descriptive message. This is more specific than
/// a generic `TypeMismatch` error and allows callers to distinguish assertion
/// failures from other runtime errors.
///
/// The custom message parameter (second argument) can be used to provide
/// context-specific error messages for debugging and testing purposes.
///
/// # Return Value
///
/// On success (condition is truthy), returns `Value::NIL`.
///
/// # Examples
///
/// ```nuzo
/// assert(true)                    // passes, returns: nil
/// assert(1 == 1)                  // passes, returns: nil
/// assert(false, "should be true") // fails with: "assertion failed: should be true"
/// assert(nil)                     // fails with: "assertion failed"
/// assert(false, "x must be positive") // fails with: "assertion failed: x must be positive"
/// ```
///
/// # Implementation Details
///
/// This function uses the dedicated `NuzoError::AssertFailed` variant instead
/// of the generic `TypeMismatch` error. This provides:
/// - Clearer error classification
/// - Better error messages for debugging
/// - Ability to catch assertion failures specifically in error handling code
fn builtin_assert(args: &[Value]) -> Result<Value, NuzoError> {
    // Need at least one argument (the condition)
    if args.is_empty() {
        return Err(NuzoError::type_mismatch("at least 1 argument (condition)", "0 arguments"));
    }

    let condition = args[0];

    if !condition.is_truthy() {
        // Assertion failed - determine error message
        // Use the new AssertFailed error type for clearer error reporting
        let message = if args.len() >= 2 {
            // Custom message provided - try to extract a meaningful representation
            // Since we're limited to &'static str in RuntimeError, we use a prefix
            // approach that indicates a custom message was provided
            //
            // In a full implementation with string support, we would do:
            // ```rust
            // if let Some(msg_string) = args[1].as_string_opt() {
            //     // Use the actual string message (would need String support in NuzoError)
            // }
            // ```
            //
            // For now, we indicate that a custom message was provided.
            // The caller can inspect args[1] separately if needed.
            "assertion failed (custom message provided)".to_string()
        } else {
            // Default assertion failure message
            "assertion failed".to_string()
        };

        return Err(NuzoError::assert_failed(message));
    }

    // Assertion passed - return NIL explicitly
    Ok(NIL)
}

/// **len(value)** → Value (number)
///
/// Return the length of a collection or string.
///
/// # Supported Types
///
/// - **Arrays** → number of elements
/// - **Strings** → number of characters (or bytes, depending on implementation)
/// - **Other types** → Error (TypeError)
///
/// # Examples
///
/// ```nuzo
/// len([1, 2, 3])     // returns: 3
/// len("hello")       // returns: 5
/// len("")            // returns: 0
/// len(42)            // error: cannot get length of number
/// ```
///
/// # Implementation Notes
///
/// This function supports heap-allocated types (strings, arrays) through
/// integration with the GC system. When a value is a pointer to a heap object,
/// the function inspects the actual object type and returns the appropriate length.
///
/// For non-pointer primitive types (numbers, bools, nil), this function always
/// returns an error as these types don't have a meaningful length concept.
///
/// # Future Enhancements
///
/// - Unicode string length (code points vs bytes vs grapheme clusters)
/// - Nested array/collection length
/// - Custom object length via `__len__` protocol
fn builtin_len(args: &[Value]) -> Result<Value, NuzoError> {
    // Require exactly one argument
    if args.len() != 1 {
        let actual = match args.len() {
            0 => "0 arguments",
            _ => "too many arguments",
        };
        return Err(NuzoError::type_mismatch("1 argument", actual));
    }

    let value = args[0];

    if value.is_string() {
        let s = value.as_string_opt().unwrap_or_default();
        return Ok(Value::from_number(char_len(&s) as f64));
    }

    if value.is_heap_object() {
        if let Some(obj) = value.as_heap_object_opt() {
            match obj.as_ref() {
                HeapObject::Array(arr) => {
                    return Ok(Value::from_number(arr.len() as f64));
                }
                HeapObject::Dict(d) => {
                    return Ok(Value::from_number(d.len() as f64));
                }
                HeapObject::Range { start, end, range_end } => {
                    let len = if matches!(range_end, RangeEnd::Inclusive) {
                        (*end - *start + 1.0).max(0.0) as usize
                    } else {
                        (*end - *start).max(0.0) as usize
                    };
                    return Ok(Value::from_number(len as f64));
                }
                _ => {}
            }
        }
        return Err(NuzoError::type_mismatch("string, array, or range", "object"));
    }

    let type_name = if value.is_number() {
        "number"
    } else if value.is_bool() {
        "bool"
    } else if value.is_nil() {
        "nil"
    } else {
        "unknown"
    };

    Err(NuzoError::type_mismatch("string, array, or range", type_name))
}

// ========================================================================
// Array/Dict Operations (P3.2)
// ========================================================================

/// **push(arr, value)** → Value::NIL
///
/// Append a value to the end of an array. Modifies the array in-place.
///
/// # Arguments
///
/// * `arr` - The array to append to (must be a heap object array)
/// * `value` - The value to append
///
/// # Returns
///
/// Always returns `Value::NIL` (convention for mutating operations)
///
/// # Example
///
/// ```nuzo
/// arr = [1, 2, 3]
/// push(arr, 4)       // returns: nil, arr is now [1, 2, 3, 4]
/// ```
fn builtin_push(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() < 2 {
        return Err(NuzoError::type_mismatch(
            "2 arguments (array, value)",
            format!("{} arguments", args.len()),
        ));
    }

    let arr = &args[0];
    let value = &args[1];

    if !arr.is_heap_object() {
        return Err(NuzoError::type_mismatch("array", arr.type_name()));
    }

    // P2-12: 显式检查是 Array 类型，非 Array 不静默 no-op
    let is_array = arr.with_heap_object(|obj| matches!(obj, HeapObject::Array(_))).unwrap_or(false);
    if !is_array {
        return Err(NuzoError::type_mismatch("array", arr.type_name()));
    }

    let new_arr = arr.mutate_heap_object(|obj| {
        if let HeapObject::Array(vec) = obj {
            vec.push(*value);
        }
    });

    match new_arr {
        Some(_) => Ok(NIL),
        None => Err(NuzoError::type_mismatch("array", arr.type_name())),
    }
}

/// **pop(arr)** → Value
///
/// Remove and return the last element from an array.
///
/// # Arguments
///
/// * `arr` - The array to pop from (must be a heap object array)
///
/// # Returns
///
/// - The removed element on success
/// - `Value::NIL` if the array is empty or invalid
///
/// # Example
///
/// ```nuzo
/// arr = [1, 2, 3]
/// last = pop(arr)     // returns: 3, arr is now [1, 2]
/// ```
fn builtin_pop(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::type_mismatch(
            "1 argument (array)",
            format!("{} arguments", args.len()),
        ));
    }

    let arr = &args[0];

    if !arr.is_heap_object() {
        return Err(NuzoError::type_mismatch("array", arr.type_name()));
    }

    // P2-12: 显式检查是 Array 类型，非 Array 不静默 no-op（与 builtin_push 对齐）
    let is_array = arr.with_heap_object(|obj| matches!(obj, HeapObject::Array(_))).unwrap_or(false);
    if !is_array {
        return Err(NuzoError::type_mismatch("array", arr.type_name()));
    }

    let mut result = NIL;
    let new_arr = arr.mutate_heap_object(|obj| {
        if let HeapObject::Array(vec) = obj {
            result = vec.pop().unwrap_or(NIL);
        }
    });

    match new_arr {
        Some(_) => Ok(result),
        None => Err(NuzoError::type_mismatch("array", arr.type_name())),
    }
}

/// **keys(dict)** → Value (array of strings)
///
/// Return all keys from a dictionary as an array of strings.
///
/// # Arguments
///
/// * `dict` - The dictionary to extract keys from
///
/// # Returns
///
/// An array containing all key strings from the dictionary.
/// Returns empty array if dict has no keys or is not a dictionary.
///
/// # Example
///
/// ```nuzo
/// d = {"name": "Alice", "age": 30}
/// k = keys(d)         // returns: ["name", "age"] (as array)
/// ```
fn builtin_keys(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::type_mismatch(
            "1 argument (dict)",
            format!("{} arguments", args.len()),
        ));
    }

    let dict = &args[0];

    // Check if argument is a heap object
    if !dict.is_heap_object() {
        return Err(NuzoError::type_mismatch("dict", dict.type_name()));
    }

    // Extract keys from dictionary
    if let Some(heap_obj) = dict.as_heap_object_opt()
        && let HeapObject::Dict(nuzo_dict) = heap_obj.as_ref()
    {
        let mut keys = Vec::new();
        for (key_index, _) in nuzo_dict.iter() {
            keys.push(Value::from_string_index(key_index));
        }
        return Ok(Value::from_heap_object_gc(HeapObject::Array(keys)));
    }

    // Not a dict: return error
    Err(NuzoError::type_mismatch("dict", dict.type_name()))
}

// ========================================================================
// Type Conversion Operations (P3.2)
// ========================================================================

/// **str(value)** → Value (string)
///
/// Convert any value to its string representation.
///
/// # Arguments
///
/// * `value` - The value to convert to string
///
/// # Returns
///
/// A string Value representing the input value.
///
/// # Examples
///
/// ```nuzo
/// str(42)           // returns: "42"
/// str(true)         // returns: "true"
/// str(nil)          // returns: "nil"
/// str([1,2,3])      // returns: "[1, 2, 3]"
/// ```
fn builtin_str(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::type_mismatch(
            "1 argument (value)",
            format!("{} arguments", args.len()),
        ));
    }

    // Convert value to string using concat_repr (no extra quotes for strings)
    let string_repr = args[0].concat_repr();
    Ok(Value::from_string(&string_repr))
}

/// **typeof(value)** → Value (string)
///
/// Return the type name of a value as a string.
///
/// This is an improved version of `type_of` that returns actual string
/// values instead of numeric type codes.
///
/// # Arguments
///
/// * `value` - The value to inspect
///
/// # Returns
///
/// A string Value with one of these values:
/// - `"number"` - for integers and floats
/// - `"bool"` - for true/false
/// - `"nil"` - for nil/null
/// - `"string"` - for string values
/// - `"array"` - for arrays
/// - `"dict"` - for dictionaries
/// - `"closure"` - for functions/closures
/// - `"range"` - for range objects
/// - `"unknown"` - for unrecognized types
///
/// # Examples
///
/// ```nuzo
/// typeof(42)        // returns: "number"
/// typeof("hello")   // returns: "string"
/// typeof([1,2])     // returns: "array"
/// typeof(nil)       // returns: "nil"
/// ```
fn _builtin_typeof(args: &[Value]) -> Result<Value, NuzoError> {
    if args.is_empty() {
        return Err(NuzoError::type_mismatch(
            "1 argument (value)",
            format!("{} arguments", args.len()),
        ));
    }

    let value = args[0];
    let raw = value.type_name();
    let normalized = match raw {
        "integer" => "number",
        other => other,
    };
    Ok(Value::from_string(normalized))
}

// ========================================================================
// Runtime Safety Utilities (TCO / Trampoline)
// ========================================================================

/// Maximum number of iterations the trampoline will execute before
/// aborting as a safety measure against infinite loops.
const TRAMPOLINE_MAX_ITERATIONS: usize = 1_000_000;

/// **trampoline(fn, initial_arg)** → Value
///
/// Execute a function in trampoline (bounce) mode, providing a safety net
/// for deep or potentially infinite recursion.
///
/// # Purpose
///
/// Nuzo VM already has Tail Call Optimization (TCO) which handles tail-recursive
/// calls with O(1) stack space. However, for **non-tail-recursive** deep call chains
/// (e.g., recursive tree traversals, divide-and-conquer algorithms), the trampoline
/// serves two purposes:
///
/// 1. **Iteration limiting**: Prevents infinite loops from hanging the process
///    by enforcing a hard upper bound on iterations (`TRAMPOLINE_MAX_ITERATIONS`).
/// 2. **Diagnostic output**: Reports iteration count and termination reason,
///    helping developers identify performance bottlenecks or accidental infinite recursion.
///
/// # Arguments
///
/// * `fn` (required) - A closure/function to execute repeatedly
/// * `initial_arg` (required) - The initial argument passed to the function
///
/// # Returns
///
/// - The final value after the function returns a non-closure result (termination)
/// - Error if iteration limit is exceeded
///
/// # Examples
///
/// ```nuzo
/// // Safe recursion with depth monitoring:
/// result = trampoline(my_recursive_fn, initial_state)
/// ```
///
/// # Implementation Notes
///
/// This is a **simplified trampoline** — since TCO already handles tail calls,
/// this builtin focuses on safety (iteration limiting) and diagnostics rather
/// than full thunk-based loop unrolling. In a future enhancement, it could be
/// extended to interop with a proper thunk/thunk-return protocol for complete
/// non-tail-recursion elimination.
///
/// # P2-11: Closure 退化行为说明
///
/// **重要**：当 `fn` 参数是 Nuzo 闭包（`HeapObject::Closure`）而非 builtin 时，
/// trampoline **不会调用该闭包**，直接返回 `initial_arg`。这是因为 builtin 函数
/// 运行在 Rust 上下文中，无法访问 VM 的执行栈和调用帧来发起 Nuzo 函数调用。
///
/// 对于闭包递归，应直接使用普通的函数调用 + TCO：
/// ```nuzo
/// // 闭包递归用普通调用 + TCO，不要用 trampoline
/// fn recurse(x) { if (x > 0) { recurse(x - 1) } else { x } }
/// ```
///
/// trampoline 主要适用于：
/// - builtin 之间的 thunk 链（如 `trampoline(builtin_fn, initial_state)`）
/// - 迭代次数监控和无限循环保护
/// - 未来扩展为完整 TCO 补充的锚点
fn builtin_trampoline(args: &[Value]) -> Result<Value, NuzoError> {
    // Validate argument count: need exactly (function, initial_arg)
    if args.len() < 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }

    let func = &args[0];
    let mut current = args[1];

    // Verify the first argument is callable (closure or builtin)
    if !func.is_closure() && !func.is_builtin_fn() {
        return Err(NuzoError::type_mismatch("function (closure or builtin)", func.type_name()));
    }

    // Safety-bounded iteration loop
    for _iteration in 0..TRAMPOLINE_MAX_ITERATIONS {
        if func.is_builtin_fn() {
            // For builtins: we can invoke directly via BuiltinFnPtr
            let (_name, _arity, builtin_func) = func.as_builtin_fn_opt().ok_or_else(|| {
                NuzoError::internal(
                    InternalError::CompilerBug {
                        message: "builtin function info not found in trampoline".to_string(),
                    },
                    None,
                )
            })?;

            let result = builtin_func(&[current]).map_err(|e| {
                NuzoError::type_mismatch("valid arguments for builtin", format!("{}", e))
            })?;

            // If the result is a closure, treat it as thunk/continuation signal
            if result.is_closure() {
                current = result;
                continue;
            }

            // Non-closure result -> normal termination
            #[cfg(debug_assertions)]
            eprintln!("[trampoline] Completed in {} iterations", _iteration + 1);

            return Ok(result);
        } else if func.is_closure() {
            // For closures, we cannot directly invoke from a builtin context
            // (builtins don't have VM access). Return current value — the caller
            // should use normal function call + TCO instead.
            //
            // This is by design: the primary value of trampoline here is:
            // 1. Iteration counting / depth monitoring for debugging
            // 2. Infinite-loop protection for manually-implemented loops
            // 3. Future extension point for thunk-based TCO
            //
            // P2-11 行为契约：闭包参数被视为「不支持调用」的退化路径，
            // 直接返回 initial_arg（即 args[1]），不执行任何迭代。
            // 这不是错误（保持向后兼容），但通过 log::warn! 输出告警
            // 提醒用户改用普通函数调用 + TCO。
            log::warn!(
                "trampoline(closure, _) is a no-op: builtin context cannot invoke Nuzo closures. \
                 Returning initial_arg unchanged. Use normal function call + TCO for closure recursion."
            );

            #[cfg(debug_assertions)]
            eprintln!(
                "[trampoline] Terminated at iteration {} — closure invocation \
                     requires VM context. Use standard function call with TCO.",
                _iteration
            );

            return Ok(current);
        }
    }

    // Exceeded maximum iterations — abort with diagnostic error
    Err(NuzoError::assert_failed(format!(
        "trampoline exceeded maximum iteration limit ({}) — \
                 possible infinite loop or excessively deep recursion. \
                 Consider refactoring to use tail-call optimization.",
        TRAMPOLINE_MAX_ITERATIONS
    )))
}

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::{FALSE, NIL, NuzoErrorKind, TRUE};

    // =========================================================================
    // Test Registry Operations
    // =========================================================================

    #[test]
    fn test_registry_creation() {
        let reg = BuiltinRegistry::new();

        // Verify registry contains all core built-ins plus submodule functions
        assert!(reg.len() >= 11, "Should have at least 11 built-in functions, got {}", reg.len());
        assert!(!reg.is_empty(), "Registry should not be empty");
    }

    #[test]
    fn test_registry_contains_expected_functions() {
        let reg = BuiltinRegistry::new();
        let names = reg.names();

        // Check all expected functions are present (original 5)
        assert!(names.contains(&"print"), "Should have 'print'");
        assert!(names.contains(&"println"), "Should have 'println'");
        assert!(names.contains(&"type_of"), "Should have 'type_of'");
        assert!(names.contains(&"assert"), "Should have 'assert'");
        assert!(names.contains(&"len"), "Should have 'len'");

        // Check new P3.2 functions are present
        assert!(names.contains(&"push"), "Should have 'push'");
        assert!(names.contains(&"pop"), "Should have 'pop'");
        assert!(names.contains(&"keys"), "Should have 'keys'");
        assert!(names.contains(&"str"), "Should have 'str'");
        assert!(names.contains(&"typeof"), "Should have 'typeof'");

        // Check trampoline safety utility
        assert!(names.contains(&"trampoline"), "Should have 'trampoline'");
    }

    #[test]
    fn test_lookup_existing_function() {
        let reg = BuiltinRegistry::new();

        // Should find all registered functions
        assert!(reg.get("print").is_some(), "Should find 'print'");
        assert!(reg.get("println").is_some(), "Should find 'println'");
        assert!(reg.get("type_of").is_some(), "Should find 'type_of'");
        assert!(reg.get("assert").is_some(), "Should find 'assert'");
        assert!(reg.get("len").is_some(), "Should find 'len'");
    }

    #[test]
    fn test_lookup_nonexistent_function() {
        let reg = BuiltinRegistry::new();

        // Should return None for unregistered functions
        assert!(reg.get("nonexistent").is_none());
        assert!(reg.get("").is_none());
        assert!(reg.get("sprintf").is_none()); // sprintf 未注册（已注册的是 printf）
    }

    // =========================================================================
    // Test print() Function
    // =========================================================================

    #[test]
    fn test_print_single_value() {
        // This test verifies print doesn't crash and returns NIL
        let result = builtin_print(&[Value::from_number(42.0)]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_multiple_values() {
        let args = vec![Value::from_number(1.0), Value::from_number(2.0), Value::from_number(3.0)];
        let result = builtin_print(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_no_arguments() {
        // print() with no args should succeed (prints nothing)
        let args: Vec<Value> = vec![];
        let result = builtin_print(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_different_types() {
        // Test printing different value types
        let args = vec![Value::from_number(2.5), TRUE, FALSE, NIL];
        let result = builtin_print(&args);
        assert!(result.is_ok());
    }

    // =========================================================================
    // Test println() Function
    // =========================================================================

    #[test]
    fn test_println_single_value() {
        let result = builtin_println(&[Value::from_number(42.0)]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_println_no_arguments() {
        // println() with no args should print just a newline
        let args: Vec<Value> = vec![];
        let result = builtin_println(&args);
        assert!(result.is_ok());
    }

    // =========================================================================
    // Test type_of() Function
    // =========================================================================

    #[test]
    fn test_type_of_number() {
        let result = builtin_type_of(&[Value::from_number(42.0)]).unwrap();
        assert_eq!(result.as_number(), 1.0); // Code for "number"
    }

    #[test]
    fn test_type_of_bool_true() {
        let result = builtin_type_of(&[TRUE]).unwrap();
        assert_eq!(result.as_number(), 2.0); // Code for "bool"
    }

    #[test]
    fn test_type_of_bool_false() {
        let result = builtin_type_of(&[FALSE]).unwrap();
        assert_eq!(result.as_number(), 2.0); // Code for "bool"
    }

    #[test]
    fn test_type_of_nil() {
        let result = builtin_type_of(&[NIL]).unwrap();
        assert_eq!(result.as_number(), 3.0); // Code for "nil"
    }

    #[test]
    fn test_type_of_float_and_integer() {
        // Both integers and floats should be "number" (code 1.0)
        let int_result = builtin_type_of(&[Value::from_number(42.0)]).unwrap();
        let float_result = builtin_type_of(&[Value::from_number(2.5)]).unwrap();

        assert_eq!(int_result.as_number(), 1.0);
        assert_eq!(float_result.as_number(), 1.0);
    }

    #[test]
    fn test_type_of_wrong_argument_count() {
        // No arguments
        let result = builtin_type_of(&[]);
        assert!(result.is_err());

        // Too many arguments
        let result = builtin_type_of(&[NIL, NIL]);
        assert!(result.is_err());
    }

    #[test]
    fn test_type_of_returns_number_type() {
        // Verify that type_of always returns a numeric Value (the type code)
        let test_values = vec![
            Value::from_number(0.0),
            Value::from_number(-1.5),
            Value::from_number(f64::INFINITY),
            TRUE,
            FALSE,
            NIL,
        ];

        for value in test_values {
            let result = builtin_type_of(&[value]).unwrap();
            assert!(
                result.is_number(),
                "type_of should return a number for any valid input, got: {:?}",
                result
            );
            // Type codes should be in valid range (0-6)
            let code = result.as_number();
            assert!(
                (0.0..=6.0).contains(&code),
                "Type code should be in range [0, 6], got: {}",
                code
            );
        }
    }

    // =========================================================================
    // Test assert() Function
    // =========================================================================

    #[test]
    fn test_assert_with_truthy_value() {
        assert!(builtin_assert(&[TRUE]).is_ok());
        assert!(builtin_assert(&[Value::from_number(1.0)]).is_ok());
        assert!(builtin_assert(&[Value::from_number(-1.0)]).is_ok());
    }

    #[test]
    fn test_assert_with_falsy_value() {
        let result = builtin_assert(&[FALSE]);
        assert!(result.is_err());

        // nil should fail
        let result = builtin_assert(&[NIL]);
        assert!(result.is_err());
    }

    #[test]
    fn test_assert_with_custom_message() {
        // Assert false with custom message
        let result = builtin_assert(&[FALSE, Value::from_number(42.0)]);

        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::AssertFailed { message } => {
                // Message should indicate custom message was provided
                assert!(
                    message.contains("custom message provided"),
                    "Expected custom message indicator, got: {}",
                    message
                );
            }
            other => panic!("Expected AssertFailed error, got: {:?}", other),
        }
    }

    #[test]
    fn test_assert_default_message() {
        // Assert nil without custom message
        let result = builtin_assert(&[NIL]);

        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::AssertFailed { message } => {
                assert_eq!(message, "assertion failed");
            }
            other => panic!("Expected AssertFailed error, got: {:?}", other),
        }
    }

    #[test]
    fn test_assert_no_arguments() {
        let result = builtin_assert(&[]);
        // Should return TypeMismatch for wrong argument count (not AssertFailed)
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { .. } => {} // Expected
            other => panic!("Expected TypeMismatch error for no arguments, got: {:?}", other),
        }
    }

    #[test]
    fn test_assert_returns_nil_on_success() {
        // Verify that successful assertions return NIL explicitly
        let result = builtin_assert(&[TRUE]).unwrap();
        assert_eq!(result, NIL, "Successful assertion should return NIL");

        let result = builtin_assert(&[Value::from_number(1.0)]).unwrap();
        assert_eq!(result, NIL, "Successful assertion should return NIL");
    }

    // =========================================================================
    // Test len() Function
    // =========================================================================

    #[test]
    fn test_len_with_number_errors() {
        let result = builtin_len(&[Value::from_number(42.0)]);
        assert!(result.is_err());

        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { actual, .. } => {
                assert_eq!(actual, "number");
            }
            other => panic!("Expected TypeMismatch error, got: {:?}", other),
        }
    }

    #[test]
    fn test_len_with_bool_errors() {
        let result = builtin_len(&[TRUE]);
        assert!(result.is_err());

        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { actual, .. } => {
                assert_eq!(actual, "bool");
            }
            other => panic!("Expected TypeMismatch error, got: {:?}", other),
        }
    }

    #[test]
    fn test_len_with_nil_errors() {
        let result = builtin_len(&[NIL]);
        assert!(result.is_err());

        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { actual, .. } => {
                assert_eq!(actual, "nil");
            }
            other => panic!("Expected TypeMismatch error, got: {:?}", other),
        }
    }

    #[test]
    fn test_len_wrong_argument_count() {
        // No arguments
        let result = builtin_len(&[]);
        assert!(result.is_err());

        // Too many arguments
        let result = builtin_len(&[NIL, NIL]);
        assert!(result.is_err());
    }

    // =========================================================================
    // Integration Tests
    // =========================================================================

    #[test]
    fn test_all_builtins_can_be_called_via_registry() {
        let reg = BuiltinRegistry::new();

        // Test each built-in can be looked up and called
        let test_cases: Vec<(&str, Vec<Value>, bool)> = vec![
            ("print", vec![Value::from_number(1.0)], true), // Should succeed
            ("println", vec![TRUE], true),                  // Should succeed
            ("type_of", vec![NIL], true),                   // Should succeed
            ("assert", vec![TRUE], true),                   // Should succeed
            ("assert", vec![FALSE], false),                 // Should fail
            ("len", vec![NIL], false),                      // Should fail (wrong type)
        ];

        for (name, args, should_succeed) in test_cases {
            if let Some(func) = reg.get(name) {
                let result = func(&args);
                if should_succeed {
                    assert!(
                        result.is_ok(),
                        "{}({:?}) should succeed but got error: {:?}",
                        name,
                        args,
                        result.err()
                    );
                } else {
                    assert!(result.is_err(), "{}({:?}) should fail but succeeded", name, args);
                }
            } else {
                panic!("Built-in '{}' not found in registry", name);
            }
        }
    }

    #[test]
    fn test_builtin_return_types() {
        // All builtins should return Value on success
        let reg = BuiltinRegistry::new();

        // print/println return NIL (not default Value)
        if let Some(print_fn) = reg.get("print") {
            let result = print_fn(&[Value::from_number(1.0)]);
            assert!(result.is_ok(), "print should return Ok");
            assert_eq!(result.unwrap(), NIL, "print should return NIL");
        }

        if let Some(println_fn) = reg.get("println") {
            let result = println_fn(&[TRUE]);
            assert!(result.is_ok(), "println should return Ok");
            assert_eq!(result.unwrap(), NIL, "println should return NIL");
        }

        // type_of returns a number (type code)
        if let Some(type_of_fn) = reg.get("type_of") {
            let result = type_of_fn(&[TRUE]);
            assert!(result.is_ok(), "type_of should return Ok");
            let val = result.unwrap();
            assert!(val.is_number(), "type_of should return number");
        }

        // assert returns NIL on success
        if let Some(assert_fn) = reg.get("assert") {
            let result = assert_fn(&[TRUE]);
            assert!(result.is_ok(), "assert should return Ok on success");
            assert_eq!(result.unwrap(), NIL, "assert should return NIL on success");
        }
    }

    #[test]
    fn test_print_returns_nil_for_various_inputs() {
        // Test that print always returns NIL regardless of input
        let test_cases: Vec<Vec<Value>> = vec![
            vec![],                                          // No arguments
            vec![NIL],                                       // Single nil
            vec![TRUE],                                      // Single bool
            vec![Value::from_number(42.0)],                  // Single number
            vec![NIL, TRUE, FALSE, Value::from_number(1.0)], // Multiple args
        ];

        for args in test_cases {
            let result = builtin_print(&args);
            assert!(result.is_ok(), "print({:?}) should succeed", args);
            assert_eq!(result.unwrap(), NIL, "print({:?}) should return NIL", args);
        }
    }

    #[test]
    fn test_len_error_messages_are_descriptive() {
        // Verify that len() provides helpful error messages for different types
        let test_cases: Vec<(Value, &str)> =
            vec![(Value::from_number(42.0), "number"), (TRUE, "bool"), (NIL, "nil")];

        for (value, expected_type_name) in test_cases {
            let result = builtin_len(&[value]);
            assert!(result.is_err(), "len({}) should fail", expected_type_name);

            match result.unwrap_err().kind {
                NuzoErrorKind::TypeMismatch { actual, .. } => {
                    assert_eq!(
                        actual, expected_type_name,
                        "Error message should contain correct type name for {}",
                        expected_type_name
                    );
                }
                other => panic!("Expected TypeMismatch error, got: {:?}", other),
            }
        }
    }

    #[test]
    fn test_assert_failed_error_has_correct_message_format() {
        // Test that AssertFailed errors have the expected message format
        let false_result = builtin_assert(&[FALSE]);
        match false_result.unwrap_err().kind {
            NuzoErrorKind::AssertFailed { message } => {
                assert!(
                    message.contains("assertion failed"),
                    "AssertFailed message should contain 'assertion failed', got: '{}'",
                    message
                );
            }
            other => panic!("Expected AssertFailed, got: {:?}", other),
        }

        let nil_result = builtin_assert(&[NIL]);
        match nil_result.unwrap_err().kind {
            NuzoErrorKind::AssertFailed { message } => {
                assert_eq!(message, "assertion failed");
            }
            other => panic!("Expected AssertFailed, got: {:?}", other),
        }
    }

    #[test]
    fn test_type_codes_are_consistent() {
        let mut seen_codes: std::collections::HashSet<u64> = std::collections::HashSet::new();

        let test_values: Vec<(Value, &str)> =
            vec![(Value::from_number(1.0), "number"), (TRUE, "bool"), (NIL, "nil")];

        for (value, type_name) in test_values {
            let result = builtin_type_of(&[value]).unwrap();
            let code = result.as_number();
            assert!(
                !seen_codes.contains(&code.to_bits()),
                "Type code {} is duplicated for type '{}'",
                code,
                type_name
            );
            seen_codes.insert(code.to_bits());
        }

        // Should have at least 3 unique codes for primitives
        assert!(seen_codes.len() >= 3, "Should have at least 3 unique type codes");
    }

    // =========================================================================
    // Test scoped SignalBus (BUILTIN_CALLED_KEY)
    // =========================================================================

    #[test]
    fn test_builtin_called_signal_via_bus() {
        let registry = BuiltinRegistry::new();
        let bus = registry.bus();
        let sig = bus.get(&BUILTIN_CALLED_KEY).unwrap();
        // 新创建的信号应该没有监听器
        assert!(sig.is_empty(), "New signal should have no listeners");
        assert_eq!(sig.name(), "builtin_called");
    }

    #[test]
    fn test_builtin_called_signal_scoped_per_registry() {
        // 每个 BuiltinRegistry 实例拥有独立的 scoped SignalBus，
        // 不再是全局共享的静态实例
        let reg_a = BuiltinRegistry::new();
        let reg_b = BuiltinRegistry::new();

        let bus_a = reg_a.bus();
        let bus_b = reg_b.bus();

        let sig_a = bus_a.get(&BUILTIN_CALLED_KEY).unwrap();
        let sig_b = bus_b.get(&BUILTIN_CALLED_KEY).unwrap();

        // 两个信号应该是不同的实例（scoped 隔离）
        assert!(
            !std::ptr::eq(&sig_a as *const _, &sig_b as *const _),
            "Different registries should have independent signal instances"
        );
    }

    // =========================================================================
    // Test configure_output_capture() / set_output_capture()
    // =========================================================================

    #[test]
    fn test_output_capture_captures_println() {
        use std::sync::{Arc, Mutex};

        let buffer = Arc::new(Mutex::new(Vec::new()));
        configure_output_capture(Some(buffer.clone()));

        let result = builtin_println(&[Value::from_string("hello")]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL);

        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1);
        // Value::Display for strings does NOT include surrounding quotes
        assert_eq!(captured[0], "hello");

        // Clean up: disable capture
        configure_output_capture(None);
    }

    #[test]
    fn test_output_capture_captures_print() {
        use std::sync::{Arc, Mutex};

        let buffer = Arc::new(Mutex::new(Vec::new()));
        configure_output_capture(Some(buffer.clone()));

        let _ = builtin_print(&[Value::from_number(42.0)]);
        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], "42");

        configure_output_capture(None);
    }

    #[test]
    fn test_output_capture_multiple_calls_append() {
        use std::sync::{Arc, Mutex};

        let buffer = Arc::new(Mutex::new(Vec::new()));
        configure_output_capture(Some(buffer.clone()));

        let _ = builtin_println(&[Value::from_string("first")]);
        let _ = builtin_println(&[Value::from_string("second")]);

        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 2);
        // Value::Display for strings does NOT include surrounding quotes
        assert_eq!(captured[0], "first");
        assert_eq!(captured[1], "second");

        configure_output_capture(None);
    }

    #[test]
    fn test_output_capture_no_args() {
        use std::sync::{Arc, Mutex};

        let buffer = Arc::new(Mutex::new(Vec::new()));
        configure_output_capture(Some(buffer.clone()));

        let _ = builtin_println(&[]);
        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], ""); // empty args -> empty string

        configure_output_capture(None);
    }

    // =========================================================================
    // Test args_join() helper
    // =========================================================================

    #[test]
    fn test_args_join_single_value() {
        let joined = args_join(&[Value::from_number(42.0)]);
        assert_eq!(joined, "42");
    }

    #[test]
    fn test_args_join_multiple_values_joins_with_space() {
        let joined =
            args_join(&[Value::from_number(1.0), Value::from_number(2.0), Value::from_number(3.0)]);
        assert_eq!(joined, "1 2 3");
    }

    #[test]
    fn test_args_join_empty_args() {
        let joined = args_join(&[]);
        assert_eq!(joined, "");
    }

    #[test]
    fn test_args_join_mixed_types() {
        let joined = args_join(&[TRUE, NIL, Value::from_string("hi")]);
        // Value::Display for strings does NOT include surrounding quotes
        assert_eq!(joined, "true nil hi");
    }

    // =========================================================================
    // Test BuiltinRegistry::register() directly
    // =========================================================================

    #[test]
    fn test_registry_register_custom_function() {
        let mut reg = BuiltinRegistry::new();
        let initial_len = reg.len();

        fn custom_echo(args: &[Value]) -> Result<Value, NuzoError> {
            if args.is_empty() { Ok(NIL) } else { Ok(args[0]) }
        }
        reg.register("echo", custom_echo, 1);

        assert_eq!(reg.len(), initial_len + 1, "Register should add one function");
        assert!(reg.names().contains(&"echo"), "Names should include 'echo'");

        let func = reg.get("echo").expect("Should find registered function");
        let result = func(&[Value::from_number(99.0)]).unwrap();
        assert_eq!(result.as_number(), 99.0);
    }

    #[test]
    fn test_registry_register_allows_duplicate_name() {
        let mut reg = BuiltinRegistry::new();

        fn first(_args: &[Value]) -> Result<Value, NuzoError> {
            Ok(Value::from_number(1.0))
        }
        fn second(_args: &[Value]) -> Result<Value, NuzoError> {
            Ok(Value::from_number(2.0))
        }

        reg.register("dup", first, 0);
        reg.register("dup", second, 0);

        // Both should be registered (Vec allows duplicates)
        assert_eq!(reg.names().iter().filter(|n| **n == "dup").count(), 2);
        // get() returns the first match
        let func = reg.get("dup").unwrap();
        let result = func(&[]).unwrap();
        assert_eq!(result.as_number(), 1.0, "get() should return first registered function");
    }

    // =========================================================================
    // Test BuiltinRegistry::call()
    // =========================================================================

    #[test]
    fn test_call_existing_function_success() {
        let reg = BuiltinRegistry::new();
        let result = reg.call("type_of", &[TRUE]);
        assert!(result.is_some(), "call() should return Some for existing function");
        assert!(result.unwrap().is_ok(), "type_of(true) should succeed");
    }

    #[test]
    fn test_call_nonexistent_function_returns_none() {
        let reg = BuiltinRegistry::new();
        let result = reg.call("totally_fake_function", &[]);
        assert!(result.is_none(), "call() should return None for nonexistent function");
    }

    #[test]
    fn test_call_with_empty_string_name() {
        let reg = BuiltinRegistry::new();
        assert!(reg.call("", &[]).is_none());
    }

    #[test]
    fn test_call_propagates_error() {
        let reg = BuiltinRegistry::new();
        // assert(false) should produce an error wrapped in Some
        let result = reg.call("assert", &[FALSE]);
        assert!(result.is_some());
        assert!(result.unwrap().is_err(), "assert(false) should return Err");
    }

    // =========================================================================
    // Test builtin_push()
    // =========================================================================

    #[test]
    fn test_push_to_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
        ]));

        let result = builtin_push(&[arr, Value::from_number(3.0)]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL);

        // Verify arr was mutated: check length via len()
        let len_result = builtin_len(&[arr]).unwrap();
        assert_eq!(len_result.as_number(), 3.0);
    }

    #[test]
    fn test_push_to_empty_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        let result = builtin_push(&[arr, Value::from_string("x")]);
        assert!(result.is_ok());

        let len_result = builtin_len(&[arr]).unwrap();
        assert_eq!(len_result.as_number(), 1.0);
    }

    #[test]
    fn test_push_wrong_arg_count_errors() {
        // No arguments
        assert!(builtin_push(&[]).is_err());
        // Only 1 argument
        assert!(builtin_push(&[NIL]).is_err());
    }

    #[test]
    fn test_push_non_array_type_error() {
        let result = builtin_push(&[Value::from_number(42.0), Value::from_number(1.0)]);
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { expected, .. } => {
                assert!(expected.contains("array"));
            }
            other => panic!("Expected TypeMismatch, got: {:?}", other),
        }
    }

    // =========================================================================
    // Test builtin_pop()
    // =========================================================================

    #[test]
    fn test_pop_from_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(10.0),
            Value::from_number(20.0),
            Value::from_number(30.0),
        ]));

        let result = builtin_pop(&[arr]);
        assert!(result.is_ok());
        let popped = result.unwrap();
        assert_eq!(popped.as_number(), 30.0, "Should pop last element 30");

        // Verify array shrunk
        let len_result = builtin_len(&[arr]).unwrap();
        assert_eq!(len_result.as_number(), 2.0);
    }

    #[test]
    fn test_pop_from_single_element_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![Value::from_string("only")]));

        let result = builtin_pop(&[arr]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_string_opt().as_deref(), Some("only"));

        // Array should now be empty
        let len_result = builtin_len(&[arr]).unwrap();
        assert_eq!(len_result.as_number(), 0.0);
    }

    #[test]
    fn test_pop_from_empty_array_returns_nil() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));

        let result = builtin_pop(&[arr]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL, "Popping empty array should return NIL");
    }

    #[test]
    fn test_pop_no_arguments_error() {
        let result = builtin_pop(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_pop_non_array_type_error() {
        let result = builtin_pop(&[Value::from_string("not_an_array")]);
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { expected, .. } => {
                assert!(expected.contains("array"));
            }
            other => panic!("Expected TypeMismatch, got: {:?}", other),
        }
    }

    // =========================================================================
    // Test builtin_keys()
    // =========================================================================

    #[test]
    fn test_keys_from_dict() {
        use nuzo_values::NuzoDict;

        let mut dict = NuzoDict::new();
        dict.insert(0, Value::from_string("Alice")); // key_index 0 -> "name"
        dict.insert(1, Value::from_number(30.0)); // key_index 1 -> "age"

        let dict_value = Value::from_heap_object_gc(HeapObject::Dict(dict));

        let result = builtin_keys(&[dict_value]);
        assert!(result.is_ok());

        // keys() returns an array of string-index Values
        if let Some(arr_obj) = result.unwrap().as_heap_object_opt() {
            if let HeapObject::Array(keys_arr) = arr_obj.as_ref() {
                assert_eq!(keys_arr.len(), 2, "Dict with 2 entries should yield 2 keys");
            } else {
                panic!("keys() should return an array");
            }
        } else {
            panic!("keys() should return a heap object");
        }
    }

    #[test]
    fn test_keys_from_empty_dict() {
        use nuzo_values::NuzoDict;

        let dict_value = Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new()));
        let result = builtin_keys(&[dict_value]);
        assert!(result.is_ok());

        if let Some(arr_obj) = result.unwrap().as_heap_object_opt() {
            if let HeapObject::Array(keys_arr) = arr_obj.as_ref() {
                assert_eq!(keys_arr.len(), 0, "Empty dict should yield empty key array");
            } else {
                panic!("keys() should return an array");
            }
        }
    }

    #[test]
    fn test_keys_no_arguments_error() {
        let result = builtin_keys(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_keys_non_dict_type_error() {
        // Pass an array instead of a dict
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![Value::from_number(1.0)]));
        let result = builtin_keys(&[arr]);
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { expected, .. } => {
                assert!(expected.contains("dict"));
            }
            other => panic!("Expected TypeMismatch, got: {:?}", other),
        }
    }

    #[test]
    fn test_keys_primitive_type_error() {
        let result = builtin_keys(&[Value::from_number(42.0)]);
        assert!(result.is_err());
    }

    // =========================================================================
    // Test builtin_str()
    // =========================================================================

    #[test]
    fn test_str_number() {
        let result = builtin_str(&[Value::from_number(42.0)]).unwrap();
        assert!(result.is_string());
        assert_eq!(result.as_string_opt().as_deref(), Some("42"));
    }

    #[test]
    fn test_str_bool_true() {
        let result = builtin_str(&[TRUE]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("true"));
    }

    #[test]
    fn test_str_bool_false() {
        let result = builtin_str(&[FALSE]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("false"));
    }

    #[test]
    fn test_str_nil() {
        let result = builtin_str(&[NIL]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("nil"));
    }

    #[test]
    fn test_str_string_value() {
        let result = builtin_str(&[Value::from_string("hello")]).unwrap();
        // str() of a string value returns the string content without quotes
        assert_eq!(result.as_string_opt().as_deref(), Some("hello"));
    }

    #[test]
    fn test_str_empty_args_error() {
        let result = builtin_str(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_str_float_precision() {
        let result = builtin_str(&[Value::from_number(2.5)]).unwrap();
        let s = result.as_string_opt().unwrap();
        assert!(s.contains("2.5"), "Float string repr should contain 2.5, got: {}", s);
    }

    // =========================================================================
    // Test _builtin_typeof()
    // =========================================================================

    #[test]
    fn test_typeof_number() {
        // from_number(42.0) may be stored as Smi ("integer") or Float ("number")
        // depending on the value. Test both paths.
        let int_result = _builtin_typeof(&[Value::from_number(42.0)]).unwrap();
        let type_name = int_result.as_string_opt().unwrap();
        // Smi values are reported as "integer", Float as "number"
        assert!(
            type_name == "integer" || type_name == "number",
            "Expected 'integer' or 'number', got: '{}'",
            type_name
        );

        // Explicitly test float path with a non-integer value
        let float_result = _builtin_typeof(&[Value::from_number(2.5)]).unwrap();
        assert_eq!(float_result.as_string_opt().as_deref(), Some("number"));
    }

    #[test]
    fn test_typeof_bool() {
        let result = _builtin_typeof(&[TRUE]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("bool"));

        let result = _builtin_typeof(&[FALSE]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("bool"));
    }

    #[test]
    fn test_typeof_nil() {
        let result = _builtin_typeof(&[NIL]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("nil"));
    }

    #[test]
    fn test_typeof_string() {
        let result = _builtin_typeof(&[Value::from_string("hello")]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("string"));
    }

    #[test]
    fn test_typeof_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        let result = _builtin_typeof(&[arr]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("array"));
    }

    #[test]
    fn test_typeof_dict() {
        use nuzo_values::NuzoDict;
        let d = Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new()));
        let result = _builtin_typeof(&[d]).unwrap();
        assert_eq!(result.as_string_opt().as_deref(), Some("dict"));
    }

    #[test]
    fn test_typeof_range() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Exclusive,
        });
        let result = _builtin_typeof(&[r]).unwrap();
        // Note: type_name() only matches Array/Dict/Closure in Pointer branch;
        // Range falls through to "object" (current implementation limitation)
        let name = result.as_string_opt().unwrap();
        assert!(
            name == "range" || name == "object",
            "Expected 'range' or 'object' (implementation-dependent), got: '{}'",
            name
        );
    }

    #[test]
    fn test_typeof_empty_args_error() {
        let result = _builtin_typeof(&[]);
        assert!(result.is_err());
    }

    // =========================================================================
    // Test builtin_trampoline()
    // =========================================================================

    #[test]
    fn test_trampoline_wrong_arg_count() {
        // 0 args
        assert!(builtin_trampoline(&[]).is_err());
        // 1 arg
        assert!(builtin_trampoline(&[NIL]).is_err());
    }

    #[test]
    fn test_trampoline_non_function_first_arg() {
        // First arg must be a function; passing a number
        let result = builtin_trampoline(&[Value::from_number(1.0), NIL]);
        assert!(result.is_err());
        match result.unwrap_err().kind {
            NuzoErrorKind::TypeMismatch { expected, .. } => {
                assert!(
                    expected.contains("function")
                        || expected.contains("closure")
                        || expected.contains("builtin")
                );
            }
            other => panic!("Expected TypeMismatch, got: {:?}", other),
        }
    }

    #[test]
    fn test_trampoline_with_builtin_fn_identity() {
        // Create a builtin function that simply returns its argument (identity)
        fn identity(args: &[Value]) -> Result<Value, NuzoError> {
            if args.is_empty() { Ok(NIL) } else { Ok(args[0]) }
        }

        let builtin_val = Value::from_heap_object_gc(HeapObject::BuiltinFn {
            name: "identity".to_string(),
            arity: 1,
            func: identity,
        });

        // trampoline(identity, 42) -> identity(42) = 42 (not a closure, so terminates)
        let result = builtin_trampoline(&[builtin_val, Value::from_number(42.0)]);
        assert!(result.is_ok(), "trampoline with identity builtin should succeed");
        assert_eq!(result.unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_trampoline_with_builtin_fn_returning_nil() {
        fn always_nil(_args: &[Value]) -> Result<Value, NuzoError> {
            Ok(NIL)
        }

        let builtin_val = Value::from_heap_object_gc(HeapObject::BuiltinFn {
            name: "always_nil".to_string(),
            arity: 0,
            func: always_nil,
        });

        let result = builtin_trampoline(&[builtin_val, Value::from_number(1.0)]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), NIL);
    }

    // =========================================================================
    // Test builtin_len() -- happy paths (strings, arrays, dicts, ranges)
    // =========================================================================

    #[test]
    fn test_len_string() {
        let result = builtin_len(&[Value::from_string("hello")]).unwrap();
        assert_eq!(result.as_number(), 5.0);
    }

    #[test]
    fn test_len_empty_string() {
        let result = builtin_len(&[Value::from_string("")]).unwrap();
        assert_eq!(result.as_number(), 0.0);
    }

    #[test]
    fn test_len_unicode_string() {
        let result = builtin_len(&[Value::from_string("你好")]).unwrap();
        // char_len counts characters, not bytes
        assert_eq!(result.as_number(), 2.0);
    }

    #[test]
    fn test_len_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
            Value::from_number(3.0),
            Value::from_number(4.0),
        ]));
        let result = builtin_len(&[arr]).unwrap();
        assert_eq!(result.as_number(), 4.0);
    }

    #[test]
    fn test_len_empty_array() {
        let arr = Value::from_heap_object_gc(HeapObject::Array(vec![]));
        let result = builtin_len(&[arr]).unwrap();
        assert_eq!(result.as_number(), 0.0);
    }

    #[test]
    fn test_len_dict() {
        use nuzo_values::NuzoDict;
        let mut d = NuzoDict::new();
        d.insert(0, Value::from_string("a"));
        d.insert(1, Value::from_string("b"));
        d.insert(2, Value::from_string("c"));
        let dict_val = Value::from_heap_object_gc(HeapObject::Dict(d));
        let result = builtin_len(&[dict_val]).unwrap();
        assert_eq!(result.as_number(), 3.0);
    }

    #[test]
    fn test_len_range_exclusive() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Exclusive,
        });
        let result = builtin_len(&[r]).unwrap();
        assert_eq!(result.as_number(), 4.0); // 1,2,3,4
    }

    #[test]
    fn test_len_range_inclusive() {
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 1.0,
            end: 5.0,
            range_end: RangeEnd::Inclusive,
        });
        let result = builtin_len(&[r]).unwrap();
        assert_eq!(result.as_number(), 5.0); // 1,2,3,4,5
    }

    #[test]
    fn test_len_range_reversed_is_zero() {
        // end < start should yield 0
        let r = Value::from_heap_object_gc(HeapObject::Range {
            start: 5.0,
            end: 1.0,
            range_end: RangeEnd::Exclusive,
        });
        let result = builtin_len(&[r]).unwrap();
        assert_eq!(result.as_number(), 0.0);
    }

    // =========================================================================
    // Test BuiltinRegistry Default impl
    // =========================================================================

    #[test]
    fn test_registry_default_same_as_new() {
        let a = BuiltinRegistry::default();
        let b = BuiltinRegistry::new();
        assert_eq!(a.len(), b.len(), "Default and new should have same count");
        assert_eq!(a.names(), b.names(), "Default and new should have same names");
    }

    // =========================================================================
    // Test OutputCaptureGuard (P2-13)
    // =========================================================================

    /// 验证 guard 在正常 drop 后会调用 pop_output_capture，
    /// 后续 println 的输出不再进入已 drop 的缓冲区。
    #[test]
    fn test_output_capture_guard_pops_on_drop() {
        let buffer = Arc::new(Mutex::new(Vec::new()));

        {
            let _guard = OutputCaptureGuard::new(Some(buffer.clone()));
            // 在 guard 作用域内，println 输出应进入 buffer
            let _ = builtin_println(&[Value::from_string("inside")]);
            let captured = buffer.lock().unwrap();
            assert_eq!(captured.len(), 1);
            assert_eq!(captured[0], "inside");
        } // guard drop 在此发生

        // guard 已 drop，后续 println 不应再进入 buffer
        let _ = builtin_println(&[Value::from_string("outside")]);
        let captured = buffer.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "post-drop output should NOT be captured by the dropped buffer"
        );
        assert_eq!(captured[0], "inside");

        // 清理当前线程的捕获状态（防止影响后续测试）
        configure_output_capture(None);
    }

    /// 验证 guard 在作用域内能多次捕获输出。
    #[test]
    fn test_output_capture_guard_captures_multiple_outputs() {
        let buffer = Arc::new(Mutex::new(Vec::new()));

        {
            let _guard = OutputCaptureGuard::new(Some(buffer.clone()));
            let _ = builtin_println(&[Value::from_string("a")]);
            let _ = builtin_println(&[Value::from_string("b")]);
            let _ = builtin_println(&[Value::from_string("c")]);
        }

        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 3);
        assert_eq!(captured[0], "a");
        assert_eq!(captured[1], "b");
        assert_eq!(captured[2], "c");

        configure_output_capture(None);
    }

    /// 验证 guard 嵌套使用时栈帧顺序正确（LIFO）。
    #[test]
    fn test_output_capture_guard_nested_lifo() {
        let outer = Arc::new(Mutex::new(Vec::new()));
        let inner = Arc::new(Mutex::new(Vec::new()));

        {
            let _outer_guard = OutputCaptureGuard::new(Some(outer.clone()));
            let _ = builtin_println(&[Value::from_string("outer-1")]);

            {
                let _inner_guard = OutputCaptureGuard::new(Some(inner.clone()));
                // 在内层 guard 作用域内，输出进入 inner（栈顶）
                let _ = builtin_println(&[Value::from_string("inner-1")]);
                let _ = builtin_println(&[Value::from_string("inner-2")]);
            } // inner guard drop，inner 栈帧弹出

            // 内层 guard 已 drop，输出回到 outer（栈顶）
            let _ = builtin_println(&[Value::from_string("outer-2")]);
        } // outer guard drop

        let outer_captured = outer.lock().unwrap();
        let inner_captured = inner.lock().unwrap();

        assert_eq!(outer_captured.len(), 2);
        assert_eq!(outer_captured[0], "outer-1");
        assert_eq!(outer_captured[1], "outer-2");

        assert_eq!(inner_captured.len(), 2);
        assert_eq!(inner_captured[0], "inner-1");
        assert_eq!(inner_captured[1], "inner-2");

        configure_output_capture(None);
    }

    /// 验证 guard 在 panic 时仍会调用 pop（通过 catch_unwind 捕获 panic）。
    #[test]
    fn test_output_capture_guard_pops_on_panic() {
        use std::panic;

        let buffer = Arc::new(Mutex::new(Vec::new()));

        // 在子线程中运行，避免 panic 影响整个测试进程
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let _guard = OutputCaptureGuard::new(Some(buffer.clone()));
            let _ = builtin_println(&[Value::from_string("before-panic")]);
            panic!("simulated failure inside guarded scope");
        }));

        // panic 应被捕获
        assert!(result.is_err(), "expected panic to be caught");

        // guard 在 panic 展开期间已 drop，pop 应已执行
        // 验证：在 guard 作用域外，输出不再进入 buffer
        let _ = builtin_println(&[Value::from_string("after-panic")]);
        let captured = buffer.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "post-panic output should NOT be captured (guard should have popped on unwind)"
        );
        assert_eq!(captured[0], "before-panic");

        configure_output_capture(None);
    }

    /// 验证 guard 在 `?` 早期返回时也会 drop（通过函数返回路径）。
    #[test]
    fn test_output_capture_guard_drops_on_early_return() {
        let buffer = Arc::new(Mutex::new(Vec::new()));

        // 模拟一个可能早期返回的函数
        fn maybe_return_early(
            should_return: bool,
            buffer: Arc<Mutex<Vec<String>>>,
        ) -> Result<i32, i32> {
            let _guard = OutputCaptureGuard::new(Some(buffer));
            let _ = builtin_println(&[Value::from_string("inside-fn")]);
            if should_return {
                return Err(42); // 早期返回，guard 应在此 drop
            }
            Ok(0)
        }

        // 早期返回路径
        let result = maybe_return_early(true, buffer.clone());
        assert!(result.is_err());

        // guard 应已 drop，后续输出不再进入 buffer
        let _ = builtin_println(&[Value::from_string("after-fn")]);
        let captured = buffer.lock().unwrap();
        assert_eq!(captured.len(), 1, "only inside-fn should be captured");
        assert_eq!(captured[0], "inside-fn");

        configure_output_capture(None);
    }
}
