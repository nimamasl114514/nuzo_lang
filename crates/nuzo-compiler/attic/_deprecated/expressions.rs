//! # 表达式编译模块（Expression Compilation）
//!
//! 本模块实现了所有表达式类型（Expression）的编译逻辑，将 AST 表达式节点转换为字节码指令序列。
//!
//! ## 表达式分类与编译策略
//!
//! Nuzo 语言的表达式可分为以下几大类，每类都有其特定的编译策略：
//!
//! ### 1. 字面量表达式（Literal Expressions）
//! - **数字**（Number）：`compile_number()` → LoadK 指令
//! - **字符串**（String）：`compile_string()` → LoadK 指令
//! - **布尔值**（Bool）：`compile_bool()` → LoadTrue / LoadFalse 指令
//! - **空值**（Nil）：`compile_nil()` → LoadNil 指令
//!
//! ### 2. 变量访问（Variable Access）
//! - **标识符**（Ident）：`compile_ident()` → 三级查找策略
//!   1. 局部变量（Local）→ 直接返回寄存器编号
//!   2. 闭包捕获变量（Captured）→ GetCaptured 指令
//!   3. 全局变量（Global）→ GetGlobal 指令
//!
//! ### 3. 运算表达式（Operation Expressions）
//! - **二元运算**（Binary）：`compile_binary()` → 声明式映射 + 操作数编译 + Opcode 发射
//! - **一元运算**（Unary）：`compile_unary()` → 操作数编译 + Neg/Not 指令
//! - **逻辑运算**（And/Or）：短路求值策略
//!   - And：左操作数为假则返回左，否则返回右
//!   - Or：左操作数为真则返回左，否则返回右
//!
//! ### 4. 函数调用（Function Call）
//! - **调用表达式**（Call）：`compile_call()`
//!   - VM 调用约定：参数必须放在 func_reg+1, func_reg+2, ... 连续位置
//!   - 支持尾调用优化（TCO）：Call 紧跟 Return 时 VM 自动识别
//!
//! ### 5. 成员访问（Member Access）
//! - **索引访问**（Index）：`compile_index()` → GetIndex 指令
//! - **属性访问**（Field）：`compile_field()` → GetProp 指令
//!
//! ### 6. 控制流表达式（Control Flow Expressions）
//! - If/While/Loop/ForIn/Break/Continue/Return 的编译在 `statements.rs` 中实现
//!
//! ### 7. 复合数据类型（Compound Data Types）
//! - 数组/字典/元组/范围等的编译在 `functions.rs` 中实现
//!
//! ## 寄存器管理核心策略
//!
//! ### 表达式级寄存器释放（Expression-Level Register Release）
//!
//! 这是支持大型程序（1000+ 行）的**关键优化**：
//!
//! ```text
//! 编译表达式 a + b * c + d:
//!
//! 步骤 1: 编译 b → r1 (临时)
//! 步骤 2: 编译 c → r2 (临时)
//! 步骤 3: 发射 Mul r3, r1, r2 → r3 (结果)
//!         ↓ 立即释放 r1, r2 (不再需要)
//! 步骤 4: 编译 a → r4 (临时)
//! 步骤 5: 发射 Add r5, r4, r3 → r5 (结果)
//!         ↓ 立即释放 r3, r4 (不再需要)
//! 步骤 6: 编译 d → r6 (临时)
//! 步骤 7: 发射 Add r7, r5, r6 → r7 (最终结果)
//!         ↓ 立即释放 r5, r6 (不再需要)
//!
//! 最终状态: 只有 r7 被占用，其余寄存器已回收复用
//! ```
//!
//! **优势**：
//! - 寄存器使用率从 O(n) 降低到 O(表达式嵌套深度)
//! - 避免长程序中的寄存器耗尽问题
//! - 无需复杂的寄存器分配算法（如线性扫描或图着色）
//!
//! ## 声明式二元运算符映射（Declarative Binary Operator Mapping）
//!
//! 使用宏 `define_binary_op_mapping!` 声明 AST BinaryOp 到 Opcode 的映射关系：
//!
//! # 设计优点
//! 1. **编译期完整性检查**：如果遗漏某个 BinaryOp 变体，match 不 exhaustive，编译器报错
//! 2. **运行时可查询**：生成 BINARY_OP_MAPPING 常量数组供调试器使用
//! 3. **单一数据源**：映射定义只在一处维护，避免同步问题
//!
//! # 映射表（13 项 = BinaryOp 全部变体数）
//! - 算术运算（7）：Add, Sub, Mul, Div, Rem, Mod, Pow
//! - 等值比较（2）：Eq, Neq
//! - 序比较（4）：Lt, Gt, LtEq(→Le), GtEq(→Ge)

use crate::compiler::{CompileError, Compiler};
use crate::macros::*;
use nuzo_bytecode::Opcode;
use nuzo_bytecode::scope::ScopeKind;
use nuzo_core::Value;
use nuzo_frontend::ast;
use nuzo_values::ValueExt;
use std::cmp::Reverse;
use std::sync::Arc;

// ========================================================================
// Constant Folding Helpers (编译期常量折叠辅助函数)
// ========================================================================

/// 尝试在编译期对两个字面量值执行二元运算。
///
/// # 安全策略
/// - 仅折叠 Add/Sub/Mul/Div/Rem 五种算术运算
/// - 不折叠 Pow（浮点精度问题）、Mod（语义不同）
/// - 不折叠比较运算（比较结果虽确定，但语义上不常用于常量表达式）
/// - 除以零时不折叠，保留运行时错误行为
/// - 仅当两个操作数都是数字时才折叠
///
/// # 返回值
/// - `Some(Value)`: 折叠成功，返回计算结果
/// - `None`: 无法折叠，需要生成运行时指令
fn try_fold_binary(op: ast::BinaryOp, left: Value, right: Value) -> Option<Value> {
    // 仅当两个操作数都是数字时才尝试折叠
    if !left.is_number() || !right.is_number() {
        return None;
    }

    let a = left.as_number();
    let b = right.as_number();

    let result = match op {
        ast::BinaryOp::Add => a + b,
        ast::BinaryOp::Sub => a - b,
        ast::BinaryOp::Mul => a * b,
        ast::BinaryOp::Div => {
            // 除以零不折叠，保留运行时错误行为
            if b == 0.0 {
                return None;
            }
            a / b
        }
        ast::BinaryOp::Mod => {
            // 取余除以零不折叠
            if b == 0.0 {
                return None;
            }
            a % b
        }
        // 不折叠 Pow（浮点精度复杂）、Mod（语义不同）、比较运算
        _ => return None,
    };

    Some(Value::from_number(result))
}

/// 尝试在编译期对单个字面量值执行一元运算。
///
/// # 安全策略
/// - Neg: 仅对数字字面量取负
/// - Not: 仅对布尔字面量取反
/// - 不对 nil/字符串等类型折叠
fn try_fold_unary(op: ast::UnaryOp, operand: Value) -> Option<Value> {
    match op {
        ast::UnaryOp::Negate => {
            if operand.is_number() {
                Some(Value::from_number(-operand.as_number()))
            } else {
                None
            }
        }
        ast::UnaryOp::Not => {
            if operand.is_bool() {
                Some(Value::from_bool(!operand.as_bool()))
            } else {
                None
            }
        }
    }
}

/// 从 AST 表达式中提取字面量值（用于常量折叠判断）。
///
/// 仅提取 Number/Bool/Nil 三种可折叠类型，字符串不参与折叠。
///
/// # 返回值
/// - `Some(Value)`: 表达式是可折叠的字面量
/// - `None`: 表达式不是字面量，或是不参与折叠的类型（如字符串）
fn extract_literal_value(expr: &ast::Expr) -> Option<Value> {
    match expr {
        ast::Expr::Number { value, .. } => Some(Value::from_number(*value)),
        ast::Expr::Bool { value, .. } => Some(Value::from_bool(*value)),
        ast::Expr::Nil { .. } => Some(nuzo_values::NIL),
        _ => None, // 字符串、标识符、运算表达式等不参与折叠
    }
}

/// 格式化 Value 为折叠描述中的可读字符串。
///
/// 数字保留整数格式（如 `3` 而非 `3.0`），布尔值用 `true`/`false`，nil 用 `nil`。
fn format_fold_value(value: Value) -> String {
    if value.is_number() {
        let n = value.as_number();
        if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{}", n) }
    } else if value.is_bool() {
        if value.as_bool() { "true".to_string() } else { "false".to_string() }
    } else if value.is_nil() {
        "nil".to_string()
    } else {
        format!("{}", value)
    }
}

