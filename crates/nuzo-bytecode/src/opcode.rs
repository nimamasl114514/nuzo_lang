//! Nuzo Bytecode System — 单文件生产级字节码架构
//!
//! # 模块定位
//! 本模块是 Nuzo 虚拟机的**指令集定义与字节码容器**的核心实现。
//! 它承担了编译器后端到虚拟机执行引擎之间的桥梁作用：
//! - **编译器**通过 `Instruction` 枚举生成类型安全的中间表示 (IR)
//! - **编码器**将 `Instruction` 序列化为紧凑的二进制字节码流
//! - **虚拟机**解码并执行这些字节码指令
//!
//! # 架构设计原则
//! ## 单一数据源 (SSOT)
//! 由于 Rust 语言规范限制，过程宏必须独立成 crate。为了在单文件中实现
//! 最低维护成本和最高类型安全，本模块以 `Instruction` 枚举为**唯一数据源**，
//! 所有 Opcode 映射、编码、解码、反汇编逻辑均围绕此枚举展开。
//!
//! ## 强类型操作数 (Newtype Pattern)
//! 使用 Newtype 包装器（`Reg`, `ConstIdx`, `Offset` 等）实现**零成本抽象**，
//! 在编译期防止操作数混用（如将寄存器索引误用作常量池索引）。
//!
//! ## 内存高效存储
//! `Chunk` 使用 `Arc<Vec<u8>>` 共享字节码缓冲区，支持零拷贝克隆，
//! 适用于多函数共享同一原型对象的场景（闭包创建等）。
//!
//! # 主要组件
//! - **Newtypes**: `Reg`, `ConstIdx`, `Offset`, `CaptureIdx`, `U8`, `U16` — 类型安全的操作数
//! - **Instruction**: 高层指令枚举（44 种变体 + 3 个运行时特化 opcode = 47），面向编译器开发者
//! - **Opcode**: 底层操作码枚举（由 `define_opcodes!` 宏生成），面向虚拟机执行器
//! - **Chunk**: 字节码容器，管理指令序列、常量池、调试信息
//! - **CapturedSource**: 闭包捕获源的区分枚举（本地值 vs 外部引用）
//!
//! # 编码格式
//! 所有指令采用**小端序 (Little-Endian)** 编码：
//! - 操作码: 1 字节 (u8)
//! - 寄存器/常量索引: 2 字节 (u16 LE)
//! - 跳转偏移: 2 字节 (i16 LE)
//! - 参数计数: 1 字节 (u8)
//!
//! # 维护指南
//! 新增指令时必须同步更新以下位置（编译器会通过 `assert!` 和测试捕获遗漏）：
//! 1. `Instruction` 枚举变体
//! 2. `define_opcodes!` 宏中的 Opcode 定义（含 desc/summary 字段，自动同步 3 项）
//! 3. `Instruction::opcode()` 映射表
//! 4. `Instruction::encode()` 编码分支
//! 5. `Instruction::decode()` 解码分支
//! 6. `Chunk::disasm_instruction()` 反汇编格式化
//! 7. `constants.rs` 中的 `INSTRUCTION_COUNT`（编译期断言自动校验）
//!
//! description()、operand_summary()、iter_all() 已由宏自动生成，无需手动维护。

use nuzo_abi::index::SafeIndex;
use nuzo_core::SourceLocation;
use nuzo_core::Value;
use nuzo_core::{CAPTURE_OUTER_FLAG, CAPTURE_OUTER_INDEX_MASK, XxHashMap};
use nuzo_opcode::{DispatchKind, OperandKind};
use std::fmt;
use std::sync::Arc;

// ============================================================================
// 1. 强类型操作数 (Newtypes) — 零成本抽象，编译期类型安全
// ============================================================================

/// 虚拟机寄存器索引 (0..=65535)
///
/// # 设计意图
/// 使用 Newtype 包装 `u16` 以防止与常量池索引 (`ConstIdx`)、
/// 跳转偏移 (`Offset`) 等其他 u16 操作数混用。
///
/// # 使用场景
/// - 指令的目标寄存器 (dest)
/// - 源操作数寄存器 (src, left, right)
/// - 函数调用的基址寄存器 (func)
///
/// # 示例
/// ```ignore
/// let dest = Reg(0);   // 寄存器 r0
/// let src = Reg(1);    // 寄存器 r1
/// Instruction::Add { dest, left: src, right: Reg(2) };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Reg(pub u16);

/// 常量池索引 (0..=65535)
///
/// # 设计意图
/// 指向 `Chunk.constants` 数组中的位置，存储字面量值（数字、字符串等）。
/// 与 `Reg` 分离可避免将寄存器编号误用作常量索引。
///
/// # 使用场景
/// - `LoadK` 指令加载常量值
/// - `Closure` 指令引用函数原型
/// - `GetProp`/`SetProp` 引用属性名字符串
/// - `GetGlobal`/`SetGlobal` 引用全局变量名
///
/// # 边界约束
/// 常量池大小不得超过 `u16::MAX` (65535)，超出时 `add_constant()` 会 panic。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstIdx(pub u16);

/// 相对跳转偏移量 (-32768..=32767)
///
/// # 设计意图
/// 表示相对于**下一条指令**的字节偏移（非绝对地址）。
/// 使用 i16 支持前向和后向跳转。
///
/// # 计算公式
/// ```text
/// target_ip = current_ip + instruction_size + offset
/// ```
///
/// # 使用场景
/// - `Jmp`: 无条件跳转
/// - `Test`: 条件跳转（值为 falsy 时跳转）
///
/// # 示例
/// ```ignore
/// // 向前跳过 10 字节
/// Instruction::Jmp { offset: Offset(10) };
/// // 向后回退 5 字节（用于循环）
/// Instruction::Jmp { offset: Offset(-5) };
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Offset(pub i16);

/// 闭包捕获槽索引 (0..=65535)
///
/// # 设计意图
/// 标识闭包对象内部的捕获变量槽位，与普通寄存器 (`Reg`) 语义不同：
/// - 捕获槽位于堆分配的闭包对象上
/// - 通过 `GetCaptured`/`SetCaptured` 指令访问
/// - 生命周期独立于栈帧
///
/// # 使用场景
/// - `Capture` 指令指定捕获目标槽位
/// - `GetCaptured` 从闭包读取捕获变量
/// - `SetCaptured` 写入闭包的捕获变量
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CaptureIdx(pub u16);

/// 8 位无符号整数操作数 (0..=255)
///
/// # 使用场景
/// - `Call` 指令的参数个数 (`argc`)
/// - `RangeNew` 指令的包含标志 (`inclusive`: 0 或 1)
///
/// # 为什么不直接用 u8？
/// 保持与其他 Newtype 一致的类型安全策略，
/// 防止误将字节码原始数据当作语义化操作数使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct U8(pub u8);

/// 16 位无符号整数操作数 (0..=65535)
///
/// # 使用场景
/// - `ArrayNew` 指令的初始数组长度 (`count`)
///
/// # 为什么不直接用 u16？
/// 同 `U8`，保持类型系统一致性。
/// 未来可能扩展为带单位语义的类型（如 `Count`, `Size` 等）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct U16(pub u16);

/// 闭包捕获变量的来源类型
///
/// # 设计背景
/// Nuzo 支持函数闭包，内部函数可以捕获外部作用域的变量。
/// 捕获分为两种语义不同的方式：
///
/// ## 变体说明
/// - `ByValue(Reg)`: 捕获当前栈帧中的本地寄存器值（值拷贝）
/// - `Outer(u8)`: 引用外层闭包的捕获槽（跨层引用链）
///
/// # 编码细节（重要）
/// 在字节码中，`Capture` 指令的第三操作数使用位标志区分这两种情况：
/// - **ByValue**: 直接存储寄存器索引 (必须 < 0x8000)
/// - **Outer**: 设置最高位 `CAPTURE_OUTER_FLAG (0x8000)`，低 8 位为外层深度
///
/// 这种编码允许在单个 u16 操作数中区分两种语义，
/// 但要求寄存器索引不得使用高位（通过 assert! 保证）。
///
/// # 示例场景
/// ```ignore
/// // 场景1: 捕获本地变量 x (在寄存器 r0)
/// CapturedSource::ByValue(Reg(0))
///
/// // 场景2: 捕获外层第 3 层的变量
/// CapturedSource::Outer(3)
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedSource {
    /// 从当前栈帧的寄存器按值捕获（拷贝语义）
    ByValue(Reg),
    /// 从外层闭包的捕获槽引用（共享语义，参数为嵌套深度）
    Outer(u8),
}

// ============================================================================
// 2. 指令集定义 (Single Source of Truth) — 44 种高层指令 + 3 个运行时特化 opcode = 47
// ============================================================================
//
// # 架构定位
// `Instruction` 是面向**编译器开发者**的类型安全中间表示 (IR)。
// 每个变体携带语义化的字段名（如 `dest`, `left`, `right`），
// 而非裸操作数索引，从而在编译期防止参数顺序错误。
//
// # 与 Opcode 的关系
// - `Instruction` → 高层 IR，用于编译器代码生成
// - `Opcode` → 底层字节码，由 `define_opcodes!` 宏生成
// - 通过 `Instruction::opcode()` 方法双向映射
//
// # 指令分类
// 1. **常量与移动** (LoadK, LoadNil, LoadTrue, LoadFalse, Mov)
// 2. **算术运算** (Add, Sub, Mul, Div, Rem, Mod, Pow)
// 3. **一元运算** (Neg, Not)
// 4. **比较运算** (Eq, Neq, Lt, Gt, Le, Ge)
// 5. **控制流** (Jmp, Test)
// 6. **属性与索引** (GetProp, SetProp, GetIndex, SetIndex)
// 7. **函数操作** (Call, Return, Closure)
// 8. **内置/杂项** (Print, Halt, ArrayNew, Len)
// 9. **闭包捕获** (Capture, GetCaptured, SetCaptured)
// 10. **全局变量** (GetGlobal, SetGlobal)
// 11. **范围构造** (RangeNew)

/// Nuzo 虚拟机指令集 — 类型安全的中间表示 (IR)
///
/// # 设计哲学
/// 每个变体对应一种虚拟机操作，使用**命名字段**而非位置参数，
/// 最大化编译期错误检测能力（字段名错误会在编译时暴露）。
///
/// # 字段命名约定
/// - `dest`: 目标寄存器（写入结果）
/// - `src`: 源寄存器（单操作数）
/// - `left`, `right`: 双操作数的左/右操作数
/// - `base`, `exp`: 幂运算的底数/指数
/// - `obj`, `prop`: 对象属性访问的对象/属性名
/// - `index`, `val`: 索引访问的索引/值
/// - `func`, `argc`: 函数调用的基址寄存器/参数个数
/// - `offset`: 跳转偏移量
///
/// # 编码大小
/// 每条指令的编码字节数由对应的 `Opcode::instruction_size()` 决定，
/// 范围从 1 字节 (`Halt`) 到 8 字节 (`RangeNew`)。
#[derive(Clone, Debug, PartialEq, nuzo_proc::MatchSync, nuzo_proc::OpcodeSync)]
#[opcode_meta(extra_dispatch = [GetGlobalCached, SpillLoad, SpillStore])]
pub enum Instruction {
    // ── 常量与移动指令 ───────────────────────────────────────────────
    // 将值加载到寄存器或复制寄存器内容
    /// 加载常量: `dest = constants[const_idx]`
    ///
    /// 从常量池加载字面量值（数字、字符串、布尔值等）到指定寄存器。
    /// 这是最常用的指令之一，编译器将所有字面量都通过此指令加载。
    LoadK { dest: Reg, const_idx: ConstIdx },

    /// 加载 nil: `dest = nil`
    ///
    /// 将空值 nil 写入目标寄存器。用于变量初始化和默认值设置。
    LoadNil { dest: Reg },

    /// 加载 true: `dest = true`
    ///
    /// 将布尔值 true 写入目标寄存器。
    LoadTrue { dest: Reg },

    /// 加载 false: `dest = false`
    ///
    /// 将布尔值 false 写入目标寄存器。
    LoadFalse { dest: Reg },

    /// 寄存器移动: `dest = src`
    ///
    /// 将源寄存器的值拷贝到目标寄存器。用于参数传递、变量赋值等场景。
    Mov { dest: Reg, src: Reg },

    // ── 算术运算指令 ────────────────────────────────────────────────
    // 所有算术指令均为三地址码格式：dest = left op right
    /// 加法: `dest = left + right`
    Add { dest: Reg, left: Reg, right: Reg },

    /// 减法: `dest = left - right`
    Sub { dest: Reg, left: Reg, right: Reg },

    /// 乘法: `dest = left * right`
    Mul { dest: Reg, left: Reg, right: Reg },

    /// 除法: `dest = left / right`
    ///
    /// # 注意
    /// 除零行为由虚拟机的 Value 类型定义决定（通常返回 NaN 或 infinity）。
    Div { dest: Reg, left: Reg, right: Reg },

    /// 取余 (C 风格): `dest = left % right`
    ///
    /// 结果符号与被除数相同（截断除法），与 `Mod` 语义等价。
    /// **注意**：编译器仅生成 `Mod`，本变体保留用于 VM 测试和向后兼容。
    /// 新代码应优先使用 `Mod`。
    Rem { dest: Reg, left: Reg, right: Reg },

    /// 取模 (截断取余): `dest = left % right`
    ///
    /// 结果符号与被除数相同（C 风格截断除法），与 Rust 的 `%` 运算符一致。
    /// 例如: `-7 % 3 = -1`（而非 Euclidean 取模的 `2`）。
    /// 这是编译器实际生成的取余指令。
    Mod { dest: Reg, left: Reg, right: Reg },

    /// 幂运算: `dest = base ^ exp`
    Pow { dest: Reg, base: Reg, exp: Reg },

    /// 字符串批量构建: `dest = concat(R[start..start+count])`
    ///
    /// 编译期将连续 `+` 链收集为拼接树，一次性分配目标缓冲区后逐段拷入。
    /// 操作数按顺序存放在从 `start` 开始的连续寄存器中，共 `count` 个。
    StringBuild { dest: Reg, start: Reg, count: U16 },

    // ── 切片链字符串构建器 (SCSB) ────────────────────────────────────
    // 零拷贝循环内字符串拼接：编译器检测 `s = s + expr` 模式后生成
    /// 创建空切片链: `dest = SliceChain.new()`
    SliceChainNew { dest: Reg },

    /// 追加到切片链: `chain.append(src)`
    ///
    /// 将 src 的字符串表示追加到 chain 指向的 SliceChain 堆对象。
    /// O(1) 操作（仅增加引用计数）。
    SliceChainAppend { chain: Reg, src: Reg },

    /// 完成切片链: `dest = chain.finish()`
    ///
    /// 一次性分配目标缓冲区，逐段拷入所有节点，返回拼接结果字符串。
    /// O(N) 操作（N 为总字节数）。
    SliceChainFinish { dest: Reg, chain: Reg },

    // ── 一元运算指令 ────────────────────────────────────────────────
    /// 取负: `dest = -src`
    Neg { dest: Reg, src: Reg },

    /// 逻辑非: `dest = !src`
    ///
    /// 将值转换为布尔值后取反。falsy 值 (nil, false, 0, "") → true，
    /// 其他 truthy 值 → false。
    Not { dest: Reg, src: Reg },

    // ── 比较运算指令 ────────────────────────────────────────────────
    // 所有比较结果为布尔值 (true/false)，存入 dest
    /// 相等比较: `dest = (left == right)`
    Eq { dest: Reg, left: Reg, right: Reg },

    /// 不等比较: `dest = (left != right)`
    Neq { dest: Reg, left: Reg, right: Reg },

    /// 小于比较: `dest = (left < right)`
    Lt { dest: Reg, left: Reg, right: Reg },

    /// 大于比较: `dest = (left > right)`
    Gt { dest: Reg, left: Reg, right: Reg },

    /// 小于等于: `dest = (left <= right)`
    Le { dest: Reg, left: Reg, right: Reg },

    /// 大于等于: `dest = (left >= right)`
    Ge { dest: Reg, left: Reg, right: Reg },

    // ── 控制流指令 ──────────────────────────────────────────────────
    /// 无条件跳转: `IP += offset`
    ///
    /// offset 是相对于**下一条指令**的字节偏移。
    /// 正值向前跳转（用于 if/else、循环条件不满足时跳出），
    /// 负值向后跳转（用于循环回退）。
    Jmp { offset: Offset },

