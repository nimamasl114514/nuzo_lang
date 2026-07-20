//! # nuzo_codegen — Nuzo 编译期代码生成
//!
//! 包含 builtin、opcode、dispatch 等代码生成逻辑。
//! 从 nuzo_proc_core 拆分出来以减小 proc-macro 依赖链的编译负担。

pub mod builtin_gen;
pub mod dispatch_gen;
pub mod opcode_gen;

#[cfg(feature = "py-test")]
pub mod py_test;