/// 将 BinaryOp 转换为人类可读的运算符符号。
///
/// 用于 FoldRecord 的 description 字段，如 "1 + 2 -> 3"。
fn format_binary_op_symbol(op: ast::BinaryOp) -> &'static str {
    match op {
        ast::BinaryOp::Add => "+",
        ast::BinaryOp::Sub => "-",
        ast::BinaryOp::Mul => "*",
        ast::BinaryOp::Div => "/",
        ast::BinaryOp::Mod => "%",
        ast::BinaryOp::Pow => "**",
        ast::BinaryOp::Eq => "==",
        ast::BinaryOp::Neq => "!=",
        ast::BinaryOp::Lt => "<",
        ast::BinaryOp::Gt => ">",
        ast::BinaryOp::LtEq => "<=",
        ast::BinaryOp::GtEq => ">=",
    }
}

/// 将 UnaryOp 转换为人类可读的运算符符号。
///
/// 用于 FoldRecord 的 description 字段，如 "-42" 或 "!true"。
fn format_unary_op_symbol(op: ast::UnaryOp) -> &'static str {
    match op {
        ast::UnaryOp::Negate => "-",
        ast::UnaryOp::Not => "!",
    }
}

// ========================================================================
// Declarative Binary Operator Mapping (AST BinaryOp → Opcode)
// ========================================================================

/// 声明式二元运算符映射宏 — 定义 AST BinaryOp 到 Opcode 的对应关系。
///
/// 此宏同时生成三件产物：
/// 1. **`BINARY_OP_MAPPING`** 常量数组 — 运行时可查询的映射表
/// 2. **`binary_op_to_opcode()`** 查询函数 — 编译期优化的 const match
/// 3. **编译期覆盖检查** — 通过对每个变体做 const 查找验证完整性
///
/// # 用法
///
/// ```rust,ignore
/// define_binary_op_mapping! {
///     Add   => Add,
///     Sub   => Sub,
///     LtEq  => Le,   // AST 名称与 Opcode 名称可不同
///     // ...
/// }
/// ```
///
/// 如果遗漏了某个 `BinaryOp` 变体，`binary_op_to_opcode()` 的 match 将不 exhaustive，
/// 编译器会报错，从而保证映射表与枚举定义同步。
macro_rules! define_binary_op_mapping {
    (
        $($ast_op:ident => $opcode:ident),* $(,)?
    ) => {
        /// 将 AST [`BinaryOp`](ast::BinaryOp) 映射为对应的 [`Opcode`]。
        ///
        /// 由 `define_binary_op_mapping!` 宏生成，使用 const match 实现编译期分发。
        /// 由于宏要求列出所有变体，此函数对所有 `BinaryOp` 都返回 `Some`。
        pub(crate) const fn binary_op_to_opcode(op: ast::BinaryOp) -> Option<Opcode> {
            match op {
                $(ast::BinaryOp::$ast_op => Some(Opcode::$opcode)),*
            }
        }

        /// 编译期覆盖验证：确保每个被声明的映射项都能正确查找。
        ///
        /// 若 `BINARY_OP_MAPPING` 遗漏了某个 `BinaryOp` 变体，
        /// 上面的 match 即非 exhaustive，编译器直接报错。
        #[allow(dead_code)] // 编译期验证块，运行时不可达
        const _: () = {
            let _ = [$(binary_op_to_opcode(ast::BinaryOp::$ast_op)),*];
        };
    };
}

// 调用宏：声明完整的 BinaryOp → Opcode 映射（12 项 = BinaryOp 全部变体数）
// 分类：算术(6) + 等值比较(2) + 序比较(4)
define_binary_op_mapping! {
    // ── 算术运算 (7) ──
    Add  => Add,
    Sub  => Sub,
    Mul  => Mul,
    Div  => Div,
    Mod  => Mod,
    Pow  => Pow,   // ** 幂运算
    // ── 等值比较 (2) ──
    Eq   => Eq,
    Neq  => Neq,
    // ── 序比较 (4) ──
    Lt   => Lt,
    Gt   => Gt,
    LtEq => Le,   // AST: LtEq → Opcode: Le
    GtEq => Ge,   // AST: GtEq → Opcode: Ge

}

impl Compiler {
    // ========================================================================
    // 表达式编译主分发器（Expression Compilation - Main Dispatcher）
    // ========================================================================

    /// 编译表达式节点（Expression Compilation Entry Point）
    ///
    /// 这是所有表达式编译的**统一入口点**，根据 AST 表达式类型分发到具体的编译方法。
    /// 采用递归下降策略（Recursive Descent），先编译子节点再组合结果。
    ///
    /// # 返回值语义
    ///
    /// 返回值是表达式的结果值所在的**寄存器编号（u16）**。
    /// 调用者可以使用此寄存器获取表达式的计算结果。
    ///
    /// # 分发逻辑
    ///
    /// ```text
    /// match expr {
    ///     // 字面量
    ///     Number   → compile_number()      → LoadK 指令
    ///     String   → compile_string()      → LoadK 指令
    ///     Bool     → compile_bool()        → LoadTrue/LoadFalse
    ///     Nil      → compile_nil()         → LoadNil
    ///
    ///     // 变量访问
    ///     Ident    → compile_ident()       → 三级查找策略
    ///
    ///     // 运算符
    ///     Binary   → compile_binary()      → 声明式 Opcode 映射
    ///     Unary    → compile_unary()       → Neg/Not
    ///     And/Or   → compile_and/or()     → 短路求值
    ///
    ///     // 函数与调用
    ///     Fn/Closure → compile_fn/closure()  → Closure + Capture 指令
    ///     Call     → compile_call()        → Call 指令 + 参数布局
    ///
    ///     // 成员访问
    ///     Index    → compile_index()       → GetIndex
    ///     Field    → compile_field()       → GetProp
    ///
    ///     // 控制流（委托给 statements.rs）
    ///     If       → compile_if()
    ///     While    → compile_while()
    ///     Loop     → compile_loop()
    ///     ForIn    → compile_for_in()
    ///     Break    → compile_break()
    ///     Continue → compile_continue()
    ///     Return   → compile_return()
    ///
    ///     // 复合数据类型（委托给 functions.rs）
    ///     Array    → compile_array()
    ///     Dict     → compile_dict()
    ///     Tuple    → compile_tuple()
    ///     Range    → compile_range()
    ///     Block    → compile_block()
    /// }
    /// ```
    ///
    /// # 行号跟踪
    ///
    /// 每次进入此方法时会更新 `self.current_line` 为当前表达式的行号，
    /// 确保后续发射的指令都携带正确的调试信息。
    ///
    /// # 参数
    ///
    /// * `expr`：要编译的 AST 表达式节点（`ast::Expr`）
    ///
    /// # 返回值
    ///
    /// * `Ok(u16)`：表达式结果值所在的寄存器编号
    /// * `Err(CompileError)`：编译错误（未定义变量、常量池溢出等）
    pub fn compile_expr(&mut self, expr: &ast::Expr) -> Result<u16, CompileError> {
        self.current_line = expr.span().line;
        self.current_column = expr.span().column;
        match expr {
            ast::Expr::Number { value, span } => self.compile_number(*value, span),

            ast::Expr::String { value, span } => self.compile_string(value, span),

            ast::Expr::Bool { value, span } => self.compile_bool(*value, span),

            ast::Expr::Nil { span } => self.compile_nil(span),

            ast::Expr::Ident { name, span } => self.compile_ident(name, span),

            ast::Expr::Binary { left, op, right, span } => {
                self.compile_binary(left, *op, right, span)
            }

            ast::Expr::Unary { op, operand, span } => self.compile_unary(*op, operand, span),

            ast::Expr::Call { callee, args, span } => self.compile_call(callee, args, span),

            ast::Expr::Index { object, index, span } => self.compile_index(object, index, span),

            ast::Expr::Field { object, name, span } => self.compile_field(object, name, span),

            ast::Expr::If { condition, then_branch, else_branch, span } => {
                self.compile_if(condition, then_branch, else_branch.as_deref(), span)
            }

            ast::Expr::While { condition, body, span } => self.compile_while(condition, body, span),

            ast::Expr::Loop { body, span } => self.compile_loop(body, span),

            ast::Expr::ForIn { var_name, iterable, body, span } => {
                self.compile_for_in(var_name, iterable, body, span)
            }

            ast::Expr::Break { value: _, span } => self.compile_break(span),

            ast::Expr::Continue { span } => self.compile_continue(span),

            ast::Expr::Return { value, span } => self.compile_return(value.as_deref(), span),

            ast::Expr::Fn { name, params, body, span } => {
                let fn_name = name.as_deref().unwrap_or("<anonymous>");
                self.compile_fn(fn_name, params, body, span)
            }

            ast::Expr::Closure { params, body, span } => self.compile_closure(params, body, span),

            ast::Expr::Block { statements, span } => self.compile_block(statements, span),

            ast::Expr::Array { elements, span } => self.compile_array(elements, span),

            ast::Expr::Dict { pairs, span } => self.compile_dict(pairs, span),

            ast::Expr::Tuple { elements, span } => self.compile_tuple(elements, span),

            ast::Expr::Range { start, end, inclusive, span } => {
                self.compile_range(start, end, *inclusive, span)
            }

            ast::Expr::And { left, right, span } => self.compile_and(left, right, span),

            ast::Expr::Or { left, right, span } => self.compile_or(left, right, span),

            // M1 Phase 3: 异常处理表达式编译
            ast::Expr::Try { body, catch_clause, keep_block, span } => {
                self.compile_try_expression(body, catch_clause, keep_block, span)
            }
            ast::Expr::Out { value, span } => self.compile_out_expression(value, span),

            // ── 高级抽象：空值合并 ──
            ast::Expr::NullCoalesce { left, right, span } => {
                self.compile_null_coalesce(left, right, span)
            }

            // ── 高级抽象：模式匹配 ──
            ast::Expr::Match { scrutinee, arms, span } => self.compile_match(scrutinee, arms, span),
        }
    }

