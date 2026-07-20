use super::{ErrorCategory, ErrorSeverity, ExecutionContext, StackFrameInfo, StructuredSuggestion};
use crate::classifier::ErrorClassifier;
use nuzo_core::{NuzoError, NuzoErrorKind, VmDiagnosis};
use serde::Serialize;
use std::fmt;

// ============================================================================
// Diagnostic Error (Enhanced Error with Context)
// ============================================================================

/// 诊断性错误（包含丰富上下文信息）
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticError {
    /// 错误ID（唯一标识）
    pub id: usize,

    /// 原始运行时错误
    pub error: NuzoError,

    /// 严重程度
    pub severity: ErrorSeverity,

    /// 错误类别
    pub category: ErrorCategory,

    /// 执行上下文快照
    pub context: ExecutionContext,

    /// 完整调用栈
    pub call_stack: Vec<StackFrameInfo>,

    /// 发生时间（指令计数）
    pub instruction_count: usize,

    /// 自动生成的修复建议
    pub fix_suggestions: Vec<String>,

    /// 结构化修复建议（包含可选替换代码与源码位置）
    #[serde(default)]
    pub structured_suggestions: Vec<StructuredSuggestion>,

    // ===== 新增：NuzoError 支持字段 =====
    /// NuzoError（如果错误来源于 NuzoError，此字段为 Some）
    ///
    /// 当使用 `collect_nuzo_error()` 或 `from_nuzo_error()` 创建 DiagnosticError 时，
    /// 此字段会包含完整的 NuzoError 信息。
    /// 对于通过旧 API `collect_error()` 创建的错误，此字段为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nuzo_error: Option<NuzoError>,

    /// VM 诊断报告（仅对 InternalError 有效）
    ///
    /// 当 NuzoError::Internal 发生时，VM 可以生成详细的诊断报告，
    /// 包括反汇编代码、寄存器快照、根因分析等。
    /// 此字段存储该诊断报告（如果可用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<VmDiagnosis>,
}

impl DiagnosticError {
    /// 创建新的诊断错误
    ///
    /// 此方法保持向后兼容，现有代码仍然可以正常工作。
    /// 新代码建议使用 [`from_nuzo_error()`] 以获得更丰富的诊断信息。
    pub fn new(
        id: usize,
        error: NuzoError,
        context: ExecutionContext,
        call_stack: Vec<StackFrameInfo>,
        instruction_count: usize,
    ) -> Self {
        let (severity, category) = ErrorClassifier::classify(&error);
        let suggestions = ErrorClassifier::generate_fix_suggestion(&error);
        let structured = ErrorClassifier::generate_structured_suggestions(&error);

        DiagnosticError {
            id,
            error,
            severity,
            category,
            context,
            call_stack,
            instruction_count,
            fix_suggestions: suggestions,
            structured_suggestions: structured,
            nuzo_error: None,
            diagnosis: None,
        }
    }

