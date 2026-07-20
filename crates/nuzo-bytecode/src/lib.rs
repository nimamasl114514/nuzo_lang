//! # Nuzo Bytecode — Nuzo 字节码与指令系统核心库
//!
//! **层级**: L3（语言核心层 / 字节码层）—— 定义 Nuzo 指令集、字节码容器与词法作用域，是编译器后端与虚拟机执行引擎之间的桥梁。
//!
//! **主要入口**: [`Instruction`], [`Opcode`], [`Chunk`], [`Reg`], [`ConstIdx`], [`Scope`], [`load_chunk`], [`save_chunk`]
//!
//! **Crate 定位**: Nuzo 编程语言的字节码系统核心库
//!
//! ## 架构概览
//! 本 crate 是 Nuzo 编译器后端与虚拟机执行引擎之间的**桥梁层**，
//! 负责定义指令集、管理字节码编码/解码、以及编译器辅助数据结构。
//!
//! ## 核心职责
//!
//! ### 1. 指令集定义 (`opcode` 模块)
//! - **44 种高层指令** (`Instruction` 枚举, + 3 个运行时特化 opcode = 47): 面向编译器开发者的类型安全 IR
//! - **底层操作码** (`Opcode` 枚举): 由过程宏生成，面向虚拟机执行器
//! - **强类型操作数**: `Reg`, `ConstIdx`, `Offset`, `CaptureIdx`, `U8`, `U16`
//!   - 使用 Newtype 模式实现零成本抽象，编译期防止操作数混用
//!
//! ### 2. 字节码容器 (`Chunk`)
//! - **指令序列**: `Arc<Vec<u8>>` — 支持零拷贝共享 (COW 语义)
//! - **常量池**: `Arc<Vec<Value>>` — 存储字面量值（数字、字符串等）
//! - **调试信息**: 源文件名、源码行、IP→行号映射
//! - **反汇编器**: 人类可读的字节码输出，支持源码级调试
//!
//! ### 3. 编译器辅助 (`constants` + `scope` 模块)
//! - **常量定义**: 字节码格式层面的数值常量（指令大小、操作数大小等）
//! - **作用域管理**: 词法作用域实现，支持块级作用域嵌套和变量遮蔽
//!
//! ## 设计原则
//!
//! ### 单一数据源 (SSOT)
//! `Instruction` 枚举是唯一的数据源，所有 Opcode 映射、编码、解码逻辑均围绕它展开。
//! 新增指令时必须同步更新 9 个位置（详见 opcode.rs 模块文档）。
//!
//! ### 内存安全
//! - 使用 `Option<T>` 返回值处理越界访问（而非 panic）
//! - 使用 `assert!` 防止不可恢复的编程错误（如常量池溢出）
//! - 使用 `Arc<T>` 实现安全的共享可变状态（COW 模式）
//!
//! ### 性能优化
//! - **紧凑编码**: 变长指令格式（1-8 字节），最小化内存占用
//! - **小端序**: 与主流 CPU 对齐，避免字节序转换开销
//! - **批量发射**: `emit()` 方法内联编码逻辑，减少函数调用开销
//!
//! ## 典型使用流程
//!
//! ```ignore
//! use nuzo_bytecode::*;
//!
//! // 1. 创建字节码块
//! let mut chunk = Chunk::new();
//!
//! // 2. 添加常量到常量池
//! let const_42 = chunk.add_constant(Value::from_number(42.0));
//!
//! // 3. 发射指令
//! chunk.emit(Instruction::LoadK {
//!     dest: Reg(0),
//!     const_idx: ConstIdx(const_42 as u16)
//! });
//! chunk.emit(Instruction::Print { reg: Reg(0) });
//! chunk.emit(Instruction::Halt);
//!
//! // 4. 反汇编输出（用于调试）
//! println!("{}", chunk.disassemble());
//!
//! // 5. 虚拟机执行（chunk.code 和 chunk.constants）
//! ```
//!
//! ## 模块结构
//!
//! ```text
//! nuzo_bytecode/
//! ├── lib.rs          # Crate 入口，统一导出 API
//! ├── opcode.rs       # 指令集定义 + Chunk 容器 (核心，~1300 行)
//! ├── constants.rs    # 字节码格式常量 (~30 行)
//! └── scope.rs        # 词法作用域管理 (~190 行)
//! ```
//!
//! ## 依赖关系
//! - **nuzo_values**: Value 类型系统（动态类型值表示）
//! - **nuzo_core**: SourceLocation, CAPTURE_OUTER_FLAG 等核心类型
//! - **nuzo_opcode**: OperandKind, DispatchKind, define_opcodes! 过程宏
//!
//! ## 向后兼容性保证
//! - Opcode 数值分配一旦固定不得更改（影响字节码兼容性）
//! - 新增指令必须使用保留槽位或追加到末尾
//! - 公开 API 的移除需经历 major 版本升级