    // ========================================================================
    // 字面量表达式编译（Literal Expressions）
    // ========================================================================

    /// 编译数字字面量
    ///
    /// 将 f64 数值加载到寄存器中。
    ///
    /// # 生成的字节码
    ///
    /// ```text
    /// LoadK dest_reg, const_index
    /// ```
    ///
    /// # 参数
    ///
    /// * `value`：f64 数值
    /// * `span`：源代码位置信息（用于错误报告和调试信息）
    ///
    /// # 返回值
    ///
    /// 返回存储数值的寄存器编号
    fn compile_number(&mut self, value: f64, span: &ast::Span) -> Result<u16, CompileError> {
        Ok(emit_load_literal!(self, Value::from_number(value), span.line))
    }

    /// 编译字符串字面量
    ///
    /// 将字符串值加载到寄存器中。
    ///
    /// # 生成的字节码
    ///
    /// ```text
    /// LoadK dest_reg, const_index   ; const_index 指向常量池中的字符串
    /// ```
    ///
    /// # 参数
    ///
    /// * `value`：字符串切片引用
    /// * `span`：源代码位置信息
    ///
    /// # 返回值
    ///
    /// 返回存储字符串的寄存器编号
    fn compile_string(&mut self, value: &str, span: &ast::Span) -> Result<u16, CompileError> {
        Ok(emit_load_literal!(self, Value::from_string(value), span.line))
    }

    /// 编译布尔字面量
    ///
    /// 根据布尔值发射 LoadTrue 或 LoadFalse 指令。
    /// 相比使用 LoadK 加载常量，专用指令更紧凑（节省常量池空间）。
    ///
    /// # 生成的字节码
    ///
    /// ```text
    /// true  → LoadTrue dest_reg
    /// false → LoadFalse dest_reg
    /// ```
    ///
    /// # 参数
    ///
    /// * `value`：布尔值
    /// * `_span`：源代码位置信息（未使用，保留接口一致性）
    fn compile_bool(&mut self, value: bool, _span: &ast::Span) -> Result<u16, CompileError> {
        let reg = self.alloc_register()?;
        if value {
            emit_typed!(self, LoadTrue, reg);
        } else {
            emit_typed!(self, LoadFalse, reg);
        }
        Ok(reg)
    }

    /// 编译空值（Nil）字面量
    ///
    /// 发射 LoadNil 指令将寄存器初始化为 nil 值。
    ///
    /// # 生成的字节码
    ///
    /// ```text
    /// LoadNil dest_reg
    /// ```
    fn compile_nil(&mut self, span: &ast::Span) -> Result<u16, CompileError> {
        Ok(emit_load_nil!(self, span.line))
    }

    // ========================================================================
    // 变量访问编译（Variable Access）
    // ========================================================================

    /// 编译标识符（变量名查找与加载）
    ///
    /// 实现**三级变量解析策略**（Three-Level Variable Resolution），按照优先级从高到低查找：
    ///
    /// # 查找优先级
    ///
    /// ## 1. 局部变量（Local Variables）- 最快路径
    /// ```text
    /// 查找位置：当前作用域的局部变量表（Scope）
    /// 实现方式：self.scope.resolve(name)
    /// 返回结果：直接返回寄存器编号（无需 Mov 拷贝）
    /// 性能特点：O(1) 哈希表查找，无额外指令开销
    ///
    /// 设计决策：为什么返回原始寄存器而不是拷贝？
    /// - 大多数场景（如 a + b 的操作数）只需读取值，不需要保护
    /// - compile_call 会在 callee 是 local 时单独 Mov 保护
    ///   （因为 Call 指令会将返回值写入 func_reg，覆盖原有值）
    /// - 避免不必要的 Mov 指令，减少字节码体积
    /// ```
    ///
    /// ## 2. 闭包捕获变量（Captured Variables）- 闭包环境访问
    /// ```text
    /// 触发条件：在函数体内（current_captured.is_some()）
    /// 查找位置：current_captured 列表（CaptureInfo 向量）
    /// 实现方式：线性搜索匹配 name 的 CaptureInfo
    /// 生成的指令：GetCaptured dest_reg, capture_index
    ///
    /// 扁平化环境（FlatEnv）优势：
    /// - 通过索引直接访问捕获槽，无需遍历作用域链
    /// - 捕获索引在编译期确定，运行时 O(1) 访问
    /// - 支持多层嵌套闭包的高效变量访问
    /// ```
    ///
    /// ## 3. 全局变量（Global Variables）- 兜底方案
    /// ```text
    /// 触发条件：前两级都未找到
    /// 查找位置：运行时全局环境（Global Environment）
    /// 生成的指令：
    ///   1. 将变量名添加到常量池 → 得到 name_const_idx
    ///   2. 分配目标寄存器 dest_reg
    ///   3. 发射 GetGlobal dest_reg, name_const_idx
    ///
    /// 性能特点：全局变量访问较慢（需要哈希表查找）
    /// 优化建议：频繁访问的全局变量应缓存到局部变量
    /// ```
    ///
    /// # 参数
    ///
    /// * `name`：要查找的变量名
    /// * `_span`：源代码位置信息（未使用，保留接口一致性）
    ///
    /// # 返回值
    ///
    /// * `Ok(u16)`：变量值所在的寄存器编号
    /// * `Err(CompileError)`：可能的错误（常量池溢出等）
    fn compile_ident(&mut self, name: &str, _span: &ast::Span) -> Result<u16, CompileError> {
        if let Some(kind) = self.scope.resolve(name) {
            match kind {
                ScopeKind::Local(reg) => {
                    // 拷贝局部变量到新寄存器，防止调用者 release_temp_register
                    // 破坏局部变量的值（如 compile_binary、compile_index 等
                    // 会在表达式求值后释放操作数寄存器）
                    let dest = self.alloc_register()?;
                    self.emit_mov(dest, reg);
                    return Ok(dest);
                }
                ScopeKind::Global(_) => unreachable!(),
                _ => unreachable!("ScopeKind 新变体应在 compile_ident 中处理"),
            }
        }

        if let Some(ref captured_vars) = self.current_captured
            && let Some(capture_info) =
                captured_vars.iter().find(|info| info.name == *name).cloned()
        {
            let dest = self.alloc_register()?;
            emit_typed!(self, GetCaptured, dest, capture_info.capture_index.into());
            return Ok(dest);
        }

        let name_const_idx = self.add_constant_checked(Value::from_string(name))?;
        let dest = self.alloc_register()?;
        emit_typed!(self, GetGlobal, dest, name_const_idx);
        Ok(dest)
    }

    // ========================================================================
    // 二元运算编译（Binary Operations）
    // ========================================================================