    /// 条件跳转: `if !reg then IP += else continue`
    ///
    /// 当 reg 的值为 falsy (nil, false, 0, "") 时跳转。
    /// 用于实现 if 语句、while/for 循环的条件判断。
    Test { reg: Reg, offset: Offset },

    // ── 属性与索引访问 ─────────────────────────────────────────────
    /// 获取属性: `dest = obj.property`
    ///
    /// prop 是常量池中的属性名字符串索引。
    /// 用于对象属性读取（如 `obj.x`, `obj["key"]`）。
    GetProp { dest: Reg, obj: Reg, prop: ConstIdx },

    /// 设置属性: `obj.property = val`
    SetProp { obj: Reg, prop: ConstIdx, val: Reg },

    /// 获取索引: `dest = obj[index]`
    ///
    /// 用于数组/列表的索引访问（如 `arr[0]`, `arr[i]`）。
    GetIndex { dest: Reg, obj: Reg, index: Reg },

    /// 设置索引: `obj[index] = val`
    SetIndex { obj: Reg, index: Reg, val: Reg },

    /// 设置索引（原地修改）: `obj[index] = val` — 零克隆
    SetIndexMut { obj: Reg, index: Reg, val: Reg },

    // ── 函数操作指令 ───────────────────────────────────────────────
    /// 函数调用: `call func(argc)`
    ///
    /// func 寄存器存储闭包对象，参数从 func+1 开始连续存放。
    /// 返回值存入 func 寄存器。
    Call { func: Reg, argc: U8 },

    /// 函数返回: `return val`
    ///
    /// 将 val 寄存器的值作为函数返回值。
    Return { val: Reg },

    /// 创建闭包: `dest = closure(proto)`
    ///
    /// proto 是常量池中的函数原型 (Prototype) 索引。
    /// 后续通过 `Capture` 指令填充捕获变量。
    Closure { dest: Reg, proto: ConstIdx },

    // ── 内置/杂项指令 ───────────────────────────────────────────────
    /// 打印值到标准输出: `print(reg)`
    Print { reg: Reg },

    /// 停止虚拟机执行
    ///
    /// 这是虚拟机正常终止的唯一方式（除错误退出外）。
    #[opcode_meta(skip_ssot)]
    Halt,

    /// 创建新数组: `dest = new Array(count)`
    ///
    /// 分配一个长度为 count 的空数组，元素初始值为 nil。
    ArrayNew { dest: Reg, count: U16 },

    /// 获取长度: `dest = len(src)`
    ///
    /// 支持字符串（字符数）、数组（元素数）等类型的长度查询。
    Len { dest: Reg, src: Reg },

    // ── 闭包捕获指令 ───────────────────────────────────────────────
    // 实现词法作用域闭包的变量捕获机制
    /// 捕获变量到闭包: `closure.captures[idx] = source`
    ///
    /// source 可以是：
    /// - 当前栈帧的寄存器 (`ByValue(Reg)`)
    /// - 外层闭包的捕获槽 (`Outer(depth)`)
    #[opcode_meta(skip_ssot)]
    Capture { closure: Reg, idx: CaptureIdx, source: CapturedSource },

    /// 从闭包读取捕获变量: `dest = closure.captures[idx]`
    GetCaptured { dest: Reg, idx: CaptureIdx },

    /// 写入闭包的捕获变量: `closure.captures[idx] = val`
    SetCaptured { idx: CaptureIdx, val: Reg },

    // ── 全局变量指令 ───────────────────────────────────────────────
    /// 读取全局变量: `dest = globals[name]`
    GetGlobal { dest: Reg, name: ConstIdx, _iss_gidx: U16 },

    /// 写入全局变量: `globals[name] = val`
    SetGlobal { val: Reg, name: ConstIdx },

    // ── 范围构造指令 ───────────────────────────────────────────────
    /// 创建范围对象: `dest = start..end` 或 `dest = start..=end`
    ///
    /// inclusive != 0 时表示包含结束值 (..=)，
    /// inclusive == 0 时表示不包含结束值 (..)。
    /// 用于 for-in 循环的范围迭代。
    RangeNew { dest: Reg, start: Reg, end: Reg, inclusive: U8 },

    // ── 异常处理指令 ───────────────────────────────────────────────
    // 实现 try/catch/out 异常控制流
    /// 标记 try 块开始: 将 (catch_ip, exception_reg) 压入异常栈
    ///
    /// catch_offset 是相对于**下一条指令**的字节偏移（指向 catch 块入口）。
    /// exception_reg 是存放异常值的目标寄存器编号（u8，0-255）。
    TryStart { catch_offset: Offset, exception_reg: U8 },

    /// 标记 try 块结束（正常路径）: 弹出异常栈顶
    ///
    /// try 块正常完成时执行，表示无需跳转到 catch。
    /// 必须与每个 TryStart 配对出现。
    TryEnd,

    /// 抛出异常 (out 语句): 从寄存器取值并跳转到 catch
    ///
    /// 查找最近的 TryStart，将 value_reg 的异常值存入对应的 exception_reg，
    /// 然后跳转到 catch_offset 指向的 catch 块入口。
    Out { value_reg: Reg },

    // ── 模块初始化指令 (lazy import) ────────────────────────────────
    /// 初始化模块 (lazy import): 触发模块首次加载
    ///
    /// `module_idx` 指向常量池中的模块路径字符串。
    /// `init_flag_slot` 是全局变量中"已初始化"标志位的槽位。
    ///
    /// 执行语义：
    /// 1. 检查 `globals[init_flag_slot]` 是否为真
    /// 2. 若已初始化 → 跳过加载（no-op）
    /// 3. 若未初始化 → 加载模块、执行其顶层代码、缓存导出
    /// 4. 将 `globals[init_flag_slot]` 置为真
    InitModule { module_idx: ConstIdx, init_flag_slot: U16 },
}

// ============================================================================
// 2.5 指令注册表 — 单一数据源驱动 opcode()/encode()/decode()
// ============================================================================
//
// `with_every_instruction!` 是 Instruction 枚举的 SSOT 宏。
// 新增指令时只需在此宏中添加一行声明，opcode()/encode()/decode() 三个
// 方法会由各自的消费者宏自动展开，无需手动维护 match 表。
//
// 每行格式：
//   ($Instr, $Opcode, { $fields })
//
// - $Instr:       Instruction 变体名
// - $Opcode:      对应的 Opcode 枚举值
// - { $fields }:  结构体字段定义（SSOT — 自动驱动 encode/decode 生成）
//
// # 宏卫生性设计
// encode/decode 的代码由消费者宏内部的嵌套重复直接生成。
// `chunk`/`pos` 作为消费者宏定义中的字面标识符，拥有正确的语法上下文，
// 可以解析到函数参数/局部变量。字段名 `$name` 来自 with_every_instruction!
// 的 token 流，在 match 模式和 match body 中保持同一语法上下文，因此能正确绑定。
// 这彻底解决了旧方案中 $ebody/$dbody 跨宏边界时标识符无法解析的问题。

// ── 编码辅助宏 ──────────────────────────────────────────────────────

/// 根据操作数类型生成对应的 Chunk 编码方法调用
///
/// 在消费者宏的嵌套重复 `$(gen_encode_field!($type, chunk, $name);)*` 中使用。
/// `chunk` 由消费者宏以字面标识符传入，拥有正确的语法上下文。
macro_rules! gen_encode_field {
    (Reg,        $chunk:ident, $val:ident) => {
        $chunk.write_u16($val.0)
    };
    (ConstIdx,   $chunk:ident, $val:ident) => {
        $chunk.write_u16($val.0)
    };
    (Offset,     $chunk:ident, $val:ident) => {
        $chunk.write_i16($val.0)
    };
    (U8,         $chunk:ident, $val:ident) => {
        $chunk.write_byte($val.0)
    };
    (U16,        $chunk:ident, $val:ident) => {
        $chunk.write_u16($val.0)
    };
    (CaptureIdx, $chunk:ident, $val:ident) => {
        $chunk.write_u16($val.0)
    };
}

// ── 解码辅助宏 ──────────────────────────────────────────────────────

/// 根据操作数类型生成对应的 Chunk 解码方法调用
///
/// 在消费者宏的嵌套重复 `let $name = gen_decode_field!($type, chunk, pos);` 中使用。
/// `chunk`/`pos` 由消费者宏以字面标识符传入，拥有正确的语法上下文。
macro_rules! gen_decode_field {
    (Reg,        $chunk:ident, $pos:ident) => {
        $chunk.decode_reg(&mut $pos)?
    };
    (ConstIdx,   $chunk:ident, $pos:ident) => {
        $chunk.decode_const(&mut $pos)?
    };
    (Offset,     $chunk:ident, $pos:ident) => {
        $chunk.decode_offset(&mut $pos)?
    };
    (U8,         $chunk:ident, $pos:ident) => {
        $chunk.decode_u8_val(&mut $pos)?
    };
    (U16,        $chunk:ident, $pos:ident) => {
        $chunk.decode_u16_val(&mut $pos)?
    };
    (CaptureIdx, $chunk:ident, $pos:ident) => {
        $chunk.decode_capture(&mut $pos)?
    };
}

// with_every_instruction! 宏由 #[derive(OpcodeSync)] 自动生成（见 Instruction 枚举上方）。
// Halt (unit variant) 和 Capture (位标志编码) 标注了 #[opcode_meta(skip_ssot)]，
// 不在 SSOT 中，各消费者宏单独处理。

// Capture 指令因 CapturedSource 的位标志编码，无法纳入通用宏模板，
// 需要在 encode/decode 中手动处理。Halt 是 unit variant，也需单独处理。

macro_rules! generate_opcode_method {
    ($(($instr:ident, $opcode:ident $(, $_name:ident : $_type:ident)*));* $(;)?) => {
        pub fn opcode(&self) -> Opcode {
            match self {
                Instruction::Halt => Opcode::Halt,
                $(Instruction::$instr { .. } => Opcode::$opcode,)*
                Instruction::Capture { .. } => Opcode::Capture,
            }
        }
    };
}

macro_rules! generate_encode_method {
    ($(($instr:ident, $opcode:ident $(, $name:ident : $type:ident)*));* $(;)?) => {
        pub fn encode(&self, chunk: &mut Chunk) {
            chunk.write_opcode(self.opcode());
            match self {
                Instruction::Halt => {},
                $(
                    Instruction::$instr { $($name),* } => {
                        $(gen_encode_field!($type, chunk, $name);)*
                    }
                )*
                Instruction::Capture { closure, idx, source } => {
                    chunk.write_u16(closure.0); chunk.write_u16(idx.0);
                    match source {
                        CapturedSource::ByValue(reg) => {
                            // A12 内部不变量: 本地捕获的寄存器索引必须低于
                            // CAPTURE_OUTER_FLAG (0x8000),否则在解码端会被
                            // 误判为 Outer 捕获(见 generate_decode_method)。
                            // 这是编码协议的硬性约束,由 Reg 类型构造器和
                            // 寄存器分配器(MAX_FUNCTION_LOCALS)共同保证,
                            // 违反即编译器内部 bug → 立即 panic 而非静默错误。
                            assert!(reg.0 < CAPTURE_OUTER_FLAG, "Register index must be < CAPTURE_OUTER_FLAG (0x8000) to avoid collision with Outer capture flag");
                            chunk.write_u16(reg.0);
                        },
                        CapturedSource::Outer(n) => chunk.write_u16(CAPTURE_OUTER_FLAG | (*n as u16)),
                    }
                }
            }
        }
    };
}

macro_rules! generate_decode_method {
    ($(($instr:ident, $opcode:ident $(, $name:ident : $type:ident)*));* $(;)?) => {
        pub fn decode(chunk: &Chunk, offset: usize) -> Option<(Instruction, usize)> {
            let byte = chunk.read_byte(offset)?;
            let op = Opcode::decode_opcode(byte)?;
            let mut pos = offset + 1;

            let instr = match op {
                Opcode::Halt => Instruction::Halt,
                $(
                    Opcode::$opcode => {
                        $(let $name = gen_decode_field!($type, chunk, pos);)*
                        Instruction::$instr { $($name),* }
                    }
                )*
                Opcode::Capture => {
                    let closure = chunk.decode_reg(&mut pos)?;
                    let idx = chunk.decode_capture(&mut pos)?;
                    let raw = chunk.read_u16(pos)?;
                    pos += 2;
                    let source = if raw & CAPTURE_OUTER_FLAG != 0 {
                        let outer_idx = raw & CAPTURE_OUTER_INDEX_MASK;
                        // SafeIndex 显式窄化：outer_idx 已被 CAPTURE_OUTER_INDEX_MASK
                        // 截断至低 8 位，但用 try_from_u32 使意图更清晰且防御未来变更
                        let narrow = SafeIndex::<u8>::try_from_u32(outer_idx as u32);
                        let outer_u8 = match narrow {
                            Ok(idx) => idx.get(),
                            Err(_) => return None,
                        };
                        CapturedSource::Outer(outer_u8)
                    } else {
                        if raw >= CAPTURE_OUTER_FLAG { return None; }
                        CapturedSource::ByValue(Reg(raw))
                    };
                    Instruction::Capture { closure, idx, source }
                }
                // ISS 运行时自修补指令，无对应 Instruction 变体，不可解码
                Opcode::GetGlobalCached => return None,
                // LSRA Spill 指令，无对应 Instruction 变体（由编译器后端直接发射 Opcode）
                Opcode::SpillLoad => return None,
                Opcode::SpillStore => return None,
            };
            Some((instr, pos))
        }
    };
}

// ============================================================================
// 3. Opcode 枚举与向后兼容 API
// ============================================================================