#![allow(clippy::result_large_err)]

// Crate 元数据——使用外层属性形式，因为 `#![inner_attr]` 在 stable Rust 不稳定
#[nuzo_proc::crate_meta(layer = 3, description = "字节码与 Instruction 枚举", entry_type = "Chunk")]
const _NUZO_CRATE_META_ANCHOR: () = ();

// 核心字节码系统 (SSOT) — 指令集定义、编码/解码、反汇编
pub mod opcode;

// 编译器/VM 辅助模块 — 常量定义和作用域管理
pub mod constants;
pub mod scope;

// 字节码文件序列化/反序列化 — 负责带版本校验的字节码加载
pub mod serialization;

// 统一导出 — 显式列出每个模块的公共 API，避免通配符重导出
//
// 设计原则：显式导出让 crate 的公共 API 表面积一目了然，
// 新增/删除导出项时必须在此处同步更新，防止意外泄露内部实现。

// ── opcode 模块导出 ──────────────────────────────────────────────
// 强类型操作数 (Newtypes)
pub use opcode::{CaptureIdx, ConstIdx, Offset, Reg, U8, U16};
// 闭包捕获源枚举
pub use opcode::CapturedSource;
// 高层指令枚举 (面向编译器开发者的类型安全 IR)
pub use opcode::Instruction;
// 底层操作码枚举 (由 define_opcodes! 宏生成，面向虚拟机执行器)
pub use opcode::Opcode;
// 字节码容器 (管理指令序列、常量池、调试信息)
pub use opcode::Chunk;
pub use opcode::ChunkError;
// 调试信息类型 (从 nuzo_values 重导出)
pub use opcode::DebugInfo;
// 指令总数常量 (由 #[derive(OpcodeSync)] 自动生成，非 constants 模块)
pub use opcode::INSTRUCTION_COUNT;

// ── constants 模块导出 ───────────────────────────────────────────
// 字节码格式常量 (指令大小、操作数大小等)
pub use constants::I16_SIZE;
pub use constants::MAX_INSTRUCTION_SIZE;
pub use constants::MIN_INSTRUCTION_SIZE;
pub use constants::OPCODE_SIZE;
pub use constants::U8_SIZE;
pub use constants::U16_SIZE;

// ── scope 模块导出 ───────────────────────────────────────────────
// 词法作用域管理 (变量定义、解析、遮蔽)
pub use scope::Scope;
// 全局作用域管理 (程序级全局变量注册表)
pub use scope::GlobalScope;
// 变量来源枚举 (区分局部变量和全局变量)
pub use scope::ScopeKind;

// ── serialization 模块导出 ───────────────────────────────────────
// 字节码文件格式常量 (magic/version)
pub use serialization::{BYTECODE_MAGIC, BYTECODE_VERSION};
// 带版本校验的字节码加载
pub use serialization::load_chunk;
// 字节码保存
pub use serialization::save_chunk;
// 未知 opcode 诊断构造
pub use serialization::diagnose_opcode_byte;

// 从 nuzo_opcode re-export 共享类型，保持 API 兼容性
//
// 这样用户只需 `use nuzo_bytecode::*` 即可获得所有必需类型，
// 无需手动依赖 nuzo_opcode crate。
pub use nuzo_opcode::{DispatchKind, OperandKind};