    /// 编译二元运算表达式
    ///
    /// 使用**声明式映射表**将 AST BinaryOp 转换为对应的 Opcode，
    /// 然后发射三操作数指令。
    ///
    /// # 编译流程
    ///
    /// ```text
    /// 输入: left op right (例如: a + b)
    ///
    /// 步骤 1: 声明式 Opcode 分发
    ///   - 调用 binary_op_to_opcode(op) 查询映射表
    ///   - 如果 op 不支持 → 返回 CompileError::Error
    ///   - 编译期保证：遗漏变体会导致 match 不 exhaustive → 编译错误
    ///
    /// 步骤 2: 编译左操作数
    ///   - self.compile_expr(left) → left_reg
    ///
    /// 步骤 3: 编译右操作数
    ///   - self.compile_expr(right) → right_reg
    ///
    /// 步骤 4: 分配目标寄存器并发射指令
    ///   - dest = alloc_register()
    ///   - emit: opcode dest, left_reg, right_reg
    ///
    /// 步骤 5: 表达式级寄存器释放（关键优化！）
    ///   - if dest != left_reg: release(left_reg)
    ///   - if dest != right_reg: release(right_reg)
    ///   - 原因：操作数在指令执行后不再需要，立即回收复用
    ///
    /// 返回: dest (结果寄存器)
    /// ```
    ///
    /// # 生成的字节码示例
    ///
    /// ```text
    /// 源代码: x + y * z
    ///
    /// 编译 y * z:
    ///   LoadK    r1, const(y)     ; 加载 y
    ///   LoadK    r2, const(z)     ; 加载 z
    ///   Mul      r3, r1, r2       ; r3 = y * z
    ///                           ; 释放 r1, r2
    ///
    /// 编译 x + (y * z):
    ///   LoadK    r4, const(x)     ; 加载 x
    ///   Add      r5, r4, r3       ; r5 = x + r3
    ///                           ; 释放 r3, r4
    ///
    /// 最终结果: r5
    /// ```
    ///
    /// # 参数
    ///
    /// * `left`：左操作数的 AST 表达式
    /// * `op`：二元运算符类型（AST BinaryOp 枚举）
    /// * `right`：右操作数的 AST 表达式
    /// * `span`：源代码位置信息
    ///
    /// # 返回值
    ///
    /// * `Ok(u16)`：运算结果所在的寄存器编号
    /// * `Err(CompileError)`：不支持的运算符或子表达式编译错误
    fn compile_binary(
        &mut self,
        left: &ast::Expr,
        op: ast::BinaryOp,
        right: &ast::Expr,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // ── 常量折叠优化 ──
        // 当左右操作数都是字面量且运算可折叠时，直接在编译期计算结果，
        // 生成单条 LoadK 指令而非 LoadK + LoadK + Op 三条指令。
        // 示例: `3 + 5` → LoadK r0, 8 (而非 LoadK r0, 3; LoadK r1, 5; Add r2, r0, r1)
        if let (Some(left_val), Some(right_val)) =
            (extract_literal_value(left), extract_literal_value(right))
            && let Some(result) = try_fold_binary(op, left_val, right_val)
        {
            // 手动展开 emit_load_literal! 以捕获 const_idx 和 ip 用于 FoldRecord
            let reg = self.alloc_register()?;
            let const_idx = self.add_constant_checked(result)?;
            let loadk_ip = self.chunk.code().len();
            self.emit_opcode_with_line(Opcode::LoadK, span.line);
            self.emit_u16(reg);
            self.emit_u16(const_idx);

            // 记录常量折叠事件到 debug_info
            let description = format!(
                "{} {} {} -> {}",
                format_fold_value(left_val),
                format_binary_op_symbol(op),
                format_fold_value(right_val),
                format_fold_value(result)
            );
            Arc::make_mut(&mut self.chunk.debug_info).fold_records.push(nuzo_values::FoldRecord {
                result_const_idx: const_idx as usize,
                ip: loadk_ip,
                description,
                source_line: span.line,
            });

            return Ok(reg);
        }

        // ── 恒等消除优化（Identity / Algebraic Simplification）──
        // 检测代数恒等式和算术特殊情形，避免生成冗余运算指令。
        //
        // 覆盖的模式（保守策略，仅对无副作用的安全情形优化）：
        // ┌────────────┬─────────────────┬────────────────────────────────────┐
        // │ 模式        │ 优化结果          │ 节省的指令                        │
        // ├────────────┼─────────────────┼────────────────────────────────────┤
        // │ x + 0      │ Mov dest, x      │ 省去 Add + LoadK(0)               │
        // │ 0 + x      │ Mov dest, x      │ 同上                              │
        // │ x - 0      │ Mov dest, x      │ 省去 Sub + LoadK(0)               │
        // │ x * 1      │ Mov dest, x      │ 省去 Mul + LoadK(1)               │
        // │ 1 * x      │ Mov dest, x      │ 同上                              │
        // │ x / 1      │ Mov dest, x      │ 省去 Div + LoadK(1)               │
        // │ x - x      │ LoadK dest, 0    │ 省去 compile(x) + Sub             │
        // │ x * 0      │ LoadK dest, 0    │ 省去 Mul + LoadK(0)（需x无副作用）│
        // │ 0 * x      │ LoadK dest, 0    │ 省去 compile(x) + Mul             │
        // └────────────┴─────────────────┴────────────────────────────────────┘
        //
        // 安全性保证：
        // - 仅对 Number 字面量的 0/1 做恒等判断（不涉及函数调用等副作用表达式）
        // - x - x 和 x * 0 需要确认 x 是简单表达式（标识符或字面量），避免重复求值副作用
        if let Some(identity_result) = self.try_identity_optimization(left, op, right, span)? {
            return Ok(identity_result);
        }

        // 声明式分发：通过宏生成的映射函数获取 Opcode
        // 编译期保证：若 BinaryOp 有变体遗漏，binary_op_to_opcode 的 match 非 exhaustive，编译器报错
        let opcode = match binary_op_to_opcode(op) {
            Some(opcode) => opcode,
            None => {
                return Err(CompileError::Error {
                    message: format!("unsupported binary operator: {:?}", op),
                    line: span.line,
                    column: span.column,
                });
            }
        };

        // 编译左右操作数
        let left_reg = self.compile_expr(left)?;
        let right_reg = self.compile_expr(right)?;

        // 分配目标寄存器并发射指令
        let dest = self.alloc_register()?;
        emit_binary_op!(self, opcode, dest, left_reg, right_reg, span.line);

        // EXPRESSION-LEVEL OPTIMIZATION: Release operand registers immediately
        // This is the KEY to supporting 1000+ line programs!
        if dest != left_reg {
            self.release_temp_register(left_reg);
        }
        if dest != right_reg {
            self.release_temp_register(right_reg);
        }

        Ok(dest)
    }

    // ========================================================================
    // Identity Optimization (恒等消除 / 代数简化)
    // ========================================================================

    /// 检测两个表达式是否为同一操作（用于 x - x, x / x 等模式识别）。
    ///
    /// # 判定策略
    ///
    /// **语法级等价检测（Syntactic Equivalence）**：
    /// - 标识符：比较名称字符串
    /// - 数字字面量：比较 f64 值
    /// - 布尔字面量：比较布尔值
    /// - nil：恒等
    /// - 其他表达式（函数调用、复杂运算等）：返回 false（保守策略）
    ///
    /// # 使用场景
    ///
    /// 主要用于 `try_identity_optimization()` 中的 x - x → 0 优化。
    fn is_self_operation(left: &ast::Expr, right: &ast::Expr) -> bool {
        use ast::Expr;
        match (left, right) {
            // 标识符：名称相同即为自身运算
            (Expr::Ident { name: n1, .. }, Expr::Ident { name: n2, .. }) => n1 == n2,
            // 数字字面量：值相同
            (Expr::Number { value: v1, .. }, Expr::Number { value: v2, .. }) => v1 == v2,
            // 布尔字面量：值相同
            (Expr::Bool { value: v1, .. }, Expr::Bool { value: v2, .. }) => v1 == v2,
            // nil 恒等
            (Expr::Nil { .. }, Expr::Nil { .. }) => true,
            // 复杂表达式不做语法等价判断（保守策略）
            _ => false,
        }
    }

