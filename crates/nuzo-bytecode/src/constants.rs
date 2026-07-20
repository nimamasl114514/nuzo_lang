//! # 字节码编码相关常量定义
//!
//! 本模块定义 Nuzo 字节码格式层面的**数值常量**，
//! 用于指令大小计算、内存分配、边界检查等场景。
//!
//! ## 设计原则
//! ### 职责分离
//! - **本模块**: 字节码格式常量（指令长度、操作数大小）
//! - `nuzo_core::constants`: 运行时常量（VM 栈大小、GC 参数、编译器限制）
//!
//! 这种分离确保字节码格式独立于具体实现配置，
//! 便于跨版本兼容和序列化/反序列化。
//!
//! ### 常量来源
//! 所有值均从 `define_opcodes!` 宏中的 `size` 声明推导得出：
//! - `OPCODE_SIZE`: 固定为 1（所有操作码都是单字节）
//! - `U8_SIZE` / `U16_SIZE` / `I16_SIZE`: 操作数的字节数
//! - `MIN_INSTRUCTION_SIZE`: 最短指令 (`Halt`, 仅操作码)
//! - `MAX_INSTRUCTION_SIZE`: 最长指令 (`RangeNew`, 1+3*2+1=8 字节)
//!
//! ## 使用场景
//! ```ignore
//! // 场景1: 预分配缓冲区
//! let buffer_size = MAX_INSTRUCTION_SIZE * estimated_instruction_count;
//!
//! // 场景2: 边界检查
//! assert!(offset + MIN_INSTRUCTION_SIZE <= chunk.len());
//!
//! // 场景3: 统计分析
//! println!("Average instruction size: {} bytes", total_bytes / INSTRUCTION_COUNT);
//! ```

// INSTRUCTION_COUNT 已迁移至 opcode.rs，由 #[derive(OpcodeSync)] 自动生成。
// 见 `pub use opcode::INSTRUCTION_COUNT;` in lib.rs。
// 保留此处注释作为历史文档：原手写值为 46（43 个 Instruction 变体 + 3 个 extra_dispatch）。

/// 单字节操作码的固定大小 (1 字节)
///
/// 所有 Nuzo 指令的操作码都编码为单个 u8 字节，
/// 这限制了最大指令数为 256 种（当前使用 47 个，剩余 209 个可用）。
///
/// # 设计权衡
/// - **优点**: 解码速度快（单次数组访问），指令流紧凑
/// - **缺点**: 指令种类受限于 256（对于 DSL 足够，通用 VM 可能不足）
///
/// # 使用场景
/// - 计算指令起始偏移: `next_ip = current_ip + OPCODE_SIZE`
/// - 分配临时解码缓冲区
pub const OPCODE_SIZE: usize = 1;

/// u8 操作数的大小 (1 字节)
///
/// 用于存储 8 位无符号整数，典型用途：
/// - `Call.argc`: 函数参数个数 (0..=255)
/// - `RangeNew.inclusive`: 范围包含标志 (0 或 1)
///
/// # 边界约束
/// 最大值 255，超出需改用 U16 操作数。
pub const U8_SIZE: usize = 1;

/// u16 操作数的大小 (2 字节，小端序)
///
/// 用于存储 16 位无符号整数，典型用途：
/// - 寄存器索引 (`Reg`): 0..=65535
/// - 常量池索引 (`ConstIdx`): 0..=65535
/// - 捕获槽索引 (`CaptureIdx`): 0..=65535
/// - 数组长度 (`ArrayNew.count`): 0..=65535
///
/// # 编码格式
/// 低字节在前，高字节在后：`[low, high]`
pub const U16_SIZE: usize = 2;

/// i16 操作数的大小 (2 字节，小端序)
///
/// 用于存储 16 位有符号整数，典型用途：
/// - 跳转偏移 (`Offset`): -32768..=32767
///
/// # 编码格式
/// 采用补码表示，直接将 i16 位模式作为 u16 写入：
/// ```text
/// i16(-1) → u16(0xFFFF) → [0xFF, 0xFF]
/// i16(100) → u16(0x0064) → [0x64, 0x00]
/// ```
pub const I16_SIZE: usize = 2;

/// 最小可能的指令长度 (仅操作码，无操作数)
///
/// 对应指令: `Halt` (opcode=28, size=1)
///
/// # 使用场景
/// - 边界检查: 确保至少有一个完整指令可读
/// - 内存分配: 最小缓冲区大小估计
pub const MIN_INSTRUCTION_SIZE: usize = OPCODE_SIZE;

/// 最大可能的指令长度 (操作码 + 3个u16 + 1个u8)
///
/// 对应指令: `RangeNew` (opcode=37, size=8)
/// ```text
/// [OPCODE] [dest:u16] [start:u16] [end:u16] [inclusive:u8]
///   1B    +   2B     +   2B    +  2B   +     1B      = 8B
/// ```
///
/// # 使用场景
/// - 预分配解码缓冲区
/// - 跳转目标范围验证（确保不会跳到指令中间）
/// - 反汇编器列宽计算
pub const MAX_INSTRUCTION_SIZE: usize = OPCODE_SIZE + 3 * U16_SIZE + U8_SIZE;

// ── 自动生成: 由 build.rs 从 define_opcodes! 宏调用解析生成 ────────
include!(concat!(env!("OUT_DIR"), "/generated_constants.rs"));