nuzo_proc::define_opcodes! {
    // ── 加载指令 ──────────────────────────────────────────────────────

    /// 将常量池中的值加载到寄存器
    #[opcode(code = 0, size = 5, operands = [Reg, Const], disasm = custom, dispatch = LoadFromPool, desc = "将常量池中的值加载到寄存器", summary = "dest (Reg), constant_index (ConstIdx)")]
    LoadK,

    /// 将 nil 值加载到寄存器
    #[opcode(code = 1, size = 3, operands = [Reg], disasm = custom, dispatch = LoadConst, desc = "将 nil 值加载到寄存器", summary = "dest (Reg)")]
    LoadNil,

    /// 将 true 值加载到寄存器
    #[opcode(code = 2, size = 3, operands = [Reg], disasm = custom, dispatch = LoadConst, desc = "将 true 值加载到寄存器", summary = "dest (Reg)")]
    LoadTrue,

    /// 将 false 值加载到寄存器
    #[opcode(code = 3, size = 3, operands = [Reg], disasm = custom, dispatch = LoadConst, desc = "将 false 值加载到寄存器", summary = "dest (Reg)")]
    LoadFalse,

    /// 将值从一个寄存器移动到另一个寄存器
    #[opcode(code = 4, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = MovRegister, desc = "将值从一个寄存器移动到另一个寄存器", summary = "dest (Reg), src (Reg)")]
    Mov,

    // ── 算术指令 ──────────────────────────────────────────────────────

    /// 加法: dest = left + right
    #[opcode(code = 5, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "加法: dest = left + right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Add,

    /// 减法: dest = left - right
    #[opcode(code = 6, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "减法: dest = left - right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Sub,

    /// 乘法: dest = left * right
    #[opcode(code = 7, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "乘法: dest = left * right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Mul,

    /// 除法: dest = left / right
    #[opcode(code = 8, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "除法: dest = left / right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Div,

    /// 取余: dest = left % right（与 Mod 等价，编译器生成 Mod；保留用于 VM 测试兼容）
    #[opcode(code = 9, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "取余 (同Mod): dest = left % right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Rem,

    /// 取负: dest = -src
    #[opcode(code = 10, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = UnaryOp, desc = "取负: dest = -src", summary = "dest (Reg), src (Reg)")]
    Neg,

    // ── 比较指令 ──────────────────────────────────────────────────────

    /// 相等比较: dest = (left == right)
    #[opcode(code = 11, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = EqualityComparison, desc = "相等比较: dest = (left == right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Eq,

    /// 不等比较: dest = (left != right)
    #[opcode(code = 12, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = EqualityComparison, desc = "不等比较: dest = (left != right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Neq,

    /// 小于比较: dest = (left < right)
    #[opcode(code = 13, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryComparison, desc = "小于比较: dest = (left < right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Lt,

    /// 大于比较: dest = (left > right)
    #[opcode(code = 14, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryComparison, desc = "大于比较: dest = (left > right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Gt,

    /// 小于等于: dest = (left <= right)
    #[opcode(code = 15, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryComparison, desc = "小于等于: dest = (left <= right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Le,

    /// 大于等于: dest = (left >= right)
    #[opcode(code = 16, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryComparison, desc = "大于等于: dest = (left >= right)", summary = "dest (Reg), left (Reg), right (Reg)")]
    Ge,

    /// 逻辑非: dest = !src
    #[opcode(code = 17, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = LogicalNot, desc = "逻辑非: dest = !src", summary = "dest (Reg), src (Reg)")]
    Not,

    // ── 控制流 ────────────────────────────────────────────────────────

    /// 无条件跳转
    #[opcode(code = 18, size = 3, operands = [Offset], disasm = custom, dispatch = Custom, desc = "无条件跳转", summary = "offset (Offset)")]
    Jmp,

    /// 条件跳转: 如果寄存器值为假则跳转
    #[opcode(code = 19, size = 5, operands = [Reg, Offset], disasm = custom, dispatch = Custom, desc = "条件跳转: 如果寄存器值为假则跳转", summary = "reg (Reg), offset (Offset)")]
    Test,

    // ── 属性访问 ──────────────────────────────────────────────────────

    /// 获取属性: dest = obj.property
    #[opcode(code = 20, size = 7, operands = [Reg, Reg, Const], disasm = custom, dispatch = Custom, desc = "获取属性: dest = obj.property", summary = "dest (Reg), obj (Reg), prop (ConstIdx)")]
    GetProp,

    /// 设置属性: obj.property = value
    #[opcode(code = 21, size = 7, operands = [Reg, Const, Reg], disasm = custom, dispatch = Custom, desc = "设置属性: obj.property = value", summary = "obj (Reg), prop (ConstIdx), val (Reg)")]
    SetProp,

    /// 获取索引: dest = obj[index]
    #[opcode(code = 22, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = Custom, desc = "获取索引: dest = obj[index]", summary = "dest (Reg), left (Reg), right (Reg)")]
    GetIndex,

    /// 设置索引: obj[index] = value
    #[opcode(code = 23, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = Custom, desc = "设置索引: obj[index] = value", summary = "obj (Reg), index (Reg), val (Reg)")]
    SetIndex,

    /// 设置索引（原地修改）: obj[index] = value — 零克隆，编译器保证对象独占
    #[opcode(code = 41, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = Custom, desc = "设置索引（原地修改）: obj[index] = value", summary = "obj (Reg), index (Reg), val (Reg)")]
    SetIndexMut,

    // ── 函数调用 ──────────────────────────────────────────────────────

    /// 调用函数
    #[opcode(code = 24, size = 4, operands = [Reg, U8], disasm = custom, dispatch = Custom, desc = "调用函数", summary = "func_reg (Reg), argc (U8)")]
    Call,

    /// 函数返回
    #[opcode(code = 25, size = 3, operands = [Reg], disasm = custom, dispatch = Custom, desc = "函数返回", summary = "dest (Reg)")]
    Return,

    /// 创建闭包
    #[opcode(code = 26, size = 5, operands = [Reg, Const], disasm = custom, dispatch = LoadClosure, desc = "创建闭包", summary = "dest (Reg), constant_index (ConstIdx)")]
    Closure,

    /// 打印寄存器值
    #[opcode(code = 27, size = 3, operands = [Reg], disasm = custom, dispatch = PrintValue, desc = "打印寄存器值", summary = "dest (Reg)")]
    Print,

    // ── 特殊 ──────────────────────────────────────────────────────────

    /// 停止虚拟机
    #[opcode(code = 28, size = 1, operands = [], disasm = "halt", dispatch = Custom, desc = "停止虚拟机", summary = "")]
    Halt,

    /// 创建新数组
    #[opcode(code = 29, size = 5, operands = [Reg, U16], disasm = custom, dispatch = Custom, desc = "创建新数组", summary = "dest (Reg), count (U16)")]
    ArrayNew,

    /// 初始化模块 (lazy import)
    ///
    /// 触发模块首次加载：检查 init_flag_slot 标志位，未初始化时加载模块并置位。
    ///
    /// 编码格式: opcode(1) + module_idx:u16(2) + init_flag_slot:u16(2) = 5 字节
    #[opcode(code = 30, size = 5, operands = [Const, U16], disasm = custom, dispatch = Custom, desc = "初始化模块(lazy import)", summary = "module_idx (ConstIdx), init_flag_slot (U16)")]
    InitModule,

    // ── 闭包捕获 ──────────────────────────────────────────────────────

    /// 注意: 第三操作数是 source (Reg)，被编码器使用 CAPTURE_OUTER_FLAG 位区分本地/外部捕获
    #[opcode(code = 31, size = 7, operands = [Reg, CaptureIdx, Reg], disasm = custom, dispatch = Custom, desc = "捕获变量到闭包", summary = "closure (Reg), capture_idx (CaptureIdx), source (Reg/u16)")]
    Capture,

    /// 从闭包获取捕获的变量
    #[opcode(code = 32, size = 5, operands = [Reg, CaptureIdx], disasm = custom, dispatch = Custom, desc = "从闭包获取捕获的变量", summary = "dest (Reg), capture_idx (CaptureIdx)")]
    GetCaptured,

    /// 设置闭包中的捕获变量
    #[opcode(code = 33, size = 5, operands = [CaptureIdx, Reg], disasm = custom, dispatch = Custom, desc = "设置闭包中的捕获变量", summary = "capture_idx (CaptureIdx), val (Reg)")]
    SetCaptured,

    // slot 34 reserved (opcode 34 保留)

    // ── 全局变量 ──────────────────────────────────────────────────────

    /// 获取全局变量（含 ISS 预留缓存空间）
    ///
    /// 原始格式：opcode(1) + dest:u16(2) + name_idx:u16(2) + _iss_gidx:u16(2) = 7 字节
    ///
    /// 首次执行时，handler 只读 dest + name_idx，跳过后 2 字节 padding。
    /// resolve 成功后，将本指令 patch 为 `GetGlobalCached`：
    /// - Byte 0: opcode → GetGlobalCached
    /// - Bytes 3-4: name_idx(u16) → gidx(u16)
    /// - Bytes 5-6: _iss_gidx(u16) → version(u16)
    #[opcode(code = 35, size = 7, operands = [Reg, Const, U16], disasm = custom, dispatch = Custom, desc = "获取全局变量(ISS预留缓存空间)", summary = "dest (Reg), name_idx (ConstIdx), _iss_gidx (U16)")]
    GetGlobal,

    /// 设置全局变量
    #[opcode(code = 36, size = 5, operands = [Reg, Const], disasm = custom, dispatch = Custom, desc = "设置全局变量", summary = "dest (Reg), constant_index (ConstIdx)")]
    SetGlobal,

    // ── 范围/集合 ─────────────────────────────────────────────────────

    /// 创建范围对象: dest = start..end (含 inclusive 标志)
    #[opcode(code = 37, size = 8, operands = [Reg, Reg, Reg, U8], disasm = custom, dispatch = Custom, desc = "创建范围对象: dest = start..end (含 inclusive 标志)", summary = "dest (Reg), start (Reg), end (Reg), inclusive (U8)")]
    RangeNew,

    /// 取模: dest = left % right
    #[opcode(code = 38, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "取模: dest = left % right", summary = "dest (Reg), left (Reg), right (Reg)")]
    Mod,

    /// 获取长度: dest = len(obj)
    #[opcode(code = 39, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = Custom, desc = "获取长度: dest = len(obj)", summary = "dest (Reg), src (Reg)")]
    Len,

    /// 幂运算: dest = base ^ exp
    #[opcode(code = 40, size = 7, operands = [Reg, Reg, Reg], disasm = custom, dispatch = BinaryArithmetic, desc = "幂运算: dest = base ^ exp", summary = "dest (Reg), left (Reg), right (Reg)")]
    Pow,

    /// 字符串批量构建: dest = concat(R[start..start+count])
    ///
    /// 将连续寄存器范围中的字符串值拼接为单个字符串。
    /// 编译期将连续 `+` 链收集为拼接树，一次性分配目标缓冲区后逐段拷入，
    /// 避免多次中间分配和拷贝（零分配路径）。
    #[opcode(code = 42, size = 7, operands = [Reg, Reg, U16], disasm = custom, dispatch = Custom, desc = "字符串批量构建: dest = concat(R[start..start+count])", summary = "dest (Reg), start (Reg), count (U16)")]
    StringBuild,

    // ── ISS 特化指令 ──────────────────────────────────────────────────

    /// ISS: 特化全局变量读取（内联缓存）
    ///
    /// 通用 `GetGlobal` 在首次执行后，将自身修补为此特化指令。
    /// 所有缓存数据直接嵌入指令操作数，无需访问外部缓存表。
    ///
    /// 执行逻辑：
    /// 1. 从指令流读取 `gidx` 和 `expected_ver`（均为 u16）
    /// 2. 比较 `global_versions[gidx] == expected_ver`
    /// 3. 匹配 → 直接 `get_global(gidx)`，零表查找
    /// 4. 不匹配 → 重新读值 + 更新指令中的版本号
    #[opcode(code = 50, size = 7, operands = [Reg, U16, U16], disasm = custom, dispatch = GetGlobalCached, desc = "ISS特化全局变量读取(内联缓存)", summary = "dest (Reg), global_idx (U16), version (U16)")]
    GetGlobalCached,

    // ── 异常处理指令 ──────────────────────────────────────────────────

    /// 标记 try 块开始，记录 catch 跳转目标和异常寄存器
    ///
    /// 执行时将 (catch_ip, exception_reg) 压入异常栈。
    /// 当后续 Out 指令触发时，VM 弹出异常栈顶并跳转到 catch 入口。
    ///
    /// 编码格式: opcode(1) + catch_offset:i16(2) + exception_reg:u8(1) = 4 字节
    #[opcode(code = 51, size = 4, operands = [Offset, U8], disasm = custom, dispatch = Custom, desc = "标记try块开始，记录catch跳转目标", summary = "catch_offset (Offset), exception_reg (U8)")]
    TryStart,

    /// 标记 try 块结束（正常路径）
    ///
    /// try 块正常完成时弹出异常栈顶（无需跳转到 catch）。
    /// 与 Out 指令配合：Out 触发跳转，TryEnd 正常清理。
    ///
    /// 编码格式: 仅 opcode(1) = 1 字节
    #[opcode(code = 52, size = 1, operands = [], disasm = custom, dispatch = Custom, desc = "标记try块结束（正常路径）", summary = "")]
    TryEnd,

    /// 抛出异常（out 语句）
    ///
    /// 从指定寄存器取异常值，查找最近的 TryStart 对应的 catch 入口，
    /// 将异常值存入 exception_reg 后跳转到 catch 块执行。
    ///
    /// 编码格式: opcode(1) + value_reg:u16(2) = 3 字节
    #[opcode(code = 53, size = 3, operands = [Reg], disasm = custom, dispatch = Custom, desc = "抛出异常(out语句)", summary = "value_reg (Reg)")]
    Out,

    // ── LSRA Spill 指令 ──────────────────────────────────────────────

    /// LSRA Spill 加载: 从 VM 的 spill_stack[slot] 加载值到寄存器 R[dst]
    ///
    /// 用于线性扫描寄存器分配（LSRA）的 spill 机制：
    /// 当寄存器压力过大时，编译器将某些寄存器值临时存储到 VM 的 spill_stack，
    /// 需要时再通过 SpillLoad 恢复到寄存器。
    ///
    /// 编码格式: opcode(1) + dst:u16(2) + slot:u16(2) = 5 字节
    #[opcode(code = 54, size = 5, operands = [Reg, U16], disasm = custom, dispatch = Custom, desc = "LSRA Spill 加载: 从 spill_stack[slot] 加载到 R[dst]", summary = "dst (Reg), slot (U16)")]
    SpillLoad,

    /// LSRA Spill 存储: 从寄存器 R[src] 存值到 VM 的 spill_stack[slot]
    ///
    /// 与 SpillLoad 配对使用，实现寄存器溢出/恢复。
    /// 编译器在寄存器分配失败时发射此指令将值溢出到内存。
    ///
    /// 编码格式: opcode(1) + src:u16(2) + slot:u16(2) = 5 字节
    #[opcode(code = 55, size = 5, operands = [Reg, U16], disasm = custom, dispatch = Custom, desc = "LSRA Spill 存储: 从 R[src] 存储到 spill_stack[slot]", summary = "src (Reg), slot (U16)")]
    SpillStore,

    // ── SCSB 切片链字符串构建器 ───────────────────────────────────────

    /// 创建空切片链: dest = SliceChain.new()
    #[opcode(code = 56, size = 3, operands = [Reg], disasm = custom, dispatch = Custom, desc = "创建空切片链", summary = "dest (Reg)")]
    SliceChainNew,

    /// 追加到切片链: chain.append(src)
    #[opcode(code = 57, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = Custom, desc = "追加到切片链", summary = "chain (Reg), src (Reg)")]
    SliceChainAppend,

    /// 完成切片链: dest = chain.finish()
    #[opcode(code = 58, size = 5, operands = [Reg, Reg], disasm = custom, dispatch = Custom, desc = "完成切片链", summary = "dest (Reg), chain (Reg)")]
    SliceChainFinish,
}

// ── 编译期校验：INSTRUCTION_COUNT 与 Opcode 变体数量一致 ──────────
// INSTRUCTION_COUNT 由 #[derive(OpcodeSync)] 自动生成（基于 Instruction 变体数 + extra_dispatch）。
// 此断言确保 derive 宏生成的计数与 define_opcodes! 生成的 Opcode::ALL.len() 一致。
// 若不一致，说明 Instruction 枚举与 define_opcodes! 宏调用不同步。
const _OPCODE_COUNT_CHECK: () = {
    assert!(
        crate::INSTRUCTION_COUNT == Opcode::ALL.len(),
        "INSTRUCTION_COUNT 与 Opcode::ALL 数量不一致！\
         请检查 Instruction 枚举（含 #[opcode_meta(extra_dispatch = ...)]）是否与 define_opcodes! 同步。"
    );
};

// OperandKind 和 DispatchKind 由 nuzo_opcode crate 统一定义，此处通过 re-export 提供

impl Instruction {
    with_every_instruction!(generate_opcode_method);
    with_every_instruction!(generate_encode_method);
    with_every_instruction!(generate_decode_method);

    /// 返回指令在字节码中的字节数
    #[inline]
    #[must_use]
    pub fn size(&self) -> usize {
        self.opcode().instruction_size()
    }
}

// description()、operand_summary()、iter_all() 已纳入 define_opcodes! 宏自动生成
// 新增 Opcode 时只需在宏调用中添加 desc/summary 字段即可，无需手动维护 match 表。

// ============================================================================
// 4. Chunk (字节码容器) — 管理指令序列、常量池、调试信息
// ============================================================================
//
// # 设计目标
// Chunk 是编译单元的输出载体，对应一个函数体或脚本顶层代码块。
// 它存储了虚拟机执行所需的全部信息：
//
// ## 核心数据
// - `code`: 字节码指令流 (u8 数组)
// - `constants`: 常量池 (Value 数组，存储字面量)
// - `lines`: 行号表 (用于源码映射)
// - `debug_info`: 调试信息 (文件名、源码行、IP→行号映射)
//
// ## 内存管理
// 使用 `Arc<T>` 包装所有内部缓冲区，实现：
// - **零拷贝克隆**: 多个闭包共享同一原型对象的字节码
// - **写时复制 (COW)**: 修改时通过 `Arc::make_mut` 自动克隆
// - **线程安全读**: 可跨线程共享只读视图（未来可扩展）
//
// # 使用模式
// ```ignore
// let mut chunk = Chunk::new();
// let idx = chunk.add_constant(Value::from_number(42.0));
// chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(idx as u16) });
// chunk.emit(Instruction::Halt);
// println!("{}", chunk.disassemble()); // 打印反汇编结果
// ```

/// 反汇编输出中源码行前缀的字符宽度: `"{:3} | "` → 3 位行号 + ` | ` (3 字符) = 6
const SOURCE_LINE_PREFIX_WIDTH: usize = 6;

