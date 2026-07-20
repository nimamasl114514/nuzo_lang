//! # 全局常量与系统限制定义
//!
//! 本模块集中管理 Nuzo 运行时的**所有硬编码常量**，使用 `hardcode` crate 的
//! `define_constants!` 宏实现：
//!
//! - **编译期零开销**：真正的 `const`/`const fn`，无运行时查找
//! - **运行时可查询**：自动生成 getter 函数（`get_XXX()`）|
//! - **文档化**：每个常量附带用途说明和设计理由
//!
//! ## 常量分类
//!
//! ### 1. VM 核心资源限制
//! 控制虚拟机的内存分配和递归深度，防止恶意代码导致资源耗尽：
//!
//! | 常量 | 值 | 说明 |
//! |------|-----|------|
//! [`DEFAULT_MAX_STACK_SIZE`] | 65536 (64K) | 寄存器文件大小（栈深度）|
//! [`DEFAULT_MAX_CALL_FRAMES`] | 10000 | 最大函数调用嵌套层数 |
//! [`INITIAL_REGISTERS`] | 256 | VM 启动时预分配的寄存器数量 |
//! [`INITIAL_FRAME_CAPACITY`] | 64 | 预分配的调用帧数组容量 |
//!
//! ### 2. GC（垃圾回收）参数
//! 调整垃圾回收器的行为，在**暂停时间**和**吞吐量**之间权衡：
//!
//! | 常量 | 值 | 说明 |
//! |------|-----|------|
//! [`GC_MIN_THRESHOLD`] | 1 KB | GC 触发的最小内存阈值 |
//! [`GC_DEFAULT_THRESHOLD`] | 10 MB | 默认的 GC 触发阈值 |
//! [`GC_SURVIVAL_RATIO_THRESHOLD`] | 0.5 (50%) | 存活率低于此值则翻倍阈值 |
//! [`GC_THRESHOLD_GROWTH_FACTOR`] | 2x | 阈值增长倍数 |
//!
//! **调优建议**：
//! - 内存受限环境（嵌入式）：降低 `GC_DEFAULT_THRESHOLD` 至 1 MB
//! - 高吞吐场景：提高至 100 MB，减少 GC 频率
//! - 低延迟要求：降低阈值 + 使用增量式 GC（未来支持）|
//!
//! ### 3. 编译器限制
//! 限制编译器生成的代码复杂度：
//!
//! | 常量 | 值 | 说明 |
//! |------|-----|------|
//! [`MAX_LOCALS`] | 65535 | 单个编译单元最大局部变量数 |
//! [`MAX_FUNCTION_LOCALS`] | 255 | 单个函数内最大局部变量数 |
//!
//! ### 4. 编码检测用 BOM 字节序标记
//! 用于识别文件的字符编码（详见 [`encoding`] 模块）:
//!
//! | 常量 | 值 | 对应编码 |
//! |------|-----|---------|
//! [`UTF8_BOM_0..2`] | `EF BB BF` | UTF-8 |
//! [`UTF16_LE_BOM_0..1`] | `FF FE` | UTF-16 Little Endian |
//! [`UTF16_BE_BOM_0..1`] | `FE FF` | UTF-16 Big Endian |
//!
//! ### 5. 类型码系统
//! 用于运行时类型检查（RTTI）的数值标签：
//!
//! | 常量 | 值 | 含义 |
//! |------|-----|------|
//! [`TYPE_CODE_NUMBER`] | 1.0 | 数字类型 |
//! [`TYPE_CODE_BOOL`] | 2.0 | 布尔类型 |
//! [`TYPE_CODE_NIL`] | 3.0 | 空值类型 |
//! [`TYPE_CODE_OBJECT`] | 6.0 | 对象类型 |
//! [`TYPE_CODE_UNKNOWN`] | 0.0 | 未知类型（默认值）|
//!
//! ## 设计原则
//!
//! ### 为什么使用 `nuzo_proc_core::hardcode` 宏而非手动定义？
//!
//! 1. **单一真相源（SSOT）**: 所有常量集中在一处，避免散落各处的魔法数字
//! 2. **自动文档生成**: 可提取为配置文件或文档
//! 3. **运行时查询**: 支持动态调整（如从配置文件覆盖默认值）|
//! 4. **类型安全**: 每个常量都有明确的类型注解
//!
//! ### 性能保证
//!
//! 所有常量均为**编译期常量**，编译器会：
//! - 内联替换（消除间接寻址）|
//! - 常量折叠（如 `2 * DEFAULT_MAX_STACK_SIZE` 在编译期计算）|
//! - 死代码消除（未使用的常量不进入二进制）

use nuzo_proc_core::define_constants;