    /// 恒等消除：检测二元运算中的代数恒等式，返回优化后的寄存器编号。
    ///
    /// # 设计哲学
    ///
    /// **保守策略（Conservative Strategy）**：
    /// - 仅对**字面量 0/1** 做恒等判断，不涉及可能产生副作用的表达式
    /// - 对于 x - x 和 x * 0 模式，要求 x 是**简单表达式**（标识符或字面量），
    ///   避免重复编译带副作用的表达式（如函数调用）
    /// - 不优化的模式直接返回 None，走正常编译路径
    ///
    /// # 返回值
    ///
    /// - `Some(reg)`: 成功优化，reg 是结果寄存器
    /// - `None`: 无法优化，需要正常编译
    fn try_identity_optimization(
        &mut self,
        left: &ast::Expr,
        op: ast::BinaryOp,
        right: &ast::Expr,
        span: &ast::Span,
    ) -> Result<Option<u16>, CompileError> {
        use ast::{BinaryOp, Expr};

        // 辅助函数：检查表达式是否为数字字面量 0
        let is_zero =
            |e: &Expr| -> bool { matches!(e, Expr::Number { value, .. } if *value == 0.0) };

        // 辅助函数：检查表达式是否为数字字面量 1
        let is_one =
            |e: &Expr| -> bool { matches!(e, Expr::Number { value, .. } if *value == 1.0) };

        // 辅助函数：检查表达式是否为"简单表达式"（无副作用，可安全重复求值）
        // 简单表达式定义：标识符、数字字面量、布尔字面量、nil
        let is_simple = |e: &Expr| -> bool {
            matches!(
                e,
                Expr::Ident { .. } | Expr::Number { .. } | Expr::Bool { .. } | Expr::Nil { .. }
            )
        };

        match op {
            // ── 加法恒等 ──
            // x + 0 → Mov dest, x  （省去 Add + LoadK(0)）
            BinaryOp::Add if is_zero(right) => {
                let reg = self.compile_expr(left)?;
                Ok(Some(reg))
            }
            // 0 + x → Mov dest, x  （省去 LoadK(0) + Add）
            BinaryOp::Add if is_zero(left) => {
                let reg = self.compile_expr(right)?;
                Ok(Some(reg))
            }

            // ── 减法恒等 ──
            // x - 0 → Mov dest, x  （省去 Sub + LoadK(0)）
            // 注意：0 - x 不能优化（结果是 -x，不是 x）
            BinaryOp::Sub if is_zero(right) => {
                let reg = self.compile_expr(left)?;
                Ok(Some(reg))
            }

            // ── 乘法恒等 ──
            // x * 1 → Mov dest, x  （省去 Mul + LoadK(1)）
            BinaryOp::Mul if is_one(right) => {
                let reg = self.compile_expr(left)?;
                Ok(Some(reg))
            }
            // 1 * x → Mov dest, x  （省去 LoadK(1) + Mul）
            BinaryOp::Mul if is_one(left) => {
                let reg = self.compile_expr(right)?;
                Ok(Some(reg))
            }
            // x * 0 → LoadK dest, 0  （省去 Mul，结果恒为 0）
            // 安全条件：x 必须是简单表达式（无副作用）
            BinaryOp::Mul if is_zero(right) && is_simple(left) => {
                Ok(Some(emit_load_literal!(self, Value::from_number(0.0), span.line)))
            }
            // 0 * x → LoadK dest, 0  （同上，但不需要检查 x 的副作用）
            BinaryOp::Mul if is_zero(left) => {
                Ok(Some(emit_load_literal!(self, Value::from_number(0.0), span.line)))
            }

            // ── 除法恒等 ──
            // x / 1 → Mov dest, x  （省去 Div + LoadK(1)）
            // 注意：1 / x 不能优化（结果是倒数，不是 x）
            BinaryOp::Div if is_one(right) => {
                let reg = self.compile_expr(left)?;
                Ok(Some(reg))
            }

            // ── 自身运算特殊情形 ──
            // x - x → LoadK dest, 0  （任何数减自身都等于 0）
            // 安全条件：左右必须是同一表达式（AST 引用相等）且为简单表达式
            BinaryOp::Sub if Self::is_self_operation(left, right) && is_simple(left) => {
                Ok(Some(emit_load_literal!(self, Value::from_number(0.0), span.line)))
            }

            _ => Ok(None),
        }
    }

    // ========================================================================
    // Unary Operations
    // ========================================================================

    /// Compile unary operation
    ///
    /// Emits: compile operand, Neg/Not dest, src
    ///
    /// # 常量折叠优化
    /// 当操作数是字面量时，直接在编译期计算结果：
    /// - `-42` → LoadK r0, -42 (而非 LoadK r0, 42; Neg r1, r0)
    /// - `!true` → LoadFalse r0 (而非 LoadTrue r0; Not r1, r0)
    fn compile_unary(
        &mut self,
        op: ast::UnaryOp,
        operand: &ast::Expr,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // ── 一元常量折叠优化 ──
        if let Some(operand_val) = extract_literal_value(operand)
            && let Some(result) = try_fold_unary(op, operand_val)
        {
            // 折叠成功：对数字结果使用 LoadK，对布尔结果使用 LoadTrue/LoadFalse
            if result.is_number() {
                // 手动展开 emit_load_literal! 以捕获 const_idx 和 ip 用于 FoldRecord
                let reg = self.alloc_register()?;
                let const_idx = self.add_constant_checked(result)?;
                let loadk_ip = self.chunk.code().len();
                self.emit_opcode_with_line(Opcode::LoadK, span.line);
                self.emit_u16(reg);
                self.emit_u16(const_idx);

                // 记录常量折叠事件到 debug_info
                let description = format!(
                    "{}{} -> {}",
                    format_unary_op_symbol(op),
                    format_fold_value(operand_val),
                    format_fold_value(result)
                );
                Arc::make_mut(&mut self.chunk.debug_info).fold_records.push(
                    nuzo_values::FoldRecord {
                        result_const_idx: const_idx as usize,
                        ip: loadk_ip,
                        description,
                        source_line: span.line,
                    },
                );

                return Ok(reg);
            } else if result.is_bool() {
                let dest = self.alloc_register()?;
                let loadk_ip = self.chunk.code().len();
                if result.as_bool() {
                    emit_typed!(self, LoadTrue, dest);
                } else {
                    emit_typed!(self, LoadFalse, dest);
                }

                // 布尔折叠记录（LoadTrue/LoadFalse 无常量池索引，用 0 占位）
                let description = format!(
                    "{}{} -> {}",
                    format_unary_op_symbol(op),
                    format_fold_value(operand_val),
                    format_fold_value(result)
                );
                Arc::make_mut(&mut self.chunk.debug_info).fold_records.push(
                    nuzo_values::FoldRecord {
                        result_const_idx: 0,
                        ip: loadk_ip,
                        description,
                        source_line: span.line,
                    },
                );

                return Ok(dest);
            }
            // nil 等其他情况不折叠，走正常路径
        }

        let src_reg = self.compile_expr(operand)?;
        let dest = self.alloc_register()?;
        let opcode = match op {
            ast::UnaryOp::Negate => Opcode::Neg,
            ast::UnaryOp::Not => Opcode::Not,
        };
        self.current_column = span.column;
        self.emit_opcode_with_line(opcode, span.line);
        self.emit_u16(dest);
        self.emit_u16(src_reg);

        // EXPRESSION-LEVEL OPTIMIZATION: Release source register
        if dest != src_reg {
            self.release_temp_register(src_reg);
        }

        Ok(dest)
    }

    // ========================================================================
    // 函数调用编译（Function Calls）
    // ========================================================================