/// 字节码块 — 编译单元的完整表示
///
/// # 生命周期
/// 典型生命周期流程：
/// 1. **创建**: `Chunk::new()` — 编译器开始编译新函数/脚本
/// 2. **填充**: `emit()` / `add_constant()` — 编译器生成字节码
/// 3. **使用**: 虚拟机通过 `code` 和 `constants` 执行指令
/// 4. **调试**: `disassemble()` 生成人类可读的反汇编输出
///
/// # 性能特征
/// - **空间效率**: 使用紧凑的变长编码（1-8 字节/指令）
/// - **时间复杂度**:
///   - `emit()`: O(1) 平摊 (Amortized)
///   - `add_constant()`: O(1) 平摊
///   - `disassemble()`: O(n) 其中 n 为字节数
///   - `decode()`: O(k) 其中 k 为指令长度
///
/// 字节码 Chunk 操作错误。
///
/// 当 Chunk 的内部不变量被违反时返回,例如常量池索引超出 u16 范围。
/// 这是底层字节码容器的错误类型,独立于上层 `CompileError`/`CodegenError`,
/// 因为 `nuzo_bytecode` (L2) 不能依赖 `nuzo_compiler` (L4)。
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkError {
    /// 常量池索引超出 u16 范围。
    ///
    /// Chunk 的常量池使用 u16 索引(`ConstIdx`),最大支持 65535 个常量。
    /// 当程序包含过多字面量(字符串、数字等)时会触发。
    ConstantPoolOverflow {
        /// 触发溢出时的常量池大小
        count: usize,
    },
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConstantPoolOverflow { count } => {
                write!(f, "constant pool overflow: {} entries (max {})", count, u16::MAX)
            }
        }
    }
}

impl std::error::Error for ChunkError {}

#[derive(Debug, Clone)]
pub struct Chunk {
    /// 字节码指令序列 (只读共享，修改时 COW)
    code: Arc<Vec<u8>>,

    /// 常量池 (存储数字、字符串、布尔值等字面量)
    ///
    /// 索引类型为 `ConstIdx` (u16)，最大支持 65536 个常量。
    /// 通过 `LoadK` 指令按索引加载到寄存器。
    constants: Arc<Vec<Value>>,

    /// 常量池去重索引 (Value → 常量池索引)
    ///
    /// 提供 O(1) 查找已存在的常量值，避免重复添加相同的字面量。
    /// 例如 `3 + 5` 的结果 `8` 和代码中直接写的 `8` 共享同一个常量池槽位。
    /// 与 `constants` 共享 COW 语义：修改时通过 `Arc::make_mut` 自动深拷贝。
    constant_index: Arc<XxHashMap<Value, usize>>,

    /// 行号表 (用于基本调试信息)
    ///
    /// 已逐步被 `debug_info.ip_to_line` 取代，
    /// 保留用于向后兼容。
    lines: Arc<Vec<u32>>,

    /// 详细调试信息 (源文件名、源码行、IP→行号映射)
    pub debug_info: Arc<DebugInfo>,

    /// 本地变量计数 (由编译器设置，用于栈帧分配)
    pub locals_count: u16,

    /// LSRA spill 机制所需的栈槽数量 (0 表示无 spill，旧字节码兼容)
    pub spill_slot_count: u16,
}

pub use nuzo_values::DeadCodeReason;
pub use nuzo_values::DeadCodeRecord;
/// 从 nuzo_values crate 重导出调试信息类型
///
/// 保持 API 一致性，避免用户直接依赖 nuzo_values。
pub use nuzo_values::DebugInfo;
pub use nuzo_values::InlineRecord;

impl Chunk {
    /// 创建空的字节码块
    ///
    /// # 初始状态
    /// - `code`: 空的 Vec<u8>
    /// - `constants`: 空的 Vec<Value>
    /// - `lines`: 空的 Vec<u32>
    /// - `debug_info`: 空 DebugInfo（无源文件信息）
    /// - `locals_count`: 0
    pub fn new() -> Self {
        Chunk {
            code: Arc::new(Vec::new()),
            constants: Arc::new(Vec::new()),
            constant_index: Arc::new(XxHashMap::with_hasher(
                std::hash::BuildHasherDefault::default(),
            )),
            lines: Arc::new(Vec::new()),
            locals_count: 0,
            spill_slot_count: 0,
            debug_info: Arc::new(DebugInfo::default()),
        }
    }

    // ========================================================================
    // 只读访问器
    // ========================================================================

    /// 返回字节码指令序列的切片引用
    #[inline]
    #[must_use]
    pub fn code(&self) -> &[u8] {
        &self.code
    }

    /// 返回常量池的切片引用
    #[inline]
    #[must_use]
    pub fn constants(&self) -> &[Value] {
        &self.constants
    }

    /// 返回行号表的切片引用
    #[inline]
    #[must_use]
    pub fn lines(&self) -> &[u32] {
        &self.lines
    }

    // ========================================================================
    // COW 写入访问器
    // ========================================================================

    /// 返回字节码的可变引用（COW 语义：首次修改时自动深拷贝）
    ///
    /// 等价于 `Arc::make_mut(&mut self.code)`，但封装了 COW 细节。
    #[inline]
    pub fn code_mut(&mut self) -> &mut Vec<u8> {
        Arc::make_mut(&mut self.code)
    }

    /// 返回常量池的可变引用（COW 语义：首次修改时自动深拷贝）
    #[inline]
    pub fn constants_mut(&mut self) -> &mut Vec<Value> {
        Arc::make_mut(&mut self.constants)
    }

    /// 返回行号表的可变引用（COW 语义：首次修改时自动深拷贝）
    #[inline]
    pub fn lines_mut(&mut self) -> &mut Vec<u32> {
        Arc::make_mut(&mut self.lines)
    }

    // ========================================================================
    // 构造与消费方法
    // ========================================================================

    /// 从 Arc 组件构造 Chunk（用于 VM 从 FunctionPrototype 创建 Chunk）
    ///
    /// 注意：此方法从 constants 重建 constant_index，确保去重索引与常量池同步。
    pub fn from_arcs(
        code: Arc<Vec<u8>>,
        constants: Arc<Vec<Value>>,
        lines: Arc<Vec<u32>>,
        debug_info: Arc<DebugInfo>,
        locals_count: u16,
        spill_slot_count: u16,
    ) -> Self {
        // 从常量池重建去重索引（只保留首次出现的索引，避免重复值覆盖）
        let constant_index: XxHashMap<Value, usize> =
            constants.iter().enumerate().fold(XxHashMap::default(), |mut map, (idx, val)| {
                map.entry(*val).or_insert(idx);
                map
            });
        Chunk {
            code,
            constants,
            constant_index: Arc::new(constant_index),
            lines,
            debug_info,
            locals_count,
            spill_slot_count,
        }
    }

    /// 消费 Chunk，返回内部 Arc 组件（用于从 Chunk 构建 FunctionPrototype）
    #[allow(clippy::type_complexity)] // Chunk 解构返回 5 个 Arc 组件，拆分会增加 API 复杂度
    pub fn into_parts(
        self,
    ) -> (Arc<Vec<u8>>, Arc<Vec<Value>>, Arc<Vec<u32>>, Arc<DebugInfo>, u16, u16) {
        (
            self.code,
            self.constants,
            self.lines,
            self.debug_info,
            self.locals_count,
            self.spill_slot_count,
        )
    }

    /// 写入操作码字节 (1 字节)
    pub fn write_opcode(&mut self, op: Opcode) {
        Arc::make_mut(&mut self.code).push(op as u8);
    }

    /// 写入单个字节 (1 字节) — 用于 u8 操作数 (argc, inclusive 等)
    pub fn write_byte(&mut self, b: u8) {
        Arc::make_mut(&mut self.code).push(b);
    }

    /// 写入 16 位无符号整数 (2 字节，小端序)
    ///
    /// 编码格式：低字节在前，高字节在后。
    /// 用于寄存器索引、常量池索引等 u16 操作数。
    pub fn write_u16(&mut self, val: u16) {
        let code = Arc::make_mut(&mut self.code);
        code.push((val & 0xFF) as u8);
        code.push((val >> 8) as u8);
    }

    /// 写入 16 位有符号整数 (2 字节，小端序) — 用于跳转偏移量
    pub fn write_i16(&mut self, val: i16) {
        self.write_u16(val as u16);
    }

    /// 发射指令到字节码块（便捷方法）
    ///
    /// 自动调用 `Instruction::encode(self)` 完成编码。
    /// 这是编译器生成字节码的主要入口点。
    pub fn emit(&mut self, instr: Instruction) {
        instr.encode(self);
    }

    /// 添加常量到常量池并返回索引（fallible 版本，推荐使用）
    ///
    /// # 去重优化
    /// 如果常量池中已存在相同值（通过 `Value` 的 `Hash + Eq` 判定），
    /// 直接返回已有索引，避免重复存储。这对于常量折叠特别重要：
    /// `3 + 5` 折叠为 `8` 后，如果代码中已有字面量 `8`，两者共享同一槽位。
    ///
    /// # 边界约束
    /// 常量池大小不得超过 `u16::MAX` (65535)。超出时返回
    /// [`ChunkError::ConstantPoolOverflow`],调用方可优雅处理(例如映射到
    /// `CompileError::ConstantPoolOverflow`)。
    ///
    /// # 返回值
    /// `Ok(usize)` — 新常量的索引位置,可安全转换为 `ConstIdx`
    /// `Err(ChunkError::ConstantPoolOverflow { count })` — 常量池已满
    ///
    /// # Dual API 设计
    /// - **fallible**: `try_add_constant` (本方法) — 返回 `Result`,适合编译器/代码生成器
    /// - **infallible**: `add_constant` — 内部调用本方法并 `expect` 兜底,
    ///   适合测试或确定不会溢出的场景
    pub fn try_add_constant(&mut self, value: Value) -> Result<usize, ChunkError> {
        let constants = Arc::make_mut(&mut self.constants);
        let index = Arc::make_mut(&mut self.constant_index);

        // 去重：检查是否已存在相同值
        if let Some(&idx) = index.get(&value) {
            return Ok(idx);
        }

        let idx = constants.len();
        // C1 修复: 用 Result 替代 assert,从根源消除 panic
        if idx > u16::MAX as usize {
            return Err(ChunkError::ConstantPoolOverflow { count: idx });
        }
        constants.push(value);
        index.insert(value, idx);
        Ok(idx)
    }

    /// 添加常量到常量池并返回索引（infallible 便捷版本）
    ///
    /// 内部委托给 [`try_add_constant`],常量池溢出时 panic。
    /// **生产代码(编译器/代码生成器)应优先使用 `try_add_constant`**,
    /// 本方法保留用于测试或调用方确定不会溢出的场景。
    ///
    /// # Panic
    /// 常量池大小超过 `u16::MAX` (65535) 时 panic。消息包含溢出时的常量池大小,
    /// 并提示调用方改用 `try_add_constant` 以便将错误转化为 `CompileError`，
    /// 保留源码位置信息（遵守项目约束"编译错误必须保留源码位置，不能降级"）。
    ///
    /// # 返回值
    /// 新常量的索引位置 (usize)，可安全转换为 `ConstIdx`
    pub fn add_constant(&mut self, value: Value) -> usize {
        match self.try_add_constant(value) {
            Ok(idx) => idx,
            Err(ChunkError::ConstantPoolOverflow { count }) => panic!(
                "Chunk::add_constant: constant pool overflow ({} entries, max {}). \
                 This indicates a bug: the calling code should use `try_add_constant` \
                 and propagate the error as `CompileError` to preserve source location.",
                count,
                u16::MAX
            ),
        }
    }

    /// 从常量池读取常量值（返回克隆以保护内部数据）
    #[must_use]
    pub fn get_constant(&self, idx: usize) -> Option<Value> {
        self.constants.get(idx).cloned()
    }

    /// 添加调试信息：IP 地址 → 源码行号 + 列号映射
    pub fn add_debug_info(&mut self, ip: usize, line: usize, column: usize) {
        let debug_info = Arc::make_mut(&mut self.debug_info);
        debug_info.ip_to_line.insert(ip, line);
        if column > 0 {
            debug_info.ip_to_column.insert(ip, column);
        }
    }

    /// 获取指定 IP 位置的源码位置信息（用于错误报告）
    pub fn get_source_location(&self, ip: usize) -> Option<SourceLocation> {
        self.debug_info.ip_to_line.get(&ip).map(|&line| {
            let column = self.debug_info.ip_to_column.get(&ip).copied().unwrap_or(0);
            let source_line = if line > 0 && line <= self.debug_info.source_lines.len() {
                Some(self.debug_info.source_lines[line - 1].clone())
            } else {
                None
            };
            SourceLocation {
                file: self.debug_info.source_file.clone(),
                line,
                column,
                source_line,
                function_name: self.debug_info.function_name.clone(),
            }
        })
    }

    /// 读取单个字节 (内部使用，越界返回 None)
    fn read_byte(&self, offset: usize) -> Option<u8> {
        self.code.get(offset).copied()
    }

    /// 读取 16 位无符号整数 (小端序，内部使用)
    ///
    /// 需要连续 2 个字节可用，任一字节越界即返回 None。
    /// 解码过程：`low | (high << 8)`
    fn read_u16(&self, offset: usize) -> Option<u16> {
        let low = *self.code.get(offset)? as u16;
        let high = *self.code.get(offset + 1)? as u16;
        Some(low | (high << 8))
    }

    /// 读取 16 位有符号整数 (内部使用，补码转换)
    fn read_i16(&self, offset: usize) -> Option<i16> {
        Some(self.read_u16(offset)? as i16)
    }

    // ── 解码辅助方法 — 替代 generate_decode_method 内部宏，解决宏卫生性问题 ──
    //
    // 这些方法将"读取 + 推进游标"封装为方法调用，避免在 with_every_instruction!
    // 的 $dbody 中使用宏调用（Rust 宏卫生性阻止跨宏边界的宏名解析）。
    //
    // 每个方法：读取指定位置的操作数 → 推进 pos → 返回强类型包装器。
    // 越界时返回 None，由调用方通过 ? 运算符传播。

    /// 解码寄存器操作数: 读取 u16 → 推进 pos 2 字节 → 包装为 Reg
    fn decode_reg(&self, pos: &mut usize) -> Option<Reg> {
        let v = self.read_u16(*pos)?;
        *pos += 2;
        Some(Reg(v))
    }

    /// 解码常量池索引: 读取 u16 → 推进 pos 2 字节 → 包装为 ConstIdx
    fn decode_const(&self, pos: &mut usize) -> Option<ConstIdx> {
        let v = self.read_u16(*pos)?;
        *pos += 2;
        Some(ConstIdx(v))
    }

    /// 解码跳转偏移: 读取 i16 → 推进 pos 2 字节 → 包装为 Offset
    fn decode_offset(&self, pos: &mut usize) -> Option<Offset> {
        let v = self.read_i16(*pos)?;
        *pos += 2;
        Some(Offset(v))
    }

    /// 解码 u8 操作数: 读取 u8 → 推进 pos 1 字节 → 包装为 U8
    fn decode_u8_val(&self, pos: &mut usize) -> Option<U8> {
        let v = self.read_byte(*pos)?;
        *pos += 1;
        Some(U8(v))
    }

    /// 解码 u16 操作数: 读取 u16 → 推进 pos 2 字节 → 包装为 U16
    fn decode_u16_val(&self, pos: &mut usize) -> Option<U16> {
        let v = self.read_u16(*pos)?;
        *pos += 2;
        Some(U16(v))
    }

    /// 解码捕获槽索引: 读取 u16 → 推进 pos 2 字节 → 包装为 CaptureIdx
    fn decode_capture(&self, pos: &mut usize) -> Option<CaptureIdx> {
        let v = self.read_u16(*pos)?;
        *pos += 2;
        Some(CaptureIdx(v))
    }

    /// 获取字节码总长度 (字节数)
    #[must_use]
    pub fn len(&self) -> usize {
        self.code.len()
    }