define_constants! {
    // ============================================================
    // VM 核心常量
    // ============================================================

    /// 默认最大栈大小（VM 构造时可配置覆盖）
    pub DEFAULT_MAX_STACK_SIZE: usize = 65536;

    /// 默认最大调用帧数（弹性栈模式下大幅提升，仅受系统内存限制）
    pub DEFAULT_MAX_CALL_FRAMES: usize = 1_000_000;

    /// 初始寄存器预分配数量
    pub INITIAL_REGISTERS: usize = 256;

    /// 帧预分配容量
    pub INITIAL_FRAME_CAPACITY: usize = 64;

    /// 诊断模式寄存器窗口大小
    pub DIAGNOSTIC_REGISTER_WINDOW: usize = 8;

    // ============================================================
    // GC 常量
    // ============================================================

    /// GC 最小阈值（字节）
    pub GC_MIN_THRESHOLD: usize = 1024;

    /// GC 默认阈值（10MB）
    pub GC_DEFAULT_THRESHOLD: usize = 10 * 1024 * 1024;

    /// GC 存活率阈值（低于此值则翻倍阈值）
    pub GC_SURVIVAL_RATIO_THRESHOLD: f64 = 0.5;

    /// GC 阈值增长倍数
    pub GC_THRESHOLD_GROWTH_FACTOR: usize = 2;

    // ============================================================
    // 编译器常量
    // ============================================================

    /// 编译器最大局部变量/寄存器数
    pub MAX_LOCALS: u16 = 65535;

    /// 函数内最大局部变量数（寄存器分配上限）
    ///
    /// 原值 255 对中等复杂度函数（大 switch、大量临时变量）容易触发 TooManyLocals。
    /// 提升到 4096 后：
    /// - 栈开销：4 个数组 × 4096 × sizeof ≈ 64KB（Windows 8MB 栈安全）
    /// - 覆盖 99.9% 的脚本语言场景
    /// - 如需更大，应将 LsraAllocator/Compiler 数组改为 Box 堆分配
    pub MAX_FUNCTION_LOCALS: u16 = 4096; // was 255, raised for LSRA integration

    /// 默认源文件名
    pub DEFAULT_SOURCE_FILE: &str = "<source>";

    /// 默认函数源文件名
    pub DEFAULT_FUNCTION_SOURCE_FILE: &str = "<function>";

    // ============================================================
    // 指令常量
    // ============================================================

    /// Capture 指令外部引用标记位（16位操作数）
    pub CAPTURE_OUTER_FLAG: u16 = 0x8000;

    /// Capture 指令外部索引掩码（16位操作数）
    pub CAPTURE_OUTER_INDEX_MASK: u16 = 0x7FFF;

    // ============================================================
    // 堆对象常量
    // ============================================================

    /// Array 开销字节数
    pub ARRAY_OVERHEAD_BYTES: usize = 24;

    /// Range 大小估算字节数
    pub RANGE_SIZE_BYTES: usize = 32;

    /// Closure 每个捕获变量字节数
    pub CAPTURED_VAR_SIZE_BYTES: usize = 16;

    /// Closure 基础开销字节数
    pub CLOSURE_OVERHEAD_BYTES: usize = 48;

    /// BuiltinFn 大小估算字节数
    pub BUILTIN_FN_SIZE_BYTES: usize = 32;

    /// Box 大小估算字节数
    pub BOX_SIZE_BYTES: usize = 24;

    /// Exception 异常对象大小估算字节数（基础开销，不含动态字段）
    pub EXCEPTION_SIZE_BYTES: usize = 128;

    /// StrBuilder (SliceChain) 大小估算字节数（基础开销，不含动态节点）
    pub STRBUILDER_SIZE_BYTES: usize = 48;

    // ============================================================
    // Arena (Region) 常量
    // ============================================================

    /// Arena 索引区起始值（位于 GC Persistent 和 Scratch 之间）
    /// 编码空间: [ARENA_BASE, SCRATCH_BASE) = [0x4000_0000, 0x8000_0000) = 1GB
    pub ARENA_BASE: u32 = 0x4000_0000;

    /// Arena 索引掩码（用于提取偏移量）
    pub ARENA_MASK: u32 = 0x3FFF_FFFF;

    // ============================================================
    // 内置函数常量
    // ============================================================

    /// 类型码：数字
    pub TYPE_CODE_NUMBER: f64 = 1.0;

    /// 类型码：布尔
    pub TYPE_CODE_BOOL: f64 = 2.0;

    /// 类型码：空值
    pub TYPE_CODE_NIL: f64 = 3.0;

    /// 类型码：对象
    pub TYPE_CODE_OBJECT: f64 = 6.0;

    /// 类型码：未知
    pub TYPE_CODE_UNKNOWN: f64 = 0.0;

    // ============================================================
    // 版本常量
    // ============================================================

    /// 应用版本号（与 workspace Cargo.toml 中的 version 保持同步）
    pub APP_VERSION: &str = "0.5.0";

    /// REPL 标题
    pub REPL_TITLE: &str = "Nuzo REPL";

    /// Runner 标题
    pub RUNNER_TITLE: &str = "Nuzo Runner";

    // ============================================================
    // 编码常量
    // ============================================================

    /// UTF-8 BOM 标记第1字节
    pub UTF8_BOM_0: u8 = 0xEF;

    /// UTF-8 BOM 标记第2字节
    pub UTF8_BOM_1: u8 = 0xBB;

    /// UTF-8 BOM 标记第3字节
    pub UTF8_BOM_2: u8 = 0xBF;

    /// UTF-16 LE BOM 第1字节
    pub UTF16_LE_BOM_0: u8 = 0xFF;

    /// UTF-16 LE BOM 第2字节
    pub UTF16_LE_BOM_1: u8 = 0xFE;

    /// UTF-16 BE BOM 第1字节
    pub UTF16_BE_BOM_0: u8 = 0xFE;

    /// UTF-16 BE BOM 第2字节
    pub UTF16_BE_BOM_1: u8 = 0xFF;
}