    /// 编译函数调用表达式
    ///
    /// 这是编译器中最复杂的方法之一，需要处理：
    /// 1. **VM 调用约定**（Calling Convention）：参数必须连续存放
    /// 2. **寄存器保护**：防止 Call 覆盖被调用者寄存器
    /// 3. **活跃变量溢出**：参数槽覆盖局部变量时的保护机制
    /// 4. **尾调用优化（TCO）支持**：Call 紧跟 Return 的模式识别
    ///
    /// # VM 调用约定（VM Calling Convention）
    ///
    /// ```text
    /// Nuzo VM 采用基于寄存器的调用约定：
    ///
    /// 调用前内存布局:
    /// ┌─────────────┬─────────────┬─────────────┬─────────────┐
    /// │  func_reg   │   arg_1     │   arg_2     │   arg_n     │
    /// │ (函数对象)  │ (参数1)     │ (参数2)     │ (参数n)     │
    /// └─────────────┴─────────────┴─────────────┴─────────────┘
    /// ↑              ←── argc 个连续的参数寄存器 ──→
    /// func_reg
    ///
    /// 规则:
    /// - 参数必须存放在 func_reg+1, func_reg+2, ..., func_reg+argc
    /// - Call 指令格式: Call func_reg, argc
    /// - 返回值存放在 func_reg 中（覆盖原函数对象）
    /// ```
    ///
    /// # 编译流程详解
    ///
    /// ```text
    /// 输入: callee(args...) (例如: foo(a, b, c))
    ///
    /// 步骤 1: 编译被调用者（Callee Compilation）
    ///   raw_func_reg = self.compile_expr(callee)
    ///   ↓ 可能是标识符（局部/全局/捕获）或其他表达式
    ///
    /// 步骤 2: 保护被调用者寄存器（Callee Protection）
    ///   func_reg = alloc_register()
    ///   emit Mov func_reg, raw_func_reg
    ///   release(raw_func_reg)
    ///   ↓ 原因: Call 指令会将返回值写入 func_reg，
    ///         如果直接使用 raw_func_reg，可能覆盖仍在使用的局部变量
    ///
    /// 步骤 3: 计算参数槽范围（Argument Slot Calculation）
    ///   args_start = func_reg + 1
    /// args_end = args_start + argc
    ///   ↓ 确保参数连续存放
    ///
    /// 步骤 4: 活跃变量溢出保护（Active Local Spilling）
    ///   for each active_local in scope.get_active_locals():
    ///     if active_local ∈ [args_start, args_end):
    ///       new_reg = alloc_register()
    ///       emit Mov new_reg, active_local
    ///       scope.rebind(name, new_reg)
    ///   ↓ 防止参数槽覆盖仍在使用的局部变量
    ///
    /// 步骤 5: 预分配参数寄存器并推进 next_reg
    ///   if next_reg < args_end: next_reg = args_end
    ///   从 free_registers 中移除 [args_start, args_end) 范围的寄存器
    ///   ↓ 避免后续 alloc_register 返回已被参数占用的寄存器
    ///
    /// 步骤 6: 编译每个参数并移动到目标槽
    ///   for i, arg in args.iter():
    ///     target = args_start + i
    ///     val_reg = self.compile_expr(arg)
    ///     if val_reg != target: emit Mov target, val_reg
    ///     release(val_reg)
    ///
    /// 步骤 7: 发射 Call 指令
    ///   emit Call func_reg, argc
    ///   ↓ VM 层会检测此指令后是否紧跟 Return，如果是则走 TCO 路径
    ///
    /// 步骤 8: 释放参数寄存器
    ///   for i in 0..argc: release(args_start + i)
    ///
    /// 返回: func_reg (包含返回值)
    /// ```
    ///
    /// # 尾调用优化（Tail Call Optimization, TCO）
    ///
    /// ═══════════════════════════════════════════════════════════
    /// 🔗 TCO 代码分布导航 (本函数是 Call 表达式的编译入口)
    /// ═══════════════════════════════════════════════════════════
    ///
    /// 【本函数职责】编译 `fn_call(args)` 为 OP_CALL 指令，标记尾位置
    ///
    /// 【完整 TCO 链路】
    ///   1. 本函数: 编译 Call 表达式 → 发射 OP_CALL + 设置 is_tail 标记
    ///   2. functions.rs:L130-133: 函数体末尾的隐式 return 保证
    ///   3. dispatch.rs:L668, L695: VM 检测 is_tail → 路由到 TCO
    ///   4. dispatch.rs:L193-290: execute_tail_call() 原地变异帧 ⭐ 核心
    ///
    /// 【设计哲学】编译器 dumb (只发射 Call+Return) → VM smart (自动检测+优化)
    ///
    /// Nuzo VM 实现了自动 TCO 检测机制：
    /// - **编译器职责**：确保 return 表达式中的 Call 紧跟 Return 指令
    /// - **VM 职责**：执行时检测 Call+Return 模式，复用当前栈帧而非分配新的
    /// - **优势**：尾递归函数不会导致栈溢出（如 factorial、fibonacci 等）
    ///
    /// 示例：
    /// ```text
    /// // 源代码
    /// fn factorial(n, acc) {
    ///     if (n <= 1) { return acc }
    ///     return factorial(n - 1, n * acc)  // 尾调用位置
    /// }
    ///
    /// // 生成的字节码（简化）
    /// ...
    /// Call r0, 2      ; 调用 factorial(n-1, n*acc)
    /// Return r0        ; 紧跟 Return → VM 识别为尾调用
    /// ```
    ///
    /// # 参数
    ///
    /// * `callee`：被调用者的 AST 表达式（可以是标识符、属性访问等）
    /// * `args`：实参表达式的切片
    /// * `_span`：源代码位置信息
    ///
    /// # 返回值
    ///
    /// * `Ok(u16)`：包含返回值的寄存器编号（func_reg）
    /// * `Err(CompileError)`：子表达式编译错误或寄存器耗尽
    fn compile_call(
        &mut self,
        callee: &ast::Expr,
        args: &[ast::Expr],
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // 编译被调用者表达式
        let raw_func_reg = self.compile_expr(callee)?;

        // 分配函数寄存器并移动 callee 结果
        let func_reg = {
            let dest = self.alloc_register()?;
            self.emit_mov(dest, raw_func_reg);
            self.release_temp_register(raw_func_reg);
            dest
        };

        let argc = args.len() as u8;

        // VM calling convention: args must be at func_reg+1, func_reg+2, ... (contiguous)
        let args_start = func_reg + 1;
        let args_end = args_start + argc as u16;

        // Save any active locals that would be overwritten by arg slots
        let active_locals = self.scope.active_locals();
        for &reg in &active_locals {
            if reg >= args_start && reg < args_end {
                let new_reg = self.alloc_register()?;
                self.emit_mov(new_reg, reg);
                if let Some(name) = self.scope.find_name_by_reg(reg) {
                    self.scope.rebind(&name, new_reg);
                }
            }
        }

        // 推进 next_reg 确保参数槽位不被后续 alloc_register 复用
        if self.next_reg < args_end {
            self.next_reg = args_end;
            self.peak_reg = self.peak_reg.max(self.next_reg);
        }
        // Watermark protection: prevent release_temp_register from shrinking
        // next_reg into the pre-allocated arg slot zone [args_start, args_end).
        let saved_watermark = self.reserve_watermark;
        self.reserve_watermark = args_end;

        // 从 free_registers 中移除参数槽位范围内的寄存器，防止重复分配
        let kept: Vec<_> = self
            .free_registers
            .drain()
            .filter(|Reverse(r)| *r < args_start || *r >= args_end)
            .collect();
        self.free_registers.extend(kept);

        // Compile each arg, move result to pre-allocated contiguous slot
        for (i, arg) in args.iter().enumerate() {
            let val_reg = self.compile_expr(arg)?;
            let target = args_start + i as u16;
            if val_reg != target {
                self.emit_mov(target, val_reg);
                // val_reg is outside the arg slot zone, safe to release
                self.release_temp_register(val_reg);
            }
            // If val_reg == target, the value is already in its arg slot.
            // Do NOT release it — it must stay protected until Call reads it.
        }

        // Restore watermark
        self.reserve_watermark = saved_watermark;

        // 发射 Call 指令
        emit_typed!(self, Call, func_reg, argc);

        Ok(func_reg)
    }

    // ========================================================================
    // Property/Index Access
    // ========================================================================

    fn compile_index(
        &mut self,
        object: &ast::Expr,
        index: &ast::Expr,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let obj_reg = self.compile_expr(object)?;
        let idx_reg = self.compile_expr(index)?;
        let dest = self.alloc_register()?;
        emit_typed!(self, GetIndex, dest, obj_reg, idx_reg);

        // 注意：不释放 obj_reg 和 idx_reg，因为 compile_ident 对局部变量
        // 直接返回寄存器号（不拷贝），释放会破坏局部变量的值。
        // 临时寄存器由循环/块的 release_registers 统一回收。

        Ok(dest)
    }

    /// Compile field access (object.field)
    ///
    /// Emits: compile object, GetProp obj, const_idx(field_name)
    fn compile_field(
        &mut self,
        object: &ast::Expr,
        name: &str,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let obj_reg = self.compile_expr(object)?;
        let dest = self.alloc_register()?;
        let name_const_idx = self.add_constant_checked(Value::from_string(name))?;
        emit_typed!(self, GetProp, dest, obj_reg, name_const_idx);

        // EXPRESSION-LEVEL OPTIMIZATION: Release object register
        if dest != obj_reg {
            self.release_temp_register(obj_reg);
        }

        Ok(dest)
    }

    // ========================================================================
    // Logical Operations (Short-Circuit)
    // ========================================================================

    /// Compile logical AND (short-circuit)
    ///
    /// If left is falsy, return left without evaluating right
    fn compile_and(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let left_reg = self.compile_expr(left)?;
        let dest = self.alloc_register()?;
        self.emit_mov(dest, left_reg);
        let test_ip = emit_test_with_placeholder!(self, left_reg, span.line);
        let right_reg = self.compile_expr(right)?;
        self.emit_mov(dest, right_reg);
        let end_ip = self.chunk.code().len();
        self.patch_jump(test_ip, end_ip)?;

        // EXPRESSION-LEVEL OPTIMIZATION: Release temporary registers
        if dest != left_reg {
            self.release_temp_register(left_reg);
        }
        if dest != right_reg {
            self.release_temp_register(right_reg);
        }

        Ok(dest)
    }

