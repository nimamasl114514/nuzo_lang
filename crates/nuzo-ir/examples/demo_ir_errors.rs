//! IR 错误体系演示 — 展示多行 rustc 风格诊断输出
//!
//! 运行：`cargo run -p nuzo_ir --example demo_ir_errors`
//!
//! 展示内容：
//! 1. IrBuildError 各变体的多行 Display 格式
//! 2. InternalError（替代 panic）的诊断信息
//! 3. IrValidationError 的结构化诊断
//! 4. ValidationWarning 的警告格式
//! 5. IrErrorCode trait 提供的 error_code/severity/category/help

use nuzo_core::SourceLocation;
use nuzo_ir::{
    IrBuildError, IrErrorCategory, IrErrorCode, IrErrorSeverity, IrValidationError,
    ValidationWarning,
};

fn separator(title: &str) {
    println!("\n{}", "═".repeat(70));
    println!("  {}", title);
    println!("{}", "═".repeat(70));
}

fn print_error<E: IrErrorCode + std::fmt::Display>(label: &str, err: &E) {
    println!("\n【{}】", label);
    println!("{}", err);
    println!("─ metadata ─");
    println!("  error_code: {}", err.error_code());
    println!("  severity:   {} ({})", err.severity(), err.severity().as_str());
    println!("  category:   {} ({})", err.category(), err.category().as_str());
    println!("  help:       {:?}", err.help().map(|s| s.chars().take(60).collect::<String>()));
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════════════╗");
    println!("║     Nuzo IR 错误体系增强演示 — 多行 rustc 风格诊断输出          ║");
    println!("╚══════════════════════════════════════════════════════════════════╝");

    let loc = SourceLocation {
        file: "demo.nuzo".to_string(),
        line: 10,
        column: 5,
        source_line: Some("x + 1".to_string()),
        function_name: None,
    };

    // ========== 1. IrBuildError 普通变体 ==========
    separator("1. IrBuildError 普通变体（IRB001-IRB010）");

    print_error(
        "IRB001 UndefinedVariable",
        &IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() },
    );

    print_error(
        "IRB002 BreakOutsideLoop",
        &IrBuildError::BreakOutsideLoop { location: loc.clone() },
    );

    print_error(
        "IRB005 TooManyLocals",
        &IrBuildError::TooManyLocals { count: 300, max: 256, location: loc.clone() },
    );

    print_error(
        "IRB008 UnexpectedExpr",
        &IrBuildError::UnexpectedExpr {
            expr_kind: "While".to_string(),
            context: "build_binary_expr",
            location: loc.clone(),
        },
    );

    // ========== 2. InternalError（替代 panic）==========
    separator("2. InternalError（替代原 panic!，IRB009）");

    print_error(
        "IRB009 InternalError - current_function_id 越界",
        &IrBuildError::InternalError {
            what: "current_function_id out of range".to_string(),
            context:
                "fn_id=5, functions.len()=3; indicates scope management bug in build_closure_expr"
                    .to_string(),
            location: SourceLocation::default(),
            hint: "Check build_closure_expr/build_fn_expr scope save/restore logic".to_string(),
        },
    );

    print_error(
        "IRB009 InternalError - current_block_id 越界",
        &IrBuildError::InternalError {
            what: "current_block_id out of range".to_string(),
            context: "block_id=99, blocks.len()=5; indicates block management bug".to_string(),
            location: SourceLocation::default(),
            hint: "Check switch_to_block/new_block block ID consistency".to_string(),
        },
    );

    // ========== 3. IrValidationError ==========
    separator("3. IrValidationError（IRV001-IRV011）");

    print_error(
        "IRV001 UndefinedValueRef",
        &IrValidationError::UndefinedValueRef {
            value_ref: 42,
            context: "instruction Binary { op: Add, left: v41, right: v42 } in bb0:5".to_string(),
        },
    );

    print_error(
        "IRV002 BlockMissingTerminator",
        &IrValidationError::BlockMissingTerminator { block_id: 3 },
    );

    print_error("IRV007 MainFunctionEmpty", &IrValidationError::MainFunctionEmpty {
        function_index: 0,
        hint: "Check build_closure_expr scope management - top-level statements not emitted to main".to_string(),
    });

    print_error(
        "IRV010 InvalidClosureReference",
        &IrValidationError::InvalidClosureReference {
            instruction_index: 5,
            referenced_function: 99,
            total_functions: 3,
            hint: "Scope bug suspected - check function registration order".to_string(),
        },
    );

    // ========== 4. ValidationWarning ==========
    separator("4. ValidationWarning（IRW001）");

    print_error(
        "IRW001 SuspiciousInstructionInFunction",
        &ValidationWarning::SuspiciousInstructionInFunction {
            function_index: 2,
            instruction_index: 7,
            opcode: "Closure".to_string(),
            hint: "Check scope restore after closure build".to_string(),
        },
    );

    // ========== 5. to_single_line() 旧格式对比 ==========
    separator("5. to_single_line() 旧格式对比（向后兼容）");

    let err = IrBuildError::UndefinedVariable { name: "x".to_string(), location: loc.clone() };
    println!("\n多行 Display:");
    println!("{}", err);
    println!("\n单行 to_single_line():");
    println!("{}", err.to_single_line());

    let err2 = IrValidationError::UndefinedValueRef {
        value_ref: 42,
        context: "instruction Add bb0:5".to_string(),
    };
    println!("\n多行 Display:");
    println!("{}", err2);
    println!("\n单行 to_single_line():");
    println!("{}", err2.to_single_line());

    // ========== 6. 错误代码体系总览 ==========
    separator("6. 错误代码体系总览");

    println!(
        "
┌──────────┬─────────────────────────────────┬───────────┬────────────┐
│ 代码     │ 变体                            │ 严重级别  │ 分类       │
├──────────┼─────────────────────────────────┼───────────┼────────────┤
│ IRB001   │ UndefinedVariable               │ Error     │ Semantic   │
│ IRB002   │ BreakOutsideLoop                │ Error     │ Semantic   │
│ IRB003   │ ContinueOutsideLoop             │ Error     │ Semantic   │
│ IRB004   │ ReturnOutsideFunction           │ Error     │ Semantic   │
│ IRB005   │ TooManyLocals                   │ Error     │ Limit      │
│ IRB006   │ TooManyArguments                │ Error     │ Limit      │
│ IRB007   │ ConstantPoolOverflow            │ Error     │ Limit      │
│ IRB008   │ UnexpectedExpr                  │ Error     │ Internal   │
│ IRB009   │ InternalError (替代 panic)      │ Error     │ Internal   │
│ IRB010   │ Error (通用)                    │ Error     │ Semantic   │
├──────────┼─────────────────────────────────┼───────────┼────────────┤
│ IRV001   │ UndefinedValueRef               │ Error     │ Structural │
│ IRV002   │ BlockMissingTerminator          │ Error     │ Structural │
│ IRV003   │ DisconnectedBlock               │ Error     │ Structural │
│ IRV004   │ InvalidBlockId                  │ Error     │ Structural │
│ IRV005   │ UndefinedFunction               │ Error     │ Structural │
│ IRV006   │ Generic                         │ Error     │ Other      │
│ IRV007   │ MainFunctionEmpty               │ Error     │ Scope      │
│ IRV008   │ CaptureInMainFunction           │ Error     │ Scope      │
│ IRV009   │ ArgumentInMainFunction          │ Error     │ Scope      │
│ IRV010   │ InvalidClosureReference         │ Error     │ Scope      │
│ IRV011   │ InvalidBlockInstructionRef      │ Error     │ Scope      │
├──────────┼─────────────────────────────────┼───────────┼────────────┤
│ IRW001   │ SuspiciousInstructionInFunction │ Warning   │ Scope      │
└──────────┴─────────────────────────────────┴───────────┴────────────┘
"
    );

    // ========== 7. IrErrorSeverity / IrErrorCategory ==========
    separator("7. IrErrorSeverity / IrErrorCategory 枚举");

    println!("IrErrorSeverity:");
    for s in [IrErrorSeverity::Error, IrErrorSeverity::Warning, IrErrorSeverity::Info] {
        println!("  {:?} -> Display=\"{}\", as_str=\"{}\"", s, s, s.as_str());
    }

    println!("\nIrErrorCategory:");
    for c in [
        IrErrorCategory::Semantic,
        IrErrorCategory::Structural,
        IrErrorCategory::Limit,
        IrErrorCategory::Internal,
        IrErrorCategory::Scope,
        IrErrorCategory::Other,
    ] {
        println!("  {:?} -> Display=\"{}\", as_str=\"{}\"", c, c, c.as_str());
    }

    println!("\n✅ 演示完成 - IR 错误体系增强已生效");
}