    /// 从 NuzoError 创建诊断错误（新 API）
    ///
    /// 这是推荐的创建方式，支持完整的 NuzoError 诊断功能：
    /// - 自动使用 ErrorClassifier 进行分类
    /// - 支持附加 VmDiagnosis 诊断报告（对 InternalError）
    /// - 生成更精确的修复建议
    ///
    /// # Arguments
    ///
    /// * `id` - 错误唯一标识符
    /// * `nuzo_error` - NuzoError 错误实例
    /// * `context` - 执行上下文快照
    /// * `call_stack` - 完整调用栈
    /// * `instruction_count` - 发生时的指令计数
    /// * `diagnosis` - 可选的 VM 诊断报告（通常由 VM::diagnose_internal_error() 生成）
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let diagnostic = DiagnosticError::from_nuzo_error(
    ///     0,
    ///     nuzo_error,
    ///     context,
    ///     call_stack,
    ///     instruction_count,
    ///     Some(diagnosis),  // 对 InternalError 提供诊断报告
    /// );
    /// ```
    pub fn from_nuzo_error(
        id: usize,
        nuzo_error: NuzoError,
        context: ExecutionContext,
        call_stack: Vec<StackFrameInfo>,
        instruction_count: usize,
        diagnosis: Option<VmDiagnosis>,
    ) -> Self {
        // 使用 ErrorClassifier 进行分类
        let (severity, category) = ErrorClassifier::classify(&nuzo_error);

        // 生成修复建议
        let suggestions = ErrorClassifier::generate_fix_suggestion(&nuzo_error);
        let structured = ErrorClassifier::generate_structured_suggestions(&nuzo_error);

        // P2 #11 修复：保留原始 NuzoError 用于 backward-compatible error 字段。
        // 旧版对 InternalError 使用 `NuzoError::assert_failed("internal error")` 占位符，
        // 丢失了具体的 InternalError 变体信息（NoChunkLoaded / RegisterOutOfBounds 等）
        // 和 VmDiagnosis 关联，破坏 source 链。
        // 现在统一使用 `nuzo_error.clone()`，确保：
        // - `error` 字段保留原始错误信息（含 InternalError 变体）
        // - `nuzo_error` 字段也保留同一份原始错误
        // - Display / renderer 已优先使用 `nuzo_error` 字段，行为不变
        // - 任何读取 `error` 字段的旧代码也能看到正确的错误类型
        let runtime_error = nuzo_error.clone();

        DiagnosticError {
            id,
            error: runtime_error,
            severity,
            category,
            context,
            call_stack,
            instruction_count,
            fix_suggestions: suggestions,
            structured_suggestions: structured,
            nuzo_error: Some(nuzo_error),
            diagnosis,
        }
    }

    /// 检查此错误是否来源于 NuzoError
    ///
    /// # Returns
    ///
    /// `true` 如果此错误是通过 `from_nuzo_error()` 或 `collect_nuzo_error()` 创建的
    pub fn is_nuzo_error(&self) -> bool {
        self.nuzo_error.is_some()
    }

    /// 检查此错误是否为 InternalError（运行时内部 bug）
    ///
    /// # Returns
    ///
    /// `true` 如果此错误是 InternalError 类型
    pub fn is_internal_error(&self) -> bool {
        self.nuzo_error.as_ref().is_some_and(|e| matches!(e.kind, NuzoErrorKind::Internal(_, _)))
    }

    /// 获取 NuzoError 引用（如果存在）
    ///
    /// # Returns
    ///
    /// `Some(&NuzoError)` 如果此错误来源于 NuzoError，否则 `None`
    pub fn as_nuzo_error(&self) -> Option<&NuzoError> {
        self.nuzo_error.as_ref()
    }

    /// 获取 VM 诊断报告引用（如果存在）
    ///
    /// # Returns
    ///
    /// `Some(&VmDiagnosis)` 如果有可用的诊断报告，否则 `None`
    pub fn diagnosis(&self) -> Option<&VmDiagnosis> {
        self.diagnosis.as_ref()
    }
}

impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "\n{}", "═".repeat(70))?;
        writeln!(f, "❌ 错误 #{} ({})", self.id, self.severity)?;
        writeln!(f, "   类别: {}", self.category)?;

        // 显示错误消息：优先使用 NuzoError，否则使用 error 字段
        if let Some(ref nuzo_err) = self.nuzo_error {
            writeln!(f, "   消息: {}", nuzo_err)?;

            // 标记是否为内部错误
            if self.is_internal_error() {
                writeln!(f, "   ⚠️  类型: 内部错误 (运行时 bug)")?;
            }
        } else {
            writeln!(f, "   消息: {}", self.error)?;
        }

        writeln!(f, "   时间: 第 {} 条指令", self.instruction_count)?;
        writeln!(f, "{}", "─".repeat(70))?;

        // 执行上下文
        writeln!(f, "📋 执行上下文:")?;
        write!(f, "{}", self.context)?;

        // 调用栈
        if !self.call_stack.is_empty() {
            writeln!(f, "🔄 调用栈 (从最新到最旧):")?;
            for (i, frame) in self.call_stack.iter().rev().enumerate() {
                writeln!(f, "  #{}  {}", i, frame)?;
            }
        }

        // VM 诊断报告（仅对 InternalError 显示）
        if let Some(ref diag) = self.diagnosis {
            writeln!(f, "\n🔬 VM 诊断报告:")?;
            write!(f, "{}", diag)?;
        }

        // 修复建议
        if !self.fix_suggestions.is_empty() {
            writeln!(f, "\n💡 修复建议:")?;
            for (i, suggestion) in self.fix_suggestions.iter().enumerate() {
                writeln!(f, "  {}. {}", i + 1, suggestion)?;
            }
        }

        writeln!(f, "{}", "═".repeat(70))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_core::InternalError;

    /// P2 #11 回归测试：from_nuzo_error 对 InternalError 不再使用占位符。
    ///
    /// 旧版行为（BUG）：InternalError 被替换为 `NuzoError::assert_failed("internal error")`，
    /// 导致 `error` 字段丢失原始 InternalError 变体信息（如 NoChunkLoaded）。
    ///
    /// 修复后行为：`error` 字段保留原始 NuzoError（含 InternalError 变体），
    /// source 链完整保留。
    #[test]
    fn test_from_nuzo_error_preserves_internal_error_in_error_field() {
        let nuzo_err: NuzoError = InternalError::NoChunkLoaded.into();
        let diag = DiagnosticError::from_nuzo_error(
            1,
            nuzo_err,
            ExecutionContext::new(42, None, 3),
            Vec::new(),
            100,
            None,
        );

        // nuzo_error 字段应保留原始 InternalError
        assert!(diag.is_nuzo_error(), "is_nuzo_error must be true");
        assert!(diag.is_internal_error(), "is_internal_error must be true");
        let nuzo_err_ref = diag.as_nuzo_error().expect("nuzo_error must be Some");
        assert!(
            matches!(
                &nuzo_err_ref.kind,
                NuzoErrorKind::Internal(InternalError::NoChunkLoaded, None)
            ),
            "nuzo_error field must preserve InternalError::NoChunkLoaded"
        );

        // P2 #11 核心断言：error 字段也必须保留原始 InternalError（不再被替换为 AssertFailed）
        assert!(
            matches!(&diag.error.kind, NuzoErrorKind::Internal(InternalError::NoChunkLoaded, None)),
            "error field must preserve InternalError::NoChunkLoaded (got {:?})",
            diag.error.kind
        );
        assert!(
            !matches!(&diag.error.kind, NuzoErrorKind::AssertFailed { .. }),
            "error field must NOT be replaced with AssertFailed placeholder"
        );
    }

    /// P2 #11 回归测试：from_nuzo_error 对程序级错误（非 InternalError）行为不变。
    #[test]
    fn test_from_nuzo_error_preserves_program_error() {
        let nuzo_err = NuzoError::type_mismatch("string", "number");
        let diag = DiagnosticError::from_nuzo_error(
            2,
            nuzo_err,
            ExecutionContext::new(10, None, 1),
            Vec::new(),
            50,
            None,
        );

        // 程序级错误：error 与 nuzo_error 字段都应保留原始 TypeMismatch
        assert!(diag.is_nuzo_error());
        assert!(!diag.is_internal_error(), "TypeMismatch is not internal error");
        assert!(
            matches!(&diag.error.kind, NuzoErrorKind::TypeMismatch { .. }),
            "error field must preserve TypeMismatch"
        );
    }

    /// P2 #11 回归测试：diagnosis 字段正确存储。
    #[test]
    fn test_from_nuzo_error_preserves_diagnosis_field() {
        // 构造带 VmDiagnosis 的 InternalError
        let nuzo_err: NuzoError = InternalError::StackOverflow { depth: 100, max_depth: 64 }.into();
        let diag = DiagnosticError::from_nuzo_error(
            3,
            nuzo_err,
            ExecutionContext::new(0, None, 100),
            Vec::new(),
            0,
            None, // VmDiagnosis 暂用 None（构造复杂，此处仅验证字段透传）
        );

        // diagnosis 字段为 None（输入即 None）
        assert!(diag.diagnosis().is_none(), "diagnosis field must match input");

        // error 字段保留 StackOverflow 变体
        assert!(
            matches!(
                &diag.error.kind,
                NuzoErrorKind::Internal(
                    InternalError::StackOverflow { depth: 100, max_depth: 64 },
                    None
                )
            ),
            "error field must preserve StackOverflow variant with correct depth/max_depth"
        );
    }
}