    /// Compile logical OR (short-circuit)
    ///
    /// If left is truthy, return left without evaluating right
    fn compile_or(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let left_reg = self.compile_expr(left)?;
        let dest = self.alloc_register()?;
        self.emit_mov(dest, left_reg);

        let temp_reg = self.alloc_register()?;
        // 修复：原代码错误地使用了 emit_opcode(Opcode::Not, span.line)，
        // 将 span.line 当作了操作数传入，导致行号信息丢失/错乱。
        // 必须使用 emit_opcode_with_line 记录当前表达式的行号。
        emit_typed!(self, Not, temp_reg, left_reg);

        let jump_pos = emit_test_with_placeholder!(self, temp_reg, span.line);
        let right_reg = self.compile_expr(right)?;
        self.emit_mov(dest, right_reg);

        let end_pos = self.chunk.code().len();
        self.patch_jump(jump_pos, end_pos)?;

        // EXPRESSION-LEVEL OPTIMIZATION: Release temporary registers
        self.release_temp_register(temp_reg); // Always release temp
        if dest != left_reg {
            self.release_temp_register(left_reg);
        }
        if dest != right_reg {
            self.release_temp_register(right_reg);
        }

        Ok(dest)
    }

    // ========================================================================
    // Advanced Abstractions (高级抽象编译)
    // ========================================================================

    /// 编译空值合并表达式 `left ?? right`
    ///
    /// 语义：当 left 为 nil 时返回 right，否则返回 left（left 只求值一次）。
    ///
    /// 生成的字节码布局：
    /// ```text
    ///   [compile left → left_reg]
    ///   [Mov dest, left_reg]           // dest = left
    ///   [IsNil temp, left_reg]         // temp = (left == nil)
    ///   [Test temp → jump to right]    // if temp is truthy, jump to right
    ///   [Jmp end]                      // else skip right
    ///   <patch: right_start>
    ///   [compile right → right_reg]
    ///   [Mov dest, right_reg]          // dest = right
    ///   <patch: end>
    /// ```
    fn compile_null_coalesce(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        let left_reg = self.compile_expr(left)?;
        let dest = self.alloc_register()?;
        self.emit_mov(dest, left_reg);

        // Load nil into a temp register for comparison
        let nil_reg = self.alloc_register()?;
        emit_typed!(self, LoadNil, nil_reg);

        // Emit equality comparison: is_nil_reg = (left == nil)
        let is_nil_reg = self.alloc_register()?;
        emit_typed!(self, Eq, is_nil_reg, left_reg, nil_reg);

        // If left IS nil, jump to evaluate right
        let jump_to_right = emit_test_with_placeholder!(self, is_nil_reg, span.line);

        // Left is not nil: dest already has left value, jump to end
        let jump_to_end = emit_jmp_with_placeholder!(self, span.line);

        // Right branch: evaluate right and move to dest
        let right_start = self.chunk.code().len();
        self.patch_jump(jump_to_right, right_start)?;
        let right_reg = self.compile_expr(right)?;
        self.emit_mov(dest, right_reg);

        let end_ip = self.chunk.code().len();
        self.patch_jump(jump_to_end, end_ip)?;

        // Release temporary registers
        self.release_temp_register(nil_reg);
        self.release_temp_register(is_nil_reg);
        if dest != left_reg {
            self.release_temp_register(left_reg);
        }
        if dest != right_reg {
            self.release_temp_register(right_reg);
        }

        Ok(dest)
    }

    /// 编译模式匹配表达式 `match (scrutinee) { pattern => body, ... }`
    ///
    /// 编译策略：脱糖为 if-else 链
    ///
    /// - 字面量模式 → 相等比较 (==)
    /// - 范围模式 → 范围包含检查
    /// - 变量绑定模式 → 绑定值并执行 body（总是匹配）
    /// - 通配符模式 → 总是匹配
    ///
    /// 生成的字节码布局（简化）：
    /// ```text
    ///   [compile scrutinee → scrut_reg]
    ///   [Mov dest, scrut_reg]            // dest holds the result
    ///
    ///   // For each literal arm:
    ///   [compile literal → lit_reg]
    ///   [Eq eq_reg, scrut_reg, lit_reg]
    ///   [Test eq_reg → jump to arm body]
    ///   ...
    ///
    ///   // Variable/Wildcard arm (always matches):
    ///   [compile arm body → body_reg]
    ///   [Mov dest, body_reg]
    ///   [Jmp end]
    /// ```
    fn compile_match(
        &mut self,
        scrutinee: &ast::Expr,
        arms: &[ast::MatchArm],
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        if arms.is_empty() {
            let dest = self.alloc_register()?;
            emit_typed!(self, LoadNil, dest);
            return Ok(dest);
        }

        // Compile scrutinee into a register
        let scrut_reg = self.compile_expr(scrutinee)?;
        let dest = self.alloc_register()?;
        self.emit_mov(dest, scrut_reg);

        // Track jump-to-end placeholders for each arm that matches
        let mut end_jumps: Vec<usize> = Vec::new();

        for arm in arms {
            match &arm.pattern {
                ast::MatchPattern::Literal(lit_expr) => {
                    // Compile literal pattern value
                    let lit_reg = self.compile_expr(lit_expr)?;

                    // Emit equality comparison: eq_reg = (scrut == lit)
                    let eq_reg = self.alloc_register()?;
                    emit_typed!(self, Eq, eq_reg, scrut_reg, lit_reg);

                    // Test: if eq_reg is truthy, jump to arm body
                    let jump_to_body = emit_test_with_placeholder!(self, eq_reg, span.line);

                    // Release literal and eq registers
                    self.release_temp_register(lit_reg);
                    self.release_temp_register(eq_reg);

                    // Patch: arm body starts here
                    let body_start = self.chunk.code().len();
                    self.patch_jump(jump_to_body, body_start)?;

                    // Compile arm body
                    let body_reg = self.compile_expr(&arm.body)?;
                    self.emit_mov(dest, body_reg);
                    if dest != body_reg {
                        self.release_temp_register(body_reg);
                    }

                    // Jump to end
                    let jmp_end = emit_jmp_with_placeholder!(self, span.line);
                    end_jumps.push(jmp_end);
                }

                ast::MatchPattern::Range { start, end, inclusive } => {
                    // Compile range bounds
                    let start_reg = self.compile_expr(start)?;
                    let end_reg = self.compile_expr(end)?;

                    // Check: scrut >= start
                    let ge_reg = self.alloc_register()?;
                    emit_typed!(self, Ge, ge_reg, scrut_reg, start_reg);

                    // If scrut < start, skip this arm
                    let skip_ge = emit_test_with_placeholder!(self, ge_reg, span.line);
                    self.release_temp_register(ge_reg);

                    // Check: scrut <= end (inclusive) or scrut < end (exclusive)
                    let cmp_reg = self.alloc_register()?;
                    if *inclusive {
                        emit_typed!(self, Le, cmp_reg, scrut_reg, end_reg);
                    } else {
                        emit_typed!(self, Lt, cmp_reg, scrut_reg, end_reg);
                    }

                    // If cmp fails, skip this arm
                    let skip_cmp = emit_test_with_placeholder!(self, cmp_reg, span.line);
                    self.release_temp_register(cmp_reg);
                    self.release_temp_register(start_reg);
                    self.release_temp_register(end_reg);

                    // Both conditions met: compile arm body
                    let body_start = self.chunk.code().len();
                    self.patch_jump(skip_ge, body_start)?;

                    let body_reg = self.compile_expr(&arm.body)?;
                    self.emit_mov(dest, body_reg);
                    if dest != body_reg {
                        self.release_temp_register(body_reg);
                    }

                    let jmp_end = emit_jmp_with_placeholder!(self, span.line);
                    end_jumps.push(jmp_end);

                    // Patch skip_cmp to jump past the body
                    let after_body = self.chunk.code().len();
                    self.patch_jump(skip_cmp, after_body)?;
                }

                ast::MatchPattern::Variable(var_name) => {
                    // Bind scrutinee value to the variable name
                    // Declare a new local variable and copy scrut_reg into it
                    let var_reg = self.declare_local(var_name.clone())?;
                    if var_reg != scrut_reg {
                        self.emit_mov(var_reg, scrut_reg);
                    }

                    // Variable always matches: compile body
                    let body_reg = self.compile_expr(&arm.body)?;
                    self.emit_mov(dest, body_reg);
                    if dest != body_reg {
                        self.release_temp_register(body_reg);
                    }

                    let jmp_end = emit_jmp_with_placeholder!(self, span.line);
                    end_jumps.push(jmp_end);
                }

                ast::MatchPattern::Wildcard => {
                    // Wildcard always matches: compile body
                    let body_reg = self.compile_expr(&arm.body)?;
                    self.emit_mov(dest, body_reg);
                    if dest != body_reg {
                        self.release_temp_register(body_reg);
                    }

                    let jmp_end = emit_jmp_with_placeholder!(self, span.line);
                    end_jumps.push(jmp_end);
                }
            }
        }

        // Patch all end jumps to here
        let end_ip = self.chunk.code().len();
        for jmp_ip in &end_jumps {
            self.patch_jump(*jmp_ip, end_ip)?;
        }

        // Release scrutinee register
        self.release_temp_register(scrut_reg);

        Ok(dest)
    }

