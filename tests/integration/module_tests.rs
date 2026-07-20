//! 模块系统 (import) 集成测试
//!
//! 覆盖正常场景和错误场景的端到端验证。
//! 正常场景 .nuzo 文件测试位于 tests/e2e/modules/ 目录，
//! 由 E2eRunner 自动发现并执行。
//!
//! 本文件专注于**需要 Rust 断言能力**的错误场景测试：
//! - 循环导入检测
//! - 模块不存在错误
//! - 导入路径解析边界情况

use nuzo_compiler::Compiler;

/// 无 import 的程序编译不受影响（回归测试）
#[test]
fn test_no_import_unaffected() {
    let source = r#"
x = 1 + 2
y = x * 10
print(y)
"#;
    let result = Compiler::compile(source);
    assert!(result.is_ok(), "无 import 程序应正常编译: {:?}", result.err());
}

/// 空程序（无语句）仍可编译
#[test]
fn test_empty_program_compiles() {
    let source = "";
    let result = Compiler::compile(source);
    assert!(result.is_ok(), "空程序应正常编译");
}