    /// 检查字节码块是否为空
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.code.is_empty()
    }

    /// 解码操作码字节 (委托给 Opcode::decode_opcode，提供便捷接口)
    pub fn decode_opcode(byte: u8) -> Option<Opcode> {
        Opcode::decode_opcode(byte)
    }

    /// 解码 LSRA Spill 指令（SpillLoad/SpillStore）的操作数
    ///
    /// # 背景
    /// SpillLoad/SpillStore 是 `extra_dispatch` 特化指令，无对应 `Instruction` 变体，
    /// 因此 `Instruction::decode` 对其返回 `None`（与 GetGlobalCached 同类先例）。
    /// 本方法提供独立的解码能力，供 `disassemble()` 和外部测试使用。
    ///
    /// # 编码格式
    /// `opcode(1) + reg:u16(LE) + slot:u16(LE) = 5 字节`
    /// 本方法接收的 `bytes` 是**操作数部分**（不含 opcode 字节），需至少 4 字节。
    ///
    /// # 参数
    /// - `opcode_byte`: 指令操作码字节（必须为 54=SpillLoad 或 55=SpillStore）
    /// - `bytes`: 操作数字节切片（reg:u16 + slot:u16，小端序）
    ///
    /// # 返回
    /// - `Some((reg, slot, consumed))`: 解码成功；`consumed` 为操作数字节数（恒为 4）
    /// - `None`: opcode 非法或字节流不足 4 字节
    pub fn decode_spill(opcode_byte: u8, bytes: &[u8]) -> Option<(u16, u16, usize)> {
        // 校验 opcode：仅接受 SpillLoad(54) / SpillStore(55)
        if opcode_byte != Opcode::SpillLoad as u8 && opcode_byte != Opcode::SpillStore as u8 {
            return None;
        }
        // 操作数需至少 4 字节（reg:u16 + slot:u16）
        if bytes.len() < 4 {
            return None;
        }
        let reg = u16::from_le_bytes([bytes[0], bytes[1]]);
        let slot = u16::from_le_bytes([bytes[2], bytes[3]]);
        Some((reg, slot, 4))
    }

    /// 反汇编整个字节码块为人类可读文本
    ///
    /// # 输出格式示例
    /// ```text
    /// === File: example.nuzo ===
    ///
    ///   1 | let x = 10 + 32;
    ///       ^
    /// 0000  LoadK      r0     0    ; load constants[0] (10) into r0
    /// 0005  LoadK      r1     1    ; load constants[1] (32) into r1
    /// 0010  Add        r2   r0   r1  ; r2 = r0 add r1
    /// 0017  Print      r2          ; print r2
    /// 0020  Halt                   ; halt
    /// ```
    ///
    /// # 特性
    /// - 显示源代码行（如果 debug_info 可用）
    /// - 用 `^` 标记当前指令对应的源码位置
    /// - 显示常量值（对于 LoadK 指令）
    /// - 计算并显示跳转目标地址（绝对地址）
    /// - 对未知字节显示 `UNKNOWN` 警告
    ///
    /// # 错误处理
    /// 遇到损坏字节码时不会 panic，而是输出警告并继续反汇编下一条指令。
    pub fn disassemble(&self) -> String {
        let mut output = String::with_capacity(self.code.len() * 30);
        if !self.debug_info.source_file.is_empty() {
            output.push_str(&format!("=== File: {} ===\n\n", self.debug_info.source_file));
        }
        let mut last_displayed_line = 0;
        let mut offset = 0;
        while offset < self.code.len() {
            if let Some(source_loc) = self.get_source_location(offset)
                && source_loc.line != last_displayed_line
                && source_loc.line > 0
            {
                last_displayed_line = source_loc.line;
                if let Some(ref source_line) = source_loc.source_line {
                    output.push_str(&format!(
                        "{:3} | {}\n",
                        source_loc.line,
                        source_line.trim_end()
                    ));
                    if source_loc.column > 0 {
                        let spaces = " ".repeat(source_loc.column + SOURCE_LINE_PREFIX_WIDTH);
                        output.push_str(&format!("{}^\n", spaces));
                    }
                }
            }
            if let Some((instr, next)) = Instruction::decode(self, offset) {
                output.push_str(&self.disasm_instruction(&instr, offset));
                offset = next;
            } else {
                let byte = self.read_byte(offset).unwrap_or(0);
                if let Some(op) = Opcode::decode_opcode(byte) {
                    let size = op.instruction_size();
                    // SpillLoad/SpillStore: extra_dispatch 特化指令，无 Instruction 变体，
                    // 使用专用格式 `SpillXxx R{reg}, [{slot}]`（通用路径无法表达 R/[ ] 语义）。
                    // 操作数截断时 fall through 到通用路径做降级显示。
                    if (op == Opcode::SpillLoad || op == Opcode::SpillStore)
                        && let Some((reg, slot, _)) =
                            self.code.get(offset + 1..).and_then(|b| Chunk::decode_spill(byte, b))
                    {
                        output.push_str(&format!(
                            "{:04x}  {:<10} R{}, [{}]  ; {} R{}, [{}]",
                            offset,
                            op.name(),
                            reg,
                            slot,
                            op.name(),
                            reg,
                            slot
                        ));
                        offset += size;
                        if offset < self.code.len() {
                            output.push('\n');
                        }
                        continue;
                    }
                    output.push_str(&format!("{:04x}  {:<10}", offset, op.name()));
                    // 通用操作数解码
                    let mut pos = offset + 1;
                    for kind in op.operands() {
                        match kind {
                            OperandKind::Reg
                            | OperandKind::Const
                            | OperandKind::U16
                            | OperandKind::CaptureIdx => {
                                if let Some(v) = self.read_u16(pos) {
                                    output.push_str(&format!(" {:>4}", v));
                                    pos += 2;
                                }
                            }
                            OperandKind::Offset => {
                                if let Some(lo) = self.read_byte(pos).zip(self.read_byte(pos + 1)) {
                                    let raw = lo.0 as u16 | ((lo.1 as u16) << 8);
                                    let signed = raw as i16;
                                    let next_ip = pos + 2 - 2 + size;
                                    let target = next_ip as i32 + signed as i32;
                                    if target >= 0 {
                                        output.push_str(&format!(
                                            " {:>4}  ; -> {:04x}",
                                            signed, target as usize
                                        ));
                                    } else {
                                        output.push_str(&format!(" {:>4}  ; -> INVALID", signed));
                                    }
                                    pos += 2;
                                }
                            }
                            OperandKind::U8 => {
                                if let Some(v) = self.read_byte(pos) {
                                    output.push_str(&format!(" {:>3}", v));
                                    pos += 1;
                                }
                            }
                            OperandKind::U32 => {
                                if pos + 4 <= self.code.len() {
                                    let bytes = &self.code[pos..pos + 4];
                                    let v = u32::from_le_bytes([
                                        bytes[0], bytes[1], bytes[2], bytes[3],
                                    ]);
                                    output.push_str(&format!(" {:>6}", v));
                                    pos += 4;
                                }
                            }
                            OperandKind::None => {}
                        }
                    }
                    offset += size;
                } else {
                    output.push_str(&format!("{:04x}  UNKNOWN ({:02x})", offset, byte));
                    offset += 1;
                }
            }
            if offset < self.code.len() {
                output.push('\n');
            }
        }
        output
    }

    /// **维护提醒**：新增 `Instruction` 变体时，**必须**在对应的分类方法中同步添加反汇编格式化分支。
    /// 格式保持与其他指令一致：`{:<10} 操作数列表  ; 注释`
    ///
    /// # 指令分类分发
    /// 按指令类别分发到对应的格式化方法，每个方法负责一类指令的反汇编输出：
    /// - [`disasm_load`] — 常量与移动指令
    /// - [`disasm_arithmetic`] — 算术与一元运算指令
    /// - [`disasm_comparison`] — 比较运算指令
    /// - [`disasm_control`] — 控制流指令（需要 ip 计算跳转目标）
    /// - [`disasm_property`] — 属性与索引访问指令
    /// - [`disasm_function_and_misc`] — 函数操作与杂项指令
    /// - [`disasm_closure`] — 闭包捕获指令
    /// - [`disasm_global_and_range`] — 全局变量与范围指令
    fn disasm_instruction(&self, instr: &Instruction, ip: usize) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = write!(s, "{:04x}  ", ip);

        match instr {
            // 常量与移动指令
            Instruction::LoadK { .. }
            | Instruction::LoadNil { .. }
            | Instruction::LoadTrue { .. }
            | Instruction::LoadFalse { .. }
            | Instruction::Mov { .. } => s.push_str(&self.disasm_load(instr)),

            // 算术与一元运算指令
            Instruction::Add { .. }
            | Instruction::Sub { .. }
            | Instruction::Mul { .. }
            | Instruction::Div { .. }
            | Instruction::Rem { .. }
            | Instruction::Mod { .. }
            | Instruction::Pow { .. }
            | Instruction::Neg { .. }
            | Instruction::Not { .. } => s.push_str(&self.disasm_arithmetic(instr)),

            // 比较运算指令
            Instruction::Eq { .. }
            | Instruction::Neq { .. }
            | Instruction::Lt { .. }
            | Instruction::Gt { .. }
            | Instruction::Le { .. }
            | Instruction::Ge { .. } => s.push_str(&self.disasm_comparison(instr)),

            // 控制流指令
            Instruction::Jmp { .. } | Instruction::Test { .. } => {
                s.push_str(&self.disasm_control(instr, ip))
            }

            // 属性与索引访问指令
            Instruction::GetProp { .. }
            | Instruction::SetProp { .. }
            | Instruction::GetIndex { .. }
            | Instruction::SetIndex { .. }
            | Instruction::SetIndexMut { .. } => s.push_str(&self.disasm_property(instr)),
            // 函数操作与杂项指令
            Instruction::Call { .. }
            | Instruction::Return { .. }
            | Instruction::Closure { .. }
            | Instruction::Print { .. }
            | Instruction::Halt
            | Instruction::ArrayNew { .. }
            | Instruction::StringBuild { .. }
            | Instruction::Len { .. }
            | Instruction::SliceChainNew { .. }
            | Instruction::SliceChainAppend { .. }
            | Instruction::SliceChainFinish { .. } => {
                s.push_str(&self.disasm_function_and_misc(instr))
            }

            // 闭包捕获指令
            Instruction::Capture { .. }
            | Instruction::GetCaptured { .. }
            | Instruction::SetCaptured { .. } => s.push_str(&self.disasm_closure(instr)),

            // 全局变量与范围指令
            Instruction::GetGlobal { .. }
            | Instruction::SetGlobal { .. }
            | Instruction::RangeNew { .. }
            | Instruction::InitModule { .. } => s.push_str(&self.disasm_global_and_range(instr)),

            // 异常处理指令
            Instruction::TryStart { .. } | Instruction::TryEnd | Instruction::Out { .. } => {
                s.push_str(&self.disasm_exception(instr, ip))
            }
        }

        s
    }

    /// 反汇编常量与移动指令 (LoadK, LoadNil, LoadTrue, LoadFalse, Mov)
    fn disasm_load(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::LoadK { dest, const_idx } => {
                let val = self
                    .get_constant(const_idx.0 as usize)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "<out of bounds>".to_string());
                write!(
                    s,
                    "{:<10} {:>3}  {:>4}    ; load constants[{}] ({}) into r{}",
                    "LoadK", dest.0, const_idx.0, const_idx.0, val, dest.0
                )
                .expect("Display formatting should never fail");
            }
            Instruction::LoadNil { dest } => {
                write!(s, "{:<10} {:>3}          ; r{} = loadnil", "LoadNil", dest.0, dest.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::LoadTrue { dest } => {
                write!(s, "{:<10} {:>3}          ; r{} = loadtrue", "LoadTrue", dest.0, dest.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::LoadFalse { dest } => {
                write!(s, "{:<10} {:>3}          ; r{} = loadfalse", "LoadFalse", dest.0, dest.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::Mov { dest, src } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{} = r{}",
                "Mov", dest.0, src.0, dest.0, src.0
            )
            .expect("Display formatting should never fail"),
            _ => unreachable!("disasm_load called with non-load instruction"),
        }
        s
    }

    /// 反汇编算术与一元运算指令 (Add, Sub, Mul, Div, Rem, Mod, Pow, Neg, Not)
    fn disasm_arithmetic(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::Add { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} add r{}",
                "Add", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Sub { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} sub r{}",
                "Sub", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Mul { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} mul r{}",
                "Mul", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Div { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} div r{}",
                "Div", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Rem { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} rem r{}",
                "Rem", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Mod { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} mod r{}",
                "Mod", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Pow { dest, base, exp } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} pow r{}",
                "Pow", dest.0, base.0, exp.0, dest.0, base.0, exp.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Neg { dest, src } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{} = neg r{}",
                "Neg", dest.0, src.0, dest.0, src.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Not { dest, src } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{} = not r{}",
                "Not", dest.0, src.0, dest.0, src.0
            )
            .expect("Display formatting should never fail"),
            _ => unreachable!("disasm_arithmetic called with non-arithmetic instruction"),
        }
        s
    }

    /// 反汇编比较运算指令 (Eq, Neq, Lt, Gt, Le, Ge)
    fn disasm_comparison(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::Eq { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} == r{}",
                "Eq", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Neq { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} != r{}",
                "Neq", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Lt { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} < r{}",
                "Lt", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Gt { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} > r{}",
                "Gt", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Le { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} <= r{}",
                "Le", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Ge { dest, left, right } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>3}  ; r{} = r{} >= r{}",
                "Ge", dest.0, left.0, right.0, dest.0, left.0, right.0
            )
            .expect("Display formatting should never fail"),
            _ => unreachable!("disasm_comparison called with non-comparison instruction"),
        }
        s
    }

    /// 反汇编控制流指令 (Jmp, Test) — 需要计算跳转目标地址
    fn disasm_control(&self, instr: &Instruction, ip: usize) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::Jmp { offset } => {
                let next_ip = ip + instr.size();
                let target_ip = next_ip as i32 + offset.0 as i32;
                // 处理损坏的字节码导致的负数跳转目标
                if target_ip < 0 {
                    write!(
                        s,
                        "{:<10} {:>6}       ; jump to INVALID (negative target)",
                        "Jmp", offset.0
                    )
                    .expect("Display formatting should never fail");
                } else {
                    write!(
                        s,
                        "{:<10} {:>6}       ; jump to {:04x}",
                        "Jmp", offset.0, target_ip as usize
                    )
                    .expect("Display formatting should never fail");
                }
            }
            Instruction::Test { reg, offset } => {
                let next_ip = ip + instr.size();
                let target_ip = next_ip as i32 + offset.0 as i32;
                if target_ip < 0 {
                    write!(
                        s,
                        "{:<10} {:>3}  {:>6}    ; test r{} and jump to INVALID if falsy",
                        "Test", reg.0, offset.0, reg.0
                    )
                    .expect("Display formatting should never fail");
                } else {
                    write!(
                        s,
                        "{:<10} {:>3}  {:>6}    ; test r{} and jump to {:04x} if falsy",
                        "Test", reg.0, offset.0, reg.0, target_ip as usize
                    )
                    .expect("Display formatting should never fail");
                }
            }
            _ => unreachable!("disasm_control called with non-control instruction"),
        }
        s
    }

    /// 反汇编属性与索引访问指令 (GetProp, SetProp, GetIndex, SetIndex, SetIndexMut)
    fn disasm_property(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::GetProp { dest, obj, prop } => {
                write!(s, "{:<10} {:>3}  {:>3}  {:>4}    ", "GetProp", dest.0, obj.0, prop.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::SetProp { obj, prop, val } => {
                write!(s, "{:<10} {:>3}  {:>4}  {:>3}    ", "SetProp", obj.0, prop.0, val.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::GetIndex { dest, obj, index } => {
                write!(s, "{:<10} {:>3}  {:>3}  {:>3}    ", "GetIndex", dest.0, obj.0, index.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::SetIndex { obj, index, val } => {
                write!(s, "{:<10} {:>3}  {:>3}  {:>3}    ", "SetIndex", obj.0, index.0, val.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::SetIndexMut { obj, index, val } => {
                write!(s, "{:<10} {:>3}  {:>3}  {:>3}    ", "SetIndexMut", obj.0, index.0, val.0)
                    .expect("Display formatting should never fail")
            }
            _ => unreachable!("disasm_property called with non-property instruction"),
        }
        s
    }

    /// 反汇编函数操作与杂项指令 (Call, Return, Closure, Print, Halt, ArrayNew, Len)
    fn disasm_function_and_misc(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::Call { func, argc } => write!(
                s,
                "{:<10} {:>3}  {:>3}       ; call r{} with {} args",
                "Call", func.0, argc.0, func.0, argc.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Return { val } => write!(s, "{:<10} {:>3}          ", "Return", val.0)
                .expect("Display formatting should never fail"),
            Instruction::Closure { dest, proto } => write!(
                s,
                "{:<10} {:>3}  {:>4}    ; create closure from prototype[{}] in r{}",
                "Closure", dest.0, proto.0, proto.0, dest.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Print { reg } => {
                write!(s, "{:<10} {:>3}          ; print r{}", "Print", reg.0, reg.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::Halt => write!(s, "{:<10}         ; halt", "Halt")
                .expect("Display formatting should never fail"),
            Instruction::ArrayNew { dest, count } => {
                write!(s, "{:<10} {:>3}  {:>4}    ", "ArrayNew", dest.0, count.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::StringBuild { dest, start, count } => write!(
                s,
                "{:<10} {:>3}  {:>3}  {:>4}    ; r{} = concat(r{}..r{}+{})",
                "StringBuild", dest.0, start.0, count.0, dest.0, start.0, start.0, count.0
            )
            .expect("Display formatting should never fail"),
            Instruction::Len { dest, src } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{} = len r{}",
                "Len", dest.0, src.0, dest.0, src.0
            )
            .expect("Display formatting should never fail"),
            Instruction::SliceChainNew { dest } => write!(
                s,
                "{:<10} {:>3}          ; r{} = SliceChain.new()",
                "SliceChainNew", dest.0, dest.0
            )
            .expect("Display formatting should never fail"),
            Instruction::SliceChainAppend { chain, src } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{}.append(r{})",
                "SliceChainAppend", chain.0, src.0, chain.0, src.0
            )
            .expect("Display formatting should never fail"),
            Instruction::SliceChainFinish { dest, chain } => write!(
                s,
                "{:<10} {:>3}  {:>3}     ; r{} = r{}.finish()",
                "SliceChainFinish", dest.0, chain.0, dest.0, chain.0
            )
            .expect("Display formatting should never fail"),
            _ => unreachable!("disasm_function_and_misc called with non-function/misc instruction"),
        }
        s
    }

    /// 反汇编闭包捕获指令 (Capture, GetCaptured, SetCaptured)
    fn disasm_closure(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::Capture { closure, idx, source } => match source {
                CapturedSource::ByValue(reg) => write!(
                    s,
                    "{:<10} r{:>2}  {:>3}  r{:>3}    ; CAPTURE r{}, {}, r{}",
                    "Capture", closure.0, idx.0, reg.0, closure.0, idx.0, reg.0
                )
                .expect("Display formatting should never fail"),
                CapturedSource::Outer(n) => write!(
                    s,
                    "{:<10} r{:>2}  {:>3}  ^{:>3}    ; CAPTURE r{}, {}, ^{}",
                    "Capture", closure.0, idx.0, n, closure.0, idx.0, n
                )
                .expect("Display formatting should never fail"),
            },
            Instruction::GetCaptured { dest, idx } => {
                write!(s, "{:<10} {:>3}  cap#{:>3} ", "GetCaptured", dest.0, idx.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::SetCaptured { idx, val } => {
                write!(s, "{:<10} cap#{:>3}  {:>3}    ", "SetCaptured", idx.0, val.0)
                    .expect("Display formatting should never fail")
            }
            _ => unreachable!("disasm_closure called with non-closure instruction"),
        }
        s
    }

    /// 反汇编全局变量与范围指令 (GetGlobal, SetGlobal, RangeNew, InitModule)
    fn disasm_global_and_range(&self, instr: &Instruction) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::GetGlobal { dest, name, .. } => {
                write!(s, "{:<10} {:>3}  {:>4}    ", "GetGlobal", dest.0, name.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::SetGlobal { val, name } => {
                write!(s, "{:<10} {:>3}  {:>4}    ", "SetGlobal", val.0, name.0)
                    .expect("Display formatting should never fail")
            }
            Instruction::RangeNew { dest, start, end, inclusive } => {
                let range_kind = if inclusive.0 != 0 { "..=" } else { ".." };
                write!(
                    s,
                    "{:<10} {:>3}  {:>3}  {:>3}  {:>3}  ; r{} = r{}{}r{}",
                    "RangeNew",
                    dest.0,
                    start.0,
                    end.0,
                    inclusive.0,
                    dest.0,
                    start.0,
                    range_kind,
                    end.0
                )
                .expect("Display formatting should never fail");
            }
            Instruction::InitModule { module_idx, init_flag_slot } => {
                let module_path = self
                    .get_constant(module_idx.0 as usize)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "<out of bounds>".to_string());
                write!(
                    s,
                    "{:<10} {:>4}  {:>4}    ; init module constants[{}] ({}) -> slot {}",
                    "InitModule",
                    module_idx.0,
                    init_flag_slot.0,
                    module_idx.0,
                    module_path,
                    init_flag_slot.0
                )
                .expect("Display formatting should never fail");
            }
            _ => unreachable!("disasm_global_and_range called with non-global/range instruction"),
        }
        s
    }

    /// 反汇编异常处理指令 (TryStart, TryEnd, Out)
    fn disasm_exception(&self, instr: &Instruction, ip: usize) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        match instr {
            Instruction::TryStart { catch_offset, exception_reg } => {
                let next_ip = ip + instr.size();
                let target_ip = next_ip as i32 + catch_offset.0 as i32;
                if target_ip < 0 {
                    write!(
                        s,
                        "{:<10} {:>6}  {:>3}     ; try start -> INVALID",
                        "TryStart", catch_offset.0, exception_reg.0
                    )
                    .expect("Display formatting should never fail");
                } else {
                    write!(
                        s,
                        "{:<10} {:>6}  r{:>3}    ; try start -> {:04x}",
                        "TryStart", catch_offset.0, exception_reg.0, target_ip as usize
                    )
                    .expect("Display formatting should never fail");
                }
            }
            Instruction::TryEnd => {
                write!(s, "{:<10}         ; try end", "TryEnd")
                    .expect("Display formatting should never fail");
            }
            Instruction::Out { value_reg } => {
                write!(s, "{:<10} {:>3}          ; out r{}", "Out", value_reg.0, value_reg.0)
                    .expect("Display formatting should never fail");
            }
            _ => unreachable!("disasm_exception called with non-exception instruction"),
        }
        s
    }
}

impl Default for Chunk {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Display for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.disassemble())
    }
}

// ============================================================================
// 5. 测试套件 (完整保留)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::{FALSE, NIL, TRUE};

    #[test]
    fn test_all_opcodes_have_distinct_values() {
        let all_opcodes: Vec<u8> = Opcode::ALL.iter().map(|op| *op as u8).collect();
        assert_eq!(all_opcodes.len(), INSTRUCTION_COUNT);
        let unique: std::collections::HashSet<&u8> = all_opcodes.iter().collect();
        assert_eq!(unique.len(), INSTRUCTION_COUNT);
    }

    #[test]
    fn test_opcode_name_returns_correct_strings() {
        assert_eq!(Opcode::LoadK.name(), "LoadK");
        assert_eq!(Opcode::Add.name(), "Add");
        assert_eq!(Opcode::Halt.name(), "Halt");
        assert_eq!(Opcode::Print.name(), "Print");
    }

    #[test]
    fn test_opcode_display_matches_name() {
        assert_eq!(format!("{}", Opcode::Add), "Add");
        assert_eq!(format!("{}", Opcode::Halt), "Halt");
    }

    #[test]
    fn test_instruction_sizes() {
        assert_eq!(Opcode::Halt.instruction_size(), 1);
        assert_eq!(Opcode::LoadNil.instruction_size(), 3);
        assert_eq!(Opcode::LoadTrue.instruction_size(), 3);
        assert_eq!(Opcode::LoadFalse.instruction_size(), 3);
        assert_eq!(Opcode::Print.instruction_size(), 3);
        assert_eq!(Opcode::Return.instruction_size(), 3);
        assert_eq!(Opcode::Neg.instruction_size(), 5);
        assert_eq!(Opcode::Not.instruction_size(), 5);
        assert_eq!(Opcode::Jmp.instruction_size(), 3);
        assert_eq!(Opcode::Test.instruction_size(), 5);
        assert_eq!(Opcode::Call.instruction_size(), 4);
        assert_eq!(Opcode::Mov.instruction_size(), 5);
        assert_eq!(Opcode::Add.instruction_size(), 7);
        assert_eq!(Opcode::Sub.instruction_size(), 7);
        assert_eq!(Opcode::Mul.instruction_size(), 7);
        assert_eq!(Opcode::Div.instruction_size(), 7);
        assert_eq!(Opcode::Rem.instruction_size(), 7);
        assert_eq!(Opcode::Eq.instruction_size(), 7);
        assert_eq!(Opcode::Neq.instruction_size(), 7);
        assert_eq!(Opcode::Lt.instruction_size(), 7);
        assert_eq!(Opcode::Gt.instruction_size(), 7);
        assert_eq!(Opcode::Le.instruction_size(), 7);
        assert_eq!(Opcode::Ge.instruction_size(), 7);
        assert_eq!(Opcode::LoadK.instruction_size(), 5);
        assert_eq!(Opcode::GetProp.instruction_size(), 7);
        assert_eq!(Opcode::SetProp.instruction_size(), 7);
        assert_eq!(Opcode::GetIndex.instruction_size(), 7);
        assert_eq!(Opcode::SetIndex.instruction_size(), 7);
        assert_eq!(Opcode::SetIndexMut.instruction_size(), 7);
        assert_eq!(Opcode::Closure.instruction_size(), 5);
        assert_eq!(Opcode::ArrayNew.instruction_size(), 5);
        assert_eq!(Opcode::Capture.instruction_size(), 7);
        assert_eq!(Opcode::GetCaptured.instruction_size(), 5);
        assert_eq!(Opcode::SetCaptured.instruction_size(), 5);
        assert_eq!(Opcode::RangeNew.instruction_size(), 8);
        assert_eq!(Opcode::Mod.instruction_size(), 7);
        assert_eq!(Opcode::Len.instruction_size(), 5);
        assert_eq!(Opcode::Pow.instruction_size(), 7);
        // 异常处理指令
        assert_eq!(Opcode::TryStart.instruction_size(), 4);
        assert_eq!(Opcode::TryEnd.instruction_size(), 1);
        assert_eq!(Opcode::Out.instruction_size(), 3);
        // LSRA Spill 指令
        assert_eq!(Opcode::SpillLoad.instruction_size(), 5);
        assert_eq!(Opcode::SpillStore.instruction_size(), 5);
        // lazy import 指令
        assert_eq!(Opcode::InitModule.instruction_size(), 5);
    }

    #[test]
    fn test_opcode_roundtrip_decode() {
        // 有效 opcode 范围: 0-41, 50-55（排除保留槽位 34 和空闲间隙 42-49）
        // 注: slot 30 现已用于 InitModule
        let valid_bytes: Vec<u8> =
            (0..=55u8).filter(|&b| b != 34 && !(42..=49).contains(&b)).collect();
        for byte in valid_bytes {
            let decoded = Opcode::decode_opcode(byte);
            assert!(decoded.is_some(), "Should decode byte {}", byte);
            let op = decoded.unwrap();
            assert_eq!(op as u8, byte, "Roundtrip failed for byte {}", byte);
        }
    }

    #[test]
    fn test_invalid_opcode_returns_none() {
        assert_eq!(Opcode::decode_opcode(30), Some(Opcode::InitModule));
        assert!(Opcode::decode_opcode(34).is_none());
        assert_eq!(Opcode::decode_opcode(40), Some(Opcode::Pow));
        assert_eq!(Opcode::decode_opcode(41), Some(Opcode::SetIndexMut));
        assert_eq!(Opcode::decode_opcode(42), Some(Opcode::StringBuild));
        assert_eq!(Opcode::decode_opcode(50), Some(Opcode::GetGlobalCached));
        assert_eq!(Opcode::decode_opcode(51), Some(Opcode::TryStart));
        assert_eq!(Opcode::decode_opcode(52), Some(Opcode::TryEnd));
        assert_eq!(Opcode::decode_opcode(53), Some(Opcode::Out));
        assert_eq!(Opcode::decode_opcode(54), Some(Opcode::SpillLoad));
        assert_eq!(Opcode::decode_opcode(55), Some(Opcode::SpillStore));
        assert_eq!(Opcode::decode_opcode(56), Some(Opcode::SliceChainNew));
        assert_eq!(Opcode::decode_opcode(57), Some(Opcode::SliceChainAppend));
        assert_eq!(Opcode::decode_opcode(58), Some(Opcode::SliceChainFinish));
        assert!(Opcode::decode_opcode(59).is_none());
        assert!(Opcode::decode_opcode(255).is_none());
        assert!(Opcode::decode_opcode(128).is_none());
    }

    #[test]
    fn test_chunk_new_is_empty() {
        let chunk = Chunk::new();
        assert!(chunk.is_empty());
        assert_eq!(chunk.len(), 0);
        assert_eq!(chunk.constants().len(), 0);
        assert_eq!(chunk.lines().len(), 0);
    }

    #[test]
    fn test_chunk_default_is_empty() {
        let chunk = Chunk::default();
        assert!(chunk.is_empty());
    }

    #[test]
    fn test_write_opcode_increases_length() {
        let mut chunk = Chunk::new();
        assert_eq!(chunk.len(), 0);
        chunk.write_opcode(Opcode::Halt);
        assert_eq!(chunk.len(), 1);
        chunk.write_opcode(Opcode::LoadNil);
        assert_eq!(chunk.len(), 2);
    }

    #[test]
    fn test_write_byte_works() {
        let mut chunk = Chunk::new();
        chunk.write_byte(42);
        chunk.write_byte(255);
        chunk.write_byte(0);
        assert_eq!(chunk.len(), 3);
        assert_eq!(chunk.code()[0], 42);
        assert_eq!(chunk.code()[1], 255);
        assert_eq!(chunk.code()[2], 0);
    }

    #[test]
    fn test_add_constant_returns_index() {
        let mut chunk = Chunk::new();
        let idx0 = chunk.add_constant(Value::from_number(42.0));
        assert_eq!(idx0, 0);
        let idx1 = chunk.add_constant(Value::from_bool(true));
        assert_eq!(idx1, 1);
        let idx2 = chunk.add_constant(NIL);
        assert_eq!(idx2, 2);
        assert_eq!(chunk.constants().len(), 3);
    }

    #[test]
    fn test_add_constant_deduplication() {
        let mut chunk = Chunk::new();
        // 添加相同的数值应该返回相同索引
        let idx0 = chunk.add_constant(Value::from_number(42.0));
        let idx1 = chunk.add_constant(Value::from_number(42.0));
        assert_eq!(idx0, idx1, "Duplicate number should return same index");

        // 添加不同的数值应该返回不同索引
        let idx2 = chunk.add_constant(Value::from_number(99.0));
        assert_ne!(idx0, idx2, "Different numbers should return different indices");

        // 常量池大小应该只有 2（42.0 和 99.0），不是 3
        assert_eq!(chunk.constants().len(), 2);

        // 布尔值去重
        let idx3 = chunk.add_constant(TRUE);
        let idx4 = chunk.add_constant(TRUE);
        assert_eq!(idx3, idx4, "Duplicate TRUE should return same index");
        assert_eq!(chunk.constants().len(), 3);

        // NIL 去重
        let idx5 = chunk.add_constant(NIL);
        let idx6 = chunk.add_constant(NIL);
        assert_eq!(idx5, idx6, "Duplicate NIL should return same index");
        assert_eq!(chunk.constants().len(), 4);
    }

    #[test]
    fn test_add_constant_dedup_after_fold() {
        // 模拟常量折叠场景：3 + 5 = 8，然后代码中也有字面量 8
        let mut chunk = Chunk::new();
        let _idx_3 = chunk.add_constant(Value::from_number(3.0));
        let _idx_5 = chunk.add_constant(Value::from_number(5.0));
        // 常量折叠结果 8.0
        let idx_8_folded = chunk.add_constant(Value::from_number(8.0));
        // 代码中直接写的字面量 8.0
        let idx_8_literal = chunk.add_constant(Value::from_number(8.0));

        assert_eq!(idx_8_folded, idx_8_literal, "Folded and literal 8.0 should share same index");
        assert_eq!(chunk.constants().len(), 3, "Pool should have 3 constants: 3.0, 5.0, 8.0");
    }

    #[test]
    fn test_get_constant_retrieves_value() {
        let mut chunk = Chunk::new();
        let original = Value::from_number(2.5);
        let idx = chunk.add_constant(original);
        let retrieved = chunk.get_constant(idx);
        assert_eq!(retrieved, Some(original));
    }

    #[test]
    fn test_multiple_constants_stored_correctly() {
        let mut chunk = Chunk::new();
        let vals = vec![
            Value::from_number(1.0),
            Value::from_number(2.0),
            Value::from_number(3.0),
            TRUE,
            FALSE,
            NIL,
        ];
        for val in &vals {
            chunk.add_constant(*val);
        }
        assert_eq!(chunk.constants().len(), 6);
        for (i, val) in vals.iter().enumerate() {
            assert_eq!(chunk.get_constant(i), Some(*val), "Constant at index {} mismatch", i);
        }
    }

    #[test]
    fn test_get_constant_out_of_bounds_returns_none() {
        let chunk = Chunk::new();
        assert_eq!(chunk.get_constant(0), None);
        assert_eq!(chunk.get_constant(999), None);
    }

    #[test]
    fn test_write_read_u16_little_endian() {
        let mut chunk = Chunk::new();
        let test_values: Vec<u16> = vec![0, 1, 255, 256, 65535, 0x1234, 0xABCD];
        for &val in &test_values {
            chunk.write_u16(val);
        }
        let mut offset = 0;
        for &expected in &test_values {
            let actual = chunk.read_u16(offset);
            assert_eq!(actual, Some(expected));
            offset += 2;
        }
    }

    #[test]
    fn test_write_u16_max_value() {
        let mut chunk = Chunk::new();
        chunk.write_u16(u16::MAX);
        assert_eq!(chunk.read_u16(0), Some(u16::MAX));
    }

    #[test]
    fn test_write_u16_zero() {
        let mut chunk = Chunk::new();
        chunk.write_u16(0);
        assert_eq!(chunk.read_u16(0), Some(0));
        assert_eq!(chunk.code()[0], 0);
        assert_eq!(chunk.code()[1], 0);
    }

    #[test]
    fn test_write_read_i16_positive() {
        let mut chunk = Chunk::new();
        chunk.write_i16(100);
        assert_eq!(chunk.read_i16(0), Some(100));
    }

    #[test]
    fn test_write_read_i16_negative() {
        let mut chunk = Chunk::new();
        chunk.write_i16(-50);
        assert_eq!(chunk.read_i16(0), Some(-50));
    }

    #[test]
    fn test_write_read_i16_boundary_values() {
        let mut chunk = Chunk::new();
        chunk.write_i16(i16::MIN);
        assert_eq!(chunk.read_i16(0), Some(i16::MIN));
        chunk.write_i16(i16::MAX);
        assert_eq!(chunk.read_i16(2), Some(i16::MAX));
        chunk.write_i16(0);
        assert_eq!(chunk.read_i16(4), Some(0));
        chunk.write_i16(-1);
        assert_eq!(chunk.read_i16(6), Some(-1));
        chunk.write_i16(1);
        assert_eq!(chunk.read_i16(8), Some(1));
    }

    #[test]
    fn test_encode_halt_instruction() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Halt);
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.code()[0], Opcode::Halt as u8);
    }

    #[test]
    fn test_encode_load_nil_with_register() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(5);
        assert_eq!(chunk.len(), 3);
        assert_eq!(chunk.code()[0], Opcode::LoadNil as u8);
        assert_eq!(chunk.read_u16(1), Some(5));
    }

    #[test]
    fn test_encode_loadk_with_constant() {
        let mut chunk = Chunk::new();
        let const_idx = chunk.add_constant(Value::from_number(42.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(const_idx as u16);
        assert_eq!(chunk.len(), 5);
        assert_eq!(chunk.code()[0], Opcode::LoadK as u8);
        assert_eq!(chunk.read_u16(1), Some(0));
        assert_eq!(chunk.read_u16(3), Some(const_idx as u16));
    }

    #[test]
    fn test_encode_arithmetic_instruction() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);
        assert_eq!(chunk.len(), 7);
        assert_eq!(chunk.code()[0], Opcode::Add as u8);
        assert_eq!(chunk.read_u16(1), Some(2));
        assert_eq!(chunk.read_u16(3), Some(0));
        assert_eq!(chunk.read_u16(5), Some(1));
    }

    #[test]
    fn test_encode_jump_instruction() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(10);
        assert_eq!(chunk.len(), 3);
        assert_eq!(chunk.code()[0], Opcode::Jmp as u8);
        assert_eq!(chunk.read_i16(1), Some(10));
    }

    #[test]
    fn test_encode_backward_jump() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(-5);
        assert_eq!(chunk.read_i16(1), Some(-5));
    }

    #[test]
    fn test_max_register_index() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(65535);
        assert_eq!(chunk.read_u16(1), Some(65535));
    }

    #[test]
    fn test_large_constant_pool() {
        let mut chunk = Chunk::new();
        for i in 0..1000u64 {
            chunk.add_constant(Value::from_number(i as f64));
        }
        assert_eq!(chunk.constants().len(), 1000);
        let val = chunk.get_constant(999);
        assert_eq!(val, Some(Value::from_number(999.0)));
    }

    #[test]
    fn test_large_constant_index_encoding() {
        let mut chunk = Chunk::new();
        for i in 0..500u64 {
            chunk.add_constant(Value::from_number(i as f64));
        }
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(499);
        assert_eq!(chunk.read_u16(3), Some(499));
    }

    #[test]
    fn test_large_positive_jump_offset() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(i16::MAX);
        assert_eq!(chunk.read_i16(1), Some(i16::MAX));
    }

    #[test]
    fn test_large_negative_jump_offset() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(i16::MIN);
        assert_eq!(chunk.read_i16(1), Some(i16::MIN));
    }

    #[test]
    fn test_complex_instruction_sequence() {
        let mut chunk = Chunk::new();
        let c10 = chunk.add_constant(Value::from_number(10.0));
        let c32 = chunk.add_constant(Value::from_number(32.0));
        let c3 = chunk.add_constant(Value::from_number(3.0));

        chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(c10 as u16) });
        chunk.emit(Instruction::LoadK { dest: Reg(1), const_idx: ConstIdx(c32 as u16) });
        chunk.emit(Instruction::Add { dest: Reg(2), left: Reg(0), right: Reg(1) });
        chunk.emit(Instruction::LoadK { dest: Reg(3), const_idx: ConstIdx(c3 as u16) });
        chunk.emit(Instruction::Mul { dest: Reg(4), left: Reg(2), right: Reg(3) });
        chunk.emit(Instruction::Print { reg: Reg(4) });
        chunk.emit(Instruction::Halt);

        assert_eq!(chunk.len(), 33);
        assert_eq!(chunk.constants().len(), 3);
    }

    #[test]
    fn test_disassemble_simple_program() {
        let mut chunk = Chunk::new();
        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(32.0));

        chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(c1 as u16) });
        chunk.emit(Instruction::LoadK { dest: Reg(1), const_idx: ConstIdx(c2 as u16) });
        chunk.emit(Instruction::Add { dest: Reg(2), left: Reg(0), right: Reg(1) });
        chunk.emit(Instruction::Print { reg: Reg(2) });
        chunk.emit(Instruction::Halt);

        let output = chunk.disassemble();
        assert!(output.contains("LoadK"));
        assert!(output.contains("Add"));
        assert!(output.contains("Print"));
        assert!(output.contains("Halt"));
        assert!(output.contains("r0"));
        assert!(output.contains("r1"));
        assert!(output.contains("r2"));
        assert!(output.contains("0000"));
    }

    #[test]
    fn test_disassemble_format_contains_offsets_and_operands() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Halt);
        let output = chunk.disassemble();
        assert!(output.contains("0000"));
        assert!(output.contains("Halt"));
    }

    #[test]
    fn test_disassemble_shows_constant_values() {
        let mut chunk = Chunk::new();
        let idx = chunk.add_constant(Value::from_number(42.0));
        chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(idx as u16) });
        let output = chunk.disassemble();
        assert!(output.contains("42"));
    }

    #[test]
    fn test_disassemble_handles_all_instruction_types() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::LoadNil { dest: Reg(0) });
        chunk.emit(Instruction::LoadTrue { dest: Reg(1) });
        chunk.emit(Instruction::LoadFalse { dest: Reg(2) });
        chunk.emit(Instruction::Mov { dest: Reg(3), src: Reg(0) });
        chunk.emit(Instruction::Neg { dest: Reg(4), src: Reg(0) });
        chunk.emit(Instruction::Not { dest: Reg(5), src: Reg(0) });
        chunk.emit(Instruction::Jmp { offset: Offset(0) });
        chunk.emit(Instruction::Test { reg: Reg(0), offset: Offset(0) });
        chunk.emit(Instruction::Return { val: Reg(0) });
        chunk.emit(Instruction::Print { reg: Reg(0) });
        chunk.emit(Instruction::Halt);
        let output = chunk.disassemble();
        assert!(output.contains("LoadNil"));
        assert!(output.contains("LoadTrue"));
        assert!(output.contains("LoadFalse"));
        assert!(output.contains("Mov"));
        assert!(output.contains("Neg"));
        assert!(output.contains("Not"));
        assert!(output.contains("Jmp"));
        assert!(output.contains("Test"));
        assert!(output.contains("Return"));
        assert!(output.contains("Print"));
        assert!(output.contains("Halt"));
    }

    #[test]
    fn test_disassemble_shows_jump_targets() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Jmp { offset: Offset(10) });
        let output = chunk.disassemble();
        assert!(output.contains("jump to"));
    }

    #[test]
    fn test_chunk_display_trait() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Halt);
        let output = format!("{}", chunk);
        assert!(!output.is_empty());
        assert!(output.contains("Halt"));
    }

    #[test]
    fn test_empty_chunk_disassembly() {
        let chunk = Chunk::new();
        let output = chunk.disassemble();
        assert!(output.is_empty());
    }

    #[test]
    fn test_full_program_encode_decode_cycle() {
        let mut chunk = Chunk::new();
        let const_hello = chunk.add_constant(Value::from_number(42.0));
        let const_world = chunk.add_constant(Value::from_number(100.0));

        chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(const_hello as u16) });
        chunk.emit(Instruction::LoadK { dest: Reg(1), const_idx: ConstIdx(const_world as u16) });
        chunk.emit(Instruction::Add { dest: Reg(2), left: Reg(0), right: Reg(1) });
        chunk.emit(Instruction::Print { reg: Reg(2) });
        chunk.emit(Instruction::Halt);

        assert_eq!(chunk.code()[0], Opcode::LoadK as u8);
        assert_eq!(chunk.code()[5], Opcode::LoadK as u8);
        assert_eq!(chunk.code()[10], Opcode::Add as u8);
        assert_eq!(chunk.code()[17], Opcode::Print as u8);
        assert_eq!(chunk.code()[20], Opcode::Halt as u8);

        assert_eq!(chunk.read_u16(1), Some(0));
        assert_eq!(chunk.read_u16(3), Some(0));
        assert_eq!(chunk.read_u16(6), Some(1));
        assert_eq!(chunk.read_u16(8), Some(1));
        assert_eq!(chunk.read_u16(11), Some(2));
        assert_eq!(chunk.read_u16(13), Some(0));
        assert_eq!(chunk.read_u16(15), Some(1));
        assert_eq!(chunk.read_u16(18), Some(2));

        let disassembly = chunk.disassemble();
        assert!(!disassembly.is_empty());
        assert_eq!(chunk.get_constant(0), Some(Value::from_number(42.0)));
        assert_eq!(chunk.get_constant(1), Some(Value::from_number(100.0)));
    }

    #[test]
    fn test_comparison_instructions_encoding() {
        let mut chunk = Chunk::new();
        let ops = vec![
            Instruction::Eq { dest: Reg(0), left: Reg(1), right: Reg(2) },
            Instruction::Neq { dest: Reg(0), left: Reg(1), right: Reg(2) },
            Instruction::Lt { dest: Reg(0), left: Reg(1), right: Reg(2) },
            Instruction::Gt { dest: Reg(0), left: Reg(1), right: Reg(2) },
            Instruction::Le { dest: Reg(0), left: Reg(1), right: Reg(2) },
            Instruction::Ge { dest: Reg(0), left: Reg(1), right: Reg(2) },
        ];
        for instr in ops {
            chunk.emit(instr);
        }
        assert_eq!(chunk.len(), 42);
    }

    #[test]
    fn test_function_operations_encoding() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Call { func: Reg(1), argc: U8(3) });
        chunk.emit(Instruction::Return { val: Reg(1) });
        let proto_idx = chunk.add_constant(NIL);
        chunk.emit(Instruction::Closure { dest: Reg(2), proto: ConstIdx(proto_idx as u16) });
        assert_eq!(chunk.len(), 12);
    }

    #[test]
    fn test_object_property_operations_encoding() {
        let mut chunk = Chunk::new();
        let prop_idx = chunk.add_constant(Value::from_number(0.0));
        chunk.emit(Instruction::GetProp {
            dest: Reg(0),
            obj: Reg(1),
            prop: ConstIdx(prop_idx as u16),
        });
        chunk.emit(Instruction::SetProp {
            obj: Reg(1),
            prop: ConstIdx(prop_idx as u16),
            val: Reg(2),
        });
        assert_eq!(chunk.len(), 14);
    }

    #[test]
    fn test_opcode_groups_are_coherent() {
        let arithmetic_ops =
            vec![Opcode::Add, Opcode::Sub, Opcode::Mul, Opcode::Div, Opcode::Rem, Opcode::Neg];
        for op in &arithmetic_ops {
            let size = op.instruction_size();
            assert!(
                size == 5 || size == 7,
                "Arithmetic op {} should be 5 or 7 bytes, got {}",
                op,
                size
            );
        }
        let comparison_ops =
            vec![Opcode::Eq, Opcode::Neq, Opcode::Lt, Opcode::Gt, Opcode::Le, Opcode::Ge];
        for op in &comparison_ops {
            assert_eq!(op.instruction_size(), 7, "Comparison op {} should be 7 bytes", op);
        }
    }

    #[test]
    fn test_stress_test_many_instructions() {
        let mut chunk = Chunk::new();
        for i in 0..1000u16 {
            chunk.emit(Instruction::LoadNil { dest: Reg(i) });
        }
        assert_eq!(chunk.len(), 3000);
        let output = chunk.disassemble();
        assert!(!output.is_empty());
        let line_count = output.matches('\n').count() + 1;
        assert_eq!(line_count, 1000);
    }

    #[test]
    fn test_closure_capture_opcode_encoding() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Capture {
            closure: Reg(0),
            idx: CaptureIdx(1),
            source: CapturedSource::ByValue(Reg(0)),
        });
        assert_eq!(chunk.len(), 7);
        assert_eq!(chunk.code()[0], Opcode::Capture as u8);
        assert_eq!(chunk.read_u16(1), Some(0));
        assert_eq!(chunk.read_u16(3), Some(1));
        assert_eq!(chunk.read_u16(5), Some(0));

        chunk.emit(Instruction::GetCaptured { dest: Reg(2), idx: CaptureIdx(1) });
        assert_eq!(chunk.len(), 12);
        assert_eq!(chunk.code()[7], Opcode::GetCaptured as u8);
        assert_eq!(chunk.read_u16(8), Some(2));
        assert_eq!(chunk.read_u16(10), Some(1));

        chunk.emit(Instruction::SetCaptured { idx: CaptureIdx(1), val: Reg(3) });
        assert_eq!(chunk.len(), 17);
        assert_eq!(chunk.code()[12], Opcode::SetCaptured as u8);
        assert_eq!(chunk.read_u16(13), Some(1));
        assert_eq!(chunk.read_u16(15), Some(3));
    }

    #[test]
    fn test_closure_capture_disassembly() {
        let mut chunk = Chunk::new();
        chunk.emit(Instruction::Capture {
            closure: Reg(0),
            idx: CaptureIdx(1),
            source: CapturedSource::ByValue(Reg(5)),
        });
        chunk.emit(Instruction::Capture {
            closure: Reg(2),
            idx: CaptureIdx(0),
            source: CapturedSource::Outer(3),
        });
        chunk.emit(Instruction::GetCaptured { dest: Reg(3), idx: CaptureIdx(1) });
        chunk.emit(Instruction::SetCaptured { idx: CaptureIdx(0), val: Reg(4) });

        let output = chunk.disassemble();
        assert!(output.contains("Capture"));
        assert!(output.contains("GetCaptured"));
        assert!(output.contains("SetCaptured"));
        assert!(output.contains("^3"));
        assert!(output.contains("r5"));
        assert!(output.contains("r0"));
        assert!(output.contains("r2"));
        assert!(output.contains("  4"));
    }

    #[test]
    fn test_chunk_decode_opcode_delegates_to_opcode() {
        assert_eq!(Chunk::decode_opcode(0), Some(Opcode::LoadK));
        assert_eq!(Chunk::decode_opcode(28), Some(Opcode::Halt));
        assert_eq!(Chunk::decode_opcode(39), Some(Opcode::Len));
        assert_eq!(Chunk::decode_opcode(40), Some(Opcode::Pow));
        assert_eq!(Chunk::decode_opcode(41), Some(Opcode::SetIndexMut));
        assert_eq!(Chunk::decode_opcode(42), Some(Opcode::StringBuild));
        assert_eq!(Chunk::decode_opcode(50), Some(Opcode::GetGlobalCached));
        assert_eq!(Chunk::decode_opcode(51), Some(Opcode::TryStart));
        assert_eq!(Chunk::decode_opcode(52), Some(Opcode::TryEnd));
        assert_eq!(Chunk::decode_opcode(53), Some(Opcode::Out));
        assert_eq!(Chunk::decode_opcode(54), Some(Opcode::SpillLoad));
        assert_eq!(Chunk::decode_opcode(55), Some(Opcode::SpillStore));
        assert_eq!(Chunk::decode_opcode(56), Some(Opcode::SliceChainNew));
        assert_eq!(Chunk::decode_opcode(57), Some(Opcode::SliceChainAppend));
        assert_eq!(Chunk::decode_opcode(58), Some(Opcode::SliceChainFinish));
        assert!(Chunk::decode_opcode(59).is_none());
    }

    #[test]
    fn test_operands_method() {
        assert_eq!(Opcode::Halt.operands(), &[]);
        assert_eq!(Opcode::LoadK.operands(), &[OperandKind::Reg, OperandKind::Const]);
        assert_eq!(Opcode::Add.operands(), &[OperandKind::Reg, OperandKind::Reg, OperandKind::Reg]);
        assert_eq!(Opcode::Jmp.operands(), &[OperandKind::Offset]);
        assert_eq!(Opcode::Call.operands(), &[OperandKind::Reg, OperandKind::U8]);
        assert_eq!(Opcode::ArrayNew.operands(), &[OperandKind::Reg, OperandKind::U16]);
        assert_eq!(
            Opcode::RangeNew.operands(),
            &[OperandKind::Reg, OperandKind::Reg, OperandKind::Reg, OperandKind::U8]
        );
        // Capture 系列：第二操作数是 CaptureIdx（语义不同于 Reg，防呆设计）
        assert_eq!(
            Opcode::Capture.operands(),
            &[OperandKind::Reg, OperandKind::CaptureIdx, OperandKind::Reg]
        );
        assert_eq!(Opcode::GetCaptured.operands(), &[OperandKind::Reg, OperandKind::CaptureIdx]);
        assert_eq!(Opcode::SetCaptured.operands(), &[OperandKind::CaptureIdx, OperandKind::Reg]);
        // 异常处理指令
        assert_eq!(Opcode::TryStart.operands(), &[OperandKind::Offset, OperandKind::U8]);
        assert_eq!(Opcode::TryEnd.operands(), &[]);
        assert_eq!(Opcode::Out.operands(), &[OperandKind::Reg]);
        // LSRA Spill 指令
        assert_eq!(Opcode::SpillLoad.operands(), &[OperandKind::Reg, OperandKind::U16]);
        assert_eq!(Opcode::SpillStore.operands(), &[OperandKind::Reg, OperandKind::U16]);
    }

    #[test]
    fn test_disasm_template_method() {
        assert_eq!(Opcode::Halt.disasm_template(), Some("halt"));
        assert_eq!(Opcode::LoadK.disasm_template(), None);
        assert_eq!(Opcode::Add.disasm_template(), None);
    }

    // ========================================================================
    // C1 回归测试:常量池溢出保护(try_add_constant / add_constant)
    // ========================================================================
    //
    // # 背景
    // C1 bug 修复引入了 `try_add_constant` (fallible) 和
    // `ChunkError::ConstantPoolOverflow`,用于处理常量池超过 `u16::MAX` 的情况。
    // 旧的 `add_constant` 保留为兜底 panic 版本。
    //
    // # 测试策略(科学方法:控制变量 + 快速构造溢出场景)
    // 逐个 add_constant 添加 65536 个常量过慢,这里通过 `constants_mut()` 直接
    // 填充常量池内部状态,绕过去重索引(constant_index),快速构造溢出场景:
    // - `try_add_constant` 先查 constant_index,未命中才检查 `len > u16::MAX`
    // - 直接 resize 常量池到指定大小但不更新 constant_index,
    //   再添加新值时去重必然未命中,走到 len 检查触发溢出/成功

    /// 测试 try_add_constant 正常添加常量并返回正确索引(含去重)
    #[test]
    fn test_try_add_constant_success() {
        let mut chunk = Chunk::new();

        // 添加第一个常量,应返回索引 0
        let idx0 = chunk.try_add_constant(Value::from_number(42.0));
        assert!(idx0.is_ok(), "首次添加常量应成功");
        assert_eq!(idx0.unwrap(), 0);

        // 添加不同的常量,应返回新索引 1
        let idx1 = chunk.try_add_constant(Value::from_number(99.0));
        assert!(idx1.is_ok(), "添加不同常量应成功");
        assert_eq!(idx1.unwrap(), 1);

        // 添加已存在的常量,应返回已有索引(去重)
        let idx_dedup = chunk.try_add_constant(Value::from_number(42.0));
        assert!(idx_dedup.is_ok(), "去重命中应返回 Ok");
        assert_eq!(idx_dedup.unwrap(), 0, "去重应返回原索引");

        // 常量池大小应为 2(42.0 和 99.0)
        assert_eq!(chunk.constants().len(), 2);
    }

    /// 测试 try_add_constant 在常量池满时返回 ConstantPoolOverflow 错误
    #[test]
    fn test_try_add_constant_overflow() {
        let mut chunk = Chunk::new();

        // 直接通过 constants_mut 填充常量池到 u16::MAX + 1 个元素,
        // 模拟常量池已溢出的状态(避免逐个添加 65536 次的慢路径)
        let overflow_count: usize = (u16::MAX as usize) + 1;
        chunk.constants_mut().resize(overflow_count, Value::from_number(0.0));

        // 尝试添加一个新值(不在 constant_index 中,去重未命中)
        let result = chunk.try_add_constant(Value::from_number(1.0));

        // 应返回溢出错误
        assert!(result.is_err(), "常量池满时应返回 Err");
        match result {
            Err(ChunkError::ConstantPoolOverflow { count }) => {
                assert_eq!(count, overflow_count, "错误中的 count 应为当前常量池大小");
            }
            other => panic!("期望 ConstantPoolOverflow, 实际得到: {:?}", other),
        }
    }

    /// 测试旧 API add_constant 在常量池溢出时 panic
    #[test]
    #[should_panic(expected = "constant pool overflow")]
    fn test_add_constant_panics_on_overflow() {
        let mut chunk = Chunk::new();

        // 直接填充常量池到溢出状态
        let overflow_count: usize = (u16::MAX as usize) + 1;
        chunk.constants_mut().resize(overflow_count, Value::from_number(0.0));

        // 调用旧 API,内部 try_add_constant 返回 Err 后 expect 触发 panic
        chunk.add_constant(Value::from_number(1.0));
    }

    // ── 自动生成: 由 build.rs 从 define_opcodes! 宏调用解析生成 ────────
    include!(concat!(env!("OUT_DIR"), "/generated_tests.rs"));

    #[test]
    fn test_try_add_constant_at_max_boundary() {
        let mut chunk = Chunk::new();

        // 填充常量池到 u16::MAX 个元素(索引 0..=u16::MAX-1 已占用)
        // 下一个新常量将获得索引 u16::MAX,应为合法操作
        let filled_count: usize = u16::MAX as usize;
        chunk.constants_mut().resize(filled_count, Value::from_number(0.0));

        // 添加一个新值(去重未命中),应成功返回索引 u16::MAX
        let result = chunk.try_add_constant(Value::from_number(1.0));
        assert!(result.is_ok(), "常量池恰好填满 u16::MAX 个时,添加最后一个应成功");
        assert_eq!(result.unwrap(), u16::MAX as usize, "最后一个常量索引应为 u16::MAX");

        // 此时常量池大小为 u16::MAX + 1,再添加一个新值应触发溢出
        let overflow_result = chunk.try_add_constant(Value::from_number(2.0));
        assert!(overflow_result.is_err(), "常量池达到 u16::MAX + 1 后应溢出");
    }

    #[test]
    fn test_from_arcs_rebuilds_constant_index() {
        let mut original = Chunk::new();
        let idx0 = original.add_constant(Value::from_number(42.0));
        let _idx1 = original.add_constant(Value::from_number(99.0));
        let idx2 = original.add_constant(Value::from_number(42.0)); // duplicate, should return idx0
        assert_eq!(idx2, idx0, "try_add_constant should deduplicate");

        let (code, constants, lines, debug_info, lc, sc) = original.into_parts();
        let mut rebuilt = Chunk::from_arcs(code, constants, lines, debug_info, lc, sc);

        // 验证重建后的常量池内容一致
        assert_eq!(rebuilt.constants().len(), 2);
        assert_eq!(rebuilt.constants()[0], Value::from_number(42.0));
        assert_eq!(rebuilt.constants()[1], Value::from_number(99.0));

        // 验证重建后的去重索引指向首次出现
        let re_idx = rebuilt.try_add_constant(Value::from_number(42.0)).unwrap();
        assert_eq!(re_idx, 0, "from_arcs should rebuild index mapping to first occurrence");
        let re_idx2 = rebuilt.try_add_constant(Value::from_number(99.0)).unwrap();
        assert_eq!(re_idx2, 1);
    }

    #[test]
    fn test_into_parts_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.add_constant(Value::from_number(1.0));
        chunk.add_constant(Value::from_number(2.0));
        chunk.emit(Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(0) });
        chunk.emit(Instruction::Halt);
        chunk.add_debug_info(0, 1, 1);

        let (code, constants, lines, debug_info, lc, sc) = chunk.into_parts();
        let rebuilt = Chunk::from_arcs(code, constants, lines, debug_info, lc, sc);

        assert_eq!(rebuilt.code(), &[Opcode::LoadK as u8, 0, 0, 0, 0, Opcode::Halt as u8]);
        assert_eq!(rebuilt.constants().len(), 2);
        assert_eq!(rebuilt.len(), 6);
    }

    #[test]
    fn test_decode_truncated_operand() {
        let mut chunk = Chunk::new();
        // 写入 LoadK 的操作码 + 只有 1 字节操作数（截断）
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_byte(0x42); // 只有1字节，但LoadK需要4字节操作数
        // decode 应返回 None
        assert!(Instruction::decode(&chunk, 0).is_none());
    }

    #[test]
    fn test_disassemble_iss_opcode() {
        let mut chunk = Chunk::new();
        // 写入 GetGlobalCached (code=50, size=7, operands=[Reg, U16, U16])
        chunk.write_opcode(Opcode::GetGlobalCached);
        chunk.write_u16(0); // dest
        chunk.write_u16(5); // global_idx
        chunk.write_u16(1); // version
        chunk.emit(Instruction::Halt);

        let disasm = chunk.disassemble();
        assert!(disasm.contains("GetGlobalCached"), "ISS opcode should show name, not UNKNOWN");
        assert!(!disasm.contains("UNKNOWN"), "no UNKNOWN should appear for known ISS opcodes");
    }

    #[test]
    fn test_disassemble_spill_opcode() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::SpillLoad);
        chunk.write_u16(0); // dst
        chunk.write_u16(3); // slot
        chunk.emit(Instruction::Halt);

        let disasm = chunk.disassemble();
        assert!(disasm.contains("SpillLoad"), "Spill opcode should show name");
    }

    // ── Spill 指令 decode_spill + disasm 格式测试（方案 A）──────────────
    // SpillLoad/SpillStore 是 extra_dispatch 特化指令，无 Instruction 变体，
    // Instruction::decode 对其返回 None（与 GetGlobalCached 同类先例）。
    // 这里测试独立的 Chunk::decode_spill 方法和 disassemble 专用格式。

    /// SpillLoad roundtrip: write_opcode + write_u16×2 → decode_spill 验证 reg/slot
    #[test]
    fn test_spill_load_decode_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::SpillLoad);
        chunk.write_u16(5); // reg
        chunk.write_u16(12); // slot
        let opcode_byte = chunk.code()[0];
        let operand_bytes = &chunk.code()[1..];
        let decoded = Chunk::decode_spill(opcode_byte, operand_bytes);
        assert!(decoded.is_some(), "SpillLoad decode should succeed");
        let (reg, slot, consumed) = decoded.unwrap();
        assert_eq!(reg, 5, "reg should be 5");
        assert_eq!(slot, 12, "slot should be 12");
        assert_eq!(consumed, 4, "consumed operand bytes should be 4");
    }

    /// SpillStore roundtrip: write_opcode + write_u16×2 → decode_spill 验证 reg/slot
    #[test]
    fn test_spill_store_decode_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::SpillStore);
        chunk.write_u16(10); // reg
        chunk.write_u16(255); // slot
        let opcode_byte = chunk.code()[0];
        let operand_bytes = &chunk.code()[1..];
        let decoded = Chunk::decode_spill(opcode_byte, operand_bytes);
        assert!(decoded.is_some(), "SpillStore decode should succeed");
        let (reg, slot, consumed) = decoded.unwrap();
        assert_eq!(reg, 10, "reg should be 10");
        assert_eq!(slot, 255, "slot should be 255");
        assert_eq!(consumed, 4, "consumed operand bytes should be 4");
    }

    /// SpillLoad disasm 格式: 输出包含 `SpillLoad R5, [12]`
    #[test]
    fn test_spill_load_disasm_format() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::SpillLoad);
        chunk.write_u16(5); // reg
        chunk.write_u16(12); // slot
        chunk.emit(Instruction::Halt);
        let disasm = chunk.disassemble();
        assert!(disasm.contains("SpillLoad"), "should contain opcode name");
        assert!(disasm.contains("R5, [12]"), "should contain R5, [12] operand format");
        assert!(
            disasm.contains("SpillLoad R5, [12]"),
            "should contain full SpillLoad R5, [12] format"
        );
    }

    /// SpillStore disasm 格式: 输出包含 `SpillStore R7, [3]`
    #[test]
    fn test_spill_store_disasm_format() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::SpillStore);
        chunk.write_u16(7); // reg
        chunk.write_u16(3); // slot
        chunk.emit(Instruction::Halt);
        let disasm = chunk.disassemble();
        assert!(disasm.contains("SpillStore"), "should contain opcode name");
        assert!(disasm.contains("R7, [3]"), "should contain R7, [3] operand format");
        assert!(
            disasm.contains("SpillStore R7, [3]"),
            "should contain full SpillStore R7, [3] format"
        );
    }

    /// decode_spill 边界条件: 非法 opcode 和截断字节流返回 None
    #[test]
    fn test_decode_spill_rejects_invalid_input() {
        // 非法 opcode（非 SpillLoad/SpillStore）
        assert!(Chunk::decode_spill(0, &[0, 0, 0, 0]).is_none(), "opcode 0 should be rejected");
        assert!(Chunk::decode_spill(99, &[0, 0, 0, 0]).is_none(), "opcode 99 should be rejected");
        // 截断字节流（不足 4 字节操作数）
        assert!(
            Chunk::decode_spill(Opcode::SpillLoad as u8, &[]).is_none(),
            "empty bytes should be rejected"
        );
        assert!(
            Chunk::decode_spill(Opcode::SpillLoad as u8, &[0, 0]).is_none(),
            "2-byte bytes should be rejected"
        );
        assert!(
            Chunk::decode_spill(Opcode::SpillStore as u8, &[0, 0, 0]).is_none(),
            "3-byte bytes should be rejected"
        );
        // 合法输入
        let ok = Chunk::decode_spill(Opcode::SpillLoad as u8, &[5, 0, 12, 0]);
        assert!(ok.is_some(), "valid SpillLoad input should decode");
        assert_eq!(ok.unwrap(), (5, 12, 4));
        // SpillStore 也应通过
        let ok2 = Chunk::decode_spill(Opcode::SpillStore as u8, &[7, 0, 3, 0]);
        assert!(ok2.is_some(), "valid SpillStore input should decode");
        assert_eq!(ok2.unwrap(), (7, 3, 4));
    }
}