    /// 编译 try-catch[-keep] 表达式
    ///
    /// 生成的字节码布局:
    /// ```text
    ///   [TryStart catch_offset exc_reg]  <- 标记 try 块开始（catch_offset 先占位，后续补丁）
    ///   [try_body...]                    <- try 块的字节码
    ///   [Jmp over_catch]                 <- 正常路径跳过 catch 块（如果有 catch）
    ///   <patch: catch_start>             <- catch 入口地址（回填 TryStart 的 catch_offset）
    ///   [catch_body...]                  <- catch 块的字节码
    ///   [keep_block...]                  <- keep 块的字节码（如果有）
    ///   [TryEnd]                         <- 标记 try 块结束（正常路径弹出异常栈）
    /// ```
    ///
    /// 异常传播路径（out 触发时）:
    /// VM 查找 exception_stack -> 找到 TryStart 帧 -> 跳转到 catch_offset 对应的位置
    ///
    /// # Offset 计算说明
    ///
    /// TryStart 的 catch_offset 是**相对于 TryStart 下一条指令**的字节偏移（i16）。
    /// 补丁公式: `offset = catch_ip - (try_start_ip + TryStart指令大小4字节)`
    /// 这与 `patch_jump()` 的计算方式一致: `target_ip - (jump_ip + instr_size)`
    pub(super) fn compile_try_expression(
        &mut self,
        body: &ast::Block,
        catch_clause: &Option<Box<ast::CatchClause>>,
        keep_block: &Option<ast::Block>,
        span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // Step 1: 分配一个寄存器用于存放异常值（catch 绑定变量将映射到此寄存器）
        let exc_reg = self.alloc_register()?;

        // Step 2: 发射 TryStart 指令（catch_offset 先填 0 占位，后续补丁）
        // TryStart 编码格式: opcode(1) + catch_offset:i16(2) + exception_reg:u8(1) = 4 字节
        let try_start_ip = self.chunk.code().len();
        emit_typed!(self, TryStart, 0i16, exc_reg as u8);

        // Step 3: 进入 try 块作用域，编译 try 体
        // 注意：ast::Block 是 Vec<Stmt> 的类型别名，直接作为语句列表使用
        self.enter_scope("try");
        let try_result = self.compile_block_expr(body)?;
        // try 表达式的结果需要保留到最终返回，暂不释放

        // Step 4: 如果有 catch 子句，发射跳过 catch 块的 Jmp（正常路径不执行 catch）
        let jmp_over_catch = if catch_clause.is_some() {
            Some(emit_jmp_with_placeholder!(self, span.line))
        } else {
            None
        };

        // Step 5: 补丁 TryStart 的 catch_offset（指向当前位置 = catch 入口）
        // Offset = catch入口IP - (TryStart IP + TryStart指令大小4)
        let catch_ip = self.chunk.code().len();
        self.patch_jump(try_start_ip, catch_ip)?;

        // Step 6: 编译 catch 块（如果存在）
        if let Some(clause) = catch_clause {
            // 进入 catch 作用域，将 catch 绑定变量映射到 exc_reg
            // 注意：ast::Identifier 是 String 的类型别名，binding 直接就是变量名
            self.enter_scope("catch");
            self.scope.define(&clause.binding, exc_reg);

            let catch_result = self.compile_block_expr(&clause.body)?;

            // 补丁跳过 catch 的 Jmp（正常路径从 try 直接跳到这里）
            if let Some(jmp_ip) = jmp_over_catch {
                let end_ip = self.chunk.code().len();
                self.patch_jump(jmp_ip, end_ip)?;
            }

            // 退出 catch 作用域
            self.exit_scope();
            // catch 结果不再需要（keep 块或 TryEnd 在后面），释放临时寄存器
            self.release_temp_register(catch_result);
        }

        // Step 7: 编译 keep 块（如果存在）— 无论 try 正常完成还是 catch 完成，都会执行
        if let Some(keep) = keep_block {
            self.enter_scope("keep");
            let keep_result = self.compile_block_expr(keep)?;
            self.release_temp_register(keep_result);
            self.exit_scope();
        }

        // Step 8: 发射 TryEnd（标记 try 块结束，正常路径弹出异常栈）
        emit_typed!(self, TryEnd);

        // 退出 try 作用域
        self.exit_scope();

        // 返回 try 块的结果寄存器作为整个 try 表达式的值
        Ok(try_result)
    }

    /// 编译 out(抛出) 表达式
    ///
    /// 生成的字节码:
    /// ```text
    ///   [value_expr...]     <- 编译要抛出的值到寄存器
    ///   Out value_reg       <- 发射 Out 指令（VM 跳转到 catch）
    /// ```
    ///
    /// # 执行语义
    ///
    /// Out 指令执行时:
    /// 1. 从 value_reg 读取异常值
    /// 2. 查找最近的 TryStart 对应的异常栈帧
    /// 3. 将异常值存入 TryStart 的 exception_reg
    /// 4. 跳转到 catch_offset 指向的 catch 块入口
    pub(super) fn compile_out_expression(
        &mut self,
        value: &ast::Expr,
        _span: &ast::Span,
    ) -> Result<u16, CompileError> {
        // 编译异常值表达式，得到其所在寄存器
        let value_reg = self.compile_expr(value)?;

        // 发射 Out 指令: opcode(1) + value_reg:u16(2) = 3 字节
        emit_typed!(self, Out, value_reg);

        // out 后面的代码不会执行（VM 会跳转到 catch），
        // 但为了类型系统一致性，返回 value_reg 作为结果寄存器
        Ok(value_reg)
    }
}

// ============================================================================
// 单元测试 (Unit Tests)
// ============================================================================

#[cfg(test)]
mod tests {

    use crate::compiler::Compiler;

    #[test]
    fn test_compile_expr_number_literal() {
        // 编译数字字面量表达式
        let program = nuzo_frontend::parser::Parser::parse("42").unwrap();
        let mut compiler = Compiler::builder().source("42".to_string()).build();

        // 取第一条语句中的表达式
        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            let reg = compiler.compile_expr(expr).expect("compile_expr 应成功编译数字字面量");
            // 返回的寄存器编号应为有效值
            assert!(reg < nuzo_core::MAX_FUNCTION_LOCALS);
        } else {
            panic!("预期 Stmt::Expr");
        }
    }

    #[test]
    fn test_compile_expr_binary_arithmetic() {
        // 编译二元算术表达式
        let program = nuzo_frontend::parser::Parser::parse("1 + 2").unwrap();
        let mut compiler = Compiler::builder().source("1 + 2".to_string()).build();

        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            let reg = compiler.compile_expr(expr).expect("compile_expr 应成功编译二元运算");
            assert!(reg < nuzo_core::MAX_FUNCTION_LOCALS);
        } else {
            panic!("预期 Stmt::Expr");
        }
    }

    #[test]
    fn test_compile_expr_string_literal() {
        let program = nuzo_frontend::parser::Parser::parse("\"hello\"").unwrap();
        let mut compiler = Compiler::builder().source("\"hello\"".to_string()).build();

        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            let reg = compiler.compile_expr(expr).expect("compile_expr 应成功编译字符串字面量");
            assert!(reg < nuzo_core::MAX_FUNCTION_LOCALS);
        } else {
            panic!("预期 Stmt::Expr");
        }
    }

    #[test]
    fn test_compile_expr_updates_current_line() {
        // compile_expr 应更新 current_line 和 current_column
        let program = nuzo_frontend::parser::Parser::parse("42").unwrap();
        let mut compiler = Compiler::builder().source("42".to_string()).build();

        if let nuzo_frontend::ast::Stmt::Expr(expr) = &program.statements[0] {
            compiler.current_line = 0;
            compiler.current_column = 0;
            let _ = compiler.compile_expr(expr).unwrap();
            // 编译后 current_line 应被更新为表达式的行号
            assert!(
                compiler.current_line > 0 || compiler.current_column > 0,
                "compile_expr 应更新 current_line/current_column"
            );
        }
    }
}
