//! # 编译器构建器（Compiler Builder）
//!
//! CompilerBuilder 实现了 Builder 模式，提供灵活的编译器配置方式。
//!
//! ## 设计优势
//!
//! 1. **可扩展性**：新增配置项只需添加链式方法，无需修改已有调用代码
//! 2. **自文档化**：方法名即配置含义，无需记忆参数顺序
//! 3. **编译期验证**：required 字段缺失时在 build() 阶段 panic，而非静默使用默认值

use nuzo_core::DEFAULT_SOURCE_FILE;

/// Builder for constructing compiler configuration.
///
/// Use [`Compiler::builder()`] to create a new builder instance.
/// The `source()` method is **required**; calling `build()` without it will panic.
///
/// # Example
///
/// ```ignore
/// let compiler = Compiler::builder()
///     .source(source_code)
///     .build();
/// ```
pub struct CompilerBuilder {
    pub(crate) source: Option<String>,
    pub(crate) source_file: Option<String>,
}

impl CompilerBuilder {
    /// Set the source code to compile (required).
    ///
    /// This method must be called before [`build()`](Self::build),
    /// otherwise `build()` will panic.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the source file name for debug info (optional, defaults to `DEFAULT_SOURCE_FILE`).
    ///
    /// When compiling nested functions, pass `DEFAULT_FUNCTION_SOURCE_FILE`
    /// to distinguish function-level chunks from top-level ones.
    pub fn source_file(mut self, file: impl Into<String>) -> Self {
        self.source_file = Some(file.into());
        self
    }

    /// Build the compiler configuration and compile the source.
    ///
    /// # Panics
    ///
    /// Panics if [`source()`](Self::source) was not called.
    pub fn build(self) -> nuzo_bytecode::Chunk {
        let source = self.source.expect("Compiler::builder().source() is required");
        let _source_file = self.source_file.as_deref().unwrap_or(DEFAULT_SOURCE_FILE);
        crate::compiler::Compiler::compile(&source)
            .expect("Compiler::builder().build() failed to compile")
    }
}
