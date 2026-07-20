//! # Nuzo 抽象语法树（AST）模块
//!
//! ## 模块职责
//! 定义 Nuzo 语言的完整语法树节点类型系统，是词法分析和语法分析的产物，
//! 也是后续语义分析、代码生成和解释执行的输入。
//!
//! ## 核心设计原则：一切皆表达式（Expression-Oriented）
//!
//! Nuzo 采用**表达式优先**的语法设计，这意味着：
//! - **控制流结构返回值**：`if/else`, `while`, `loop` 都可以产生值
//! - **块是表达式**：`{ ... }` 的最后一个表达式是其返回值
//! - **函数是表达式**：函数定义可以作为值传递
//! - **声明即语句**：顶层声明被包装为语句节点
//!
//! ### 表达式优先的好处
//! ```nuzo
//! // if 表达式可以直接赋值
//! let message = if (success) { "OK" } else { "FAIL" }
//!
//! // 函数可以作为参数
//! apply(fn(x) { x * 2 }, 5)
//!
//! // 块的最后一个表达式是返回值
//! let result = {
//!     let temp = compute()
//!     temp + 1  // 这是块的返回值
//! }
//! ```
//!
//! ## AST 节点层次结构
//!
//! ```text
//! Program（程序入口）
//! └── Vec<Stmt>（语句列表）
//!     ├── Expr(Expr)          // 表达式语句
//!     └── Assign { ... }      // 赋值语句
//!
//! Stmt（语句类型）
//! ├── Expr(Expr)             // 纯表达式语句
//! └── Assign {
//!     target: AssignTarget,  // 赋值目标
//!     value: Expr,           // 赋值值
//! }
//!
//! AssignTarget（赋值目标）
//! ├── Ident { name }         // 变量名
//! ├── Index { object, index }// 数组/字典索引
//! └── Field { object, name } // 对象属性
//!
//! Expr（表达式类型 - 核心枚举，20+ 变体）
//! ├── 字面量（Literals）
//! │   ├── Number { value }   // 数值
//! │   ├── String { value }   // 字符串
//! │   ├── Bool { value }     // 布尔
//! │   └── Nil                // 空值
//! ├── 标识符
//! │   └── Ident { name }     // 变量/函数引用
//! ├── 运算符表达式
//! ├── 调用与访问
//! ├── 控制流（Control Flow）
//! ├── 函数定义
//! ├── 复合表达式
//! ```
//!
//! ## 文法规则摘要（EBNF 风格）
//!
//! ```text
//! program       ::= statement*
//! statement     ::= expression_stmt | assignment | fn_declaration
//! expression    ::= assignment_expr | arrow_expr | or_expr
//! assignment    ::= target '=' expression | target compound_op expression
//! arrow_expr    ::= ident '=>' body | '(' params ')' '=>' body | or_expr
//! or_expr       ::= and_expr ('||' | 'or' and_expr)*
//! and_expr      ::= comparison ('&&' | 'and' comparison)*
//! comparison    ::= range (('==' | '!=' | '<' | '>' | '<=' | '>=') range)?
//! range         ::= addition (('..'| '..<') addition)*
//! addition      ::= multiplication (('+' | '-') multiplication)*
//! multiplication ::= unary (('*' | '/' | '%') unary)*
//! unary         ::= ('-' | '!') unary | call_expr
//! call_expr     ::= primary ('(' args ')')* ('[' expr ']')* ('.' ident)*
//! primary       ::= literal | ident | if_expr | loop_expr | fn_expr
//!                  | '(' expr (',' expr)* ')' | '[' elements ']'
//!                  | '{' pairs '}' | block
//! ```
//!
//! ## Span（位置信息）系统
//!
//! 每个 AST 节点都携带 [`Span`] 信息，用于：
//! - **错误定位**：精确指出语法错误的位置
//! - **源码映射**：将 AST 节点映射回源码文本
//! - **调试信息**：生成带有行号的调试输出
//!
//! ```text
//! Span {
//!     line: usize,   // 行号（1-based）
//!     column: usize, // 列号（1-based）
//! }
//! ```

use std::fmt;

/// 源码位置信息
///
/// 记录 AST 节点在源代码中的位置，用于错误报告和调试。
///
/// # 使用场景
/// - 编译器错误消息："第 10 行第 5 列：期望 ')'"
/// - IDE 高亮显示错误位置
/// - 源码映射表生成
///
/// # 注意事项
/// - 目前只记录**起始位置**，不记录结束位置
/// - 未来可扩展为包含范围信息 `Span { start, end }`
#[derive(Debug, Clone)]
pub struct Span {
    /// 行号（从 1 开始计数）
    pub line: usize,
    /// 列号（从 1 开始计数，UTF-8 字符单位）
    pub column: usize,
}

impl Span {
    /// 创建新的位置信息
    ///
    /// # 参数
    /// * `line` - 行号（通常来自 Token.line）
    /// * `column` - 列号（通常来自 Token.column）
    pub fn new(line: usize, column: usize) -> Self {
        Span { line, column }
    }
}

/// 完整的程序（编译单元）
///
/// 表示一个 Nuzo 源文件解析后的完整 AST。
/// 由一系列**顶级语句**组成。
///
/// # 结构
/// ```text
/// Program {
///     statements: Vec<Stmt>,
/// }
/// ```
///
/// # 示例
/// 对于以下源代码：
/// ```nuzo
/// fn add(a, b) { a + b }
/// result = add(1, 2)
/// print(result)
/// ```
///
/// 对应的 AST 为：
/// ```text
/// Program {
///     statements: [
///         Expr(Fn { name: "add", ... }),  // 函数定义
///         Assign { target: "result", ... },// 赋值语句
///         Expr(Call { callee: "print", ... }) // 函数调用
///     ]
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Program {
    /// 顶级语句列表（按源码顺序排列）
    pub statements: Vec<Stmt>,
}

/// 语句类型枚举
///
/// 表示 Nuzo 语言中的**语句**级别的语法构造。
/// 语句是程序的基本执行单元，通常不产生值（或其值被忽略）。
///
/// # 设计决策：为什么区分 Stmt 和 Expr？
/// 虽然 Nuzo 是表达式优先的语言，但仍需要语句层来处理：
/// 1. **赋值操作**：`x = expr` 不是表达式，不能嵌套
/// 2. **顶层声明**：函数定义在顶层是声明，不是表达式
/// 3. **副作用边界**：明确标识有副作用的操作
///
/// # 变体说明
#[derive(Debug, Clone, nuzo_proc::MatchSync)]
pub enum Stmt {
    /// 表达式语句 - 将表达式作为语句执行
    ///
    /// 大多数语法构造都可以作为表达式语句出现：
    /// ```nuzo
    /// foo()                    // 函数调用作为语句
    /// if (condition) { ... }   // if 作为语句
    /// x + 1                    // 算术表达式作为语句（虽然无意义）
    /// ```
    Expr(Expr),

    /// 赋值语句 - 将值绑定到目标位置
    ///
    /// # 支持的赋值目标
    /// - **变量赋值**：`x = 42`
    /// - **索引赋值**：`arr[0] = 100`
    /// - **属性赋值**：`obj.prop = "value"`
    ///
    /// # 复合赋值
    /// 复合赋值（如 `+=`, `-=`）会被 desugar 为二元运算 + 赋值：
    /// ```nuzo
    /// x += 1  // 会被转换为: x = x + 1
    /// ```
    Assign {
        /// 赋值目标（变量/索引/属性）
        target: AssignTarget,
        /// 要赋值的表达式
        value: Expr,
        /// 赋值运算符的位置信息（用于错误报告）
        span: Span,
    },

    /// import 语句 - 导入外部模块
    ///
    /// 支持两种模式：
    /// - **eager import**（`lazy: false`）：在解析时立即加载
    /// - **lazy import**（`lazy: true`）：在首次使用时加载
    ///
    /// # 示例
    /// ```nuzo
    /// import "module.nuzo"              // eager
    /// lazy import "module.nuzo"        // lazy
    /// ```
    Import {
        /// 导入路径（字面量）
        path: String,
        /// 是否为 lazy import
        lazy: bool,
        /// 模块别名（`as` 语法，预留）
        alias: Option<String>,
        /// 位置信息（用于错误报告）
        span: Span,
    },
}

/// 赋值目标类型枚举
///
/// 定义哪些语法构造可以作为赋值的左侧（L-value）。
/// 这实现了**写时复制（Copy-on-Write）语义的基础检查**。
///
/// # 合法的赋值目标
/// | 目标类型 | 示例 | 说明 |
/// |---------|------|------|
/// | 标识符 | `x = 1` | 简单变量赋值 |
/// | 索引访问 | `arr[0] = 2` | 数组/字典元素赋值 |
/// | 属性访问 | `obj.x = 3` | 对象属性赋值 |
///
/// # 非法的赋值目标（会在 Parser 阶段报错）
/// - 字面量：`1 = 2` （错误）
/// - 函数调用结果：`foo() = 3` （错误）
/// - 算术表达式：`(x + y) = 5` （错误）
#[derive(Debug, Clone)]
pub enum AssignTarget {
    /// 标识符赋值目标（变量名）
    ///
    /// 示例：`x = 42` 中的 `x`
    Ident {
        /// 变量名称
        name: String,
    },

    /// 索引赋值目标（数组/字典元素）
    ///
    /// 示例：`arr[0] = 100` 或 `dict["key"] = "value"`
    Index {
        /// 被索引的对象（数组或字典表达式）
        object: Box<Expr>,
        /// 索引表达式
        index: Box<Expr>,
    },

    /// 属性赋值目标（对象字段）
    ///
    /// 示例：`obj.prop = "value"` 或 `obj.a.b.c = 42`
    Field {
        /// 被访问的对象表达式
        object: Box<Expr>,
        /// 属性名称
        name: String,
    },
}

/// 标识符类型别名（用于 catch 绑定等场景）
pub type Identifier = String;

/// catch 子句 - 捕获异常并绑定到变量
///
/// 当 try 块中抛出异常时，catch 子句负责接收并处理。
/// 支持可选的类型过滤和异常值绑定。
///
/// # 语法形式
/// ```nuzo
/// catch (e) {
///     // 处理异常 e
/// }
///
/// // 带类型过滤
/// catch (e: TypeError) {
///     // 仅处理 TypeError 类型的异常
/// }
/// ```
#[derive(Debug, Clone)]
pub struct CatchClause {
    /// 异常绑定变量名（catch (e) 中的 e）
    pub binding: Identifier,
    /// 可选的类型过滤表达式（如 TypeError）
    pub exception_type: Option<Expr>,
    /// catch 的执行体
    pub body: Block,
}

/// 表达式类型枚举 - Nuzo AST 的核心数据结构
///
/// 表示 Nuzo 语言中所有可以**产生值**的语法构造。
/// 这是 AST 中最复杂、变体最多的枚举，涵盖了：
/// - 字面量（Literals）
/// - 运算符表达式（Operators）
/// - 控制流结构（Control Flow）
/// - 函数定义与调用（Functions & Calls）
/// - 数据结构字面量（Data Structures）
///
/// # 设计原则：一切皆表达式
/// 在 Nuzo 中，几乎所有语法构造都是表达式，这意味着：
/// 1. **if/else 可以返回值**
///    ```nuzo
///    let max = if (a > b) { a } else { b }
///    ```
/// 2. **块可以返回值**（最后一个表达式的值）
///    ```nuzo
///    let result = {
///        let x = compute()
///        x * 2  // 块的返回值
///    }
///    ```
/// 3. **函数可以作为值传递**
///    ```nuzo
///    let double = fn(x) { x * 2 }
///    apply(double, 5)
///    ```
///
/// # 内存布局优化
/// 使用 `Box<Expr>` 包装递归和大型子表达式：
/// - 减小 Expr 枚举的整体大小
/// - 避免递归类型导致的无限大小
/// - 允许高效的克隆（只复制指针）
#[derive(Debug, Clone, nuzo_proc::MatchSync, nuzo_proc::ExprVisitor)]
pub enum Expr {
    /// 数值字面量
    ///
    /// 支持整数和浮点数，内部统一使用 f64 存储。
    /// Parser 负责将字符串转换为数值。
    ///
    /// # 示例
    /// ```nuzo
    /// 42       // 整数
    /// 3.14     // 浮点数
    /// 0.0      // 零
    /// -100     // 负数（解析为一元运算 + 正数）
    /// ```
    Number {
        /// 数值（f64 可精确表示整数到 2^53）
        value: f64,
        /// 源码位置
        span: Span,
    },

    /// 字符串字面量
    ///
    /// 支持 UTF-8 编码的中文字符。
    /// Lexer 返回的是去除引号的内容。
    ///
    /// # 示例
    /// ```nuzo
    /// "hello world"     // 英文字符串
    /// '单引号字符串'    // 单引号形式
    /// "中文 🎉 Emoji"   // UTF-8 字符
    /// ```
    String {
        /// 字符串内容（不包含引号）
        value: String,
        /// 源码位置
        span: Span,
    },

    /// 布尔字面量
    ///
    /// # 可能的值
    /// - `true` / `真`
    /// - `false` / `假`
    Bool {
        /// 布尔值
        value: bool,
        /// 源码位置
        span: Span,
    },

    /// 空值字面量
    ///
    /// 表示"无值"或"缺失"，类似于其他语言的 null/nil/None。
    /// 关键字：`nil` / `空`
    Nil {
        /// 源码位置
        span: Span,
    },

    /// 标识符引用（变量或函数名）
    ///
    /// 引用当前作用域中的变量或函数。
    /// 语义分析阶段会将其解析为具体的变量绑定。
    ///
    /// # 示例
    /// ```nuzo
    /// x           // 变量引用
    /// foo         // 函数引用
    /// myVariable  // 驼峰命名
    /// _private    // 下划线开头
    /// ```
    Ident {
        /// 标识符名称
        name: String,
        /// 源码位置
        span: Span,
    },

    /// 二元运算表达式
    ///
    /// 对两个子表达式应用二元运算符。
    /// Parser 会根据**运算符优先级**正确构建嵌套结构。
    ///
    /// # 支持的运算符
    /// | 类别 | 运算符 | 说明 |
    /// |------|--------|------|
    /// | 算术 | `+`, `-`, `*`, `/`, `%` | 四则运算 |
    /// | 比较 | `==`, `!=`, `<`, `>`, `<=`, `>=` | 比较运算 |
    /// | 位运算 | (预留) | 未来扩展 |
    ///
    /// # 优先级示例
    /// ```nuzo
    /// 1 + 2 * 3  // 解析为: 1 + (2 * 3)  （乘法优先）
    /// a < b && c > d  // 解析为: (a < b) && (c > d)
    /// ```
    Binary {
        /// 左操作数
        left: Box<Expr>,
        /// 运算符类型
        op: BinaryOp,
        /// 右操作数
        right: Box<Expr>,
        /// 运算符位置（用于错误报告）
        span: Span,
    },

    /// 一元运算表达式
    ///
    /// 对单个子表达式应用一元运算符。
    ///
    /// # 支持的运算符
    /// | 运算符 | 说明 | 示例 |
    /// |--------|------|------|
    /// | `-` | 算术取负 | `-x`, `-42` |
    /// | `!` | 逻辑非 | `!true`, `!(x > 0)` |
    Unary {
        /// 运算符类型
        op: UnaryOp,
        /// 操作数
        operand: Box<Expr>,
        /// 运算符位置
        span: Span,
    },

    /// 函数调用表达式
    ///
    /// 对一个可调用对象（函数、闭包）进行调用。
    /// 参数按值传递（语义分析阶段决定是否需要拷贝）。
    ///
    /// # 示例
    /// ```nuzo
    /// foo()              // 无参数调用
    /// add(1, 2)          // 多参数调用
    /// obj.method(x)      // 方法调用（实际上是 Field + Call）
    /// fn(x){x}(5)        // IIFE（立即执行函数表达式）
    /// ```
    Call {
        /// 被调用的对象（可以是标识符、属性访问等）
        callee: Box<Expr>,
        /// 实参列表（按位置匹配形参）
        args: Vec<Expr>,
        /// 左括号位置
        span: Span,
    },

    /// 索引访问表达式
    ///
    /// 通过索引访问数组或字典的元素。
    ///
    /// # 示例
    /// ```nuzo
    /// arr[0]          // 数组首元素
    /// dict["key"]     // 字典键值访问
    /// matrix[i][j]   // 多维数组（嵌套 Index）
    /// ```
    Index {
        /// 被索引的对象（数组或字典表达式）
        object: Box<Expr>,
        /// 索引表达式
        index: Box<Expr>,
        /// 左方括号位置
        span: Span,
    },

    /// 属性访问表达式（字段访问）
    ///
    /// 访问对象的命名属性/字段。
    ///
    /// # 示例
    /// ```nuzo
    /// obj.name            // 单层属性访问
    /// obj.a.b.c           // 链式属性访问
    /// user.profile.age    // 嵌套对象访问
    /// ```
    Field {
        /// 被访问的对象表达式
        object: Box<Expr>,
        /// 属性名称
        name: String,
        /// 点号位置
        span: Span,
    },

    /// 条件分支表达式（if-else）
    ///
    /// Nuzo 的 if 是**表达式**，可以返回值。
    /// 这使得函数式编程风格更加自然。
    ///
    /// # 语法形式
    /// ```nuzo
    /// if (condition) {
    ///     then_branch
    /// } else {
    ///     else_branch  // 可选
    /// }
    /// ```
    ///
    /// # 作为表达式使用
    /// ```nuzo
    /// let abs_x = if (x >= 0) { x } else { -x }
    /// let message = if (success) { "OK" } else { "FAIL" }
    /// ```
    ///
    /// # else-if 链
    /// 多个 if-else 会嵌套形成链：
    /// ```nuzo
    /// if (x > 0) {
    ///     "positive"
    /// } else if (x < 0) {
    ///     "negative"
    /// } else {
    ///     "zero"
    /// }
    /// ```
    If {
        /// 条件表达式（必须产生布尔值）
        condition: Box<Expr>,
        /// 条件为真时执行的代码块
        then_branch: Block,
        /// 条件为假时的可选分支（None 表示无 else）
        else_branch: Option<Box<Expr>>,
        /// if 关键字的位置
        span: Span,
    },

    /// 当型循环表达式（while）
    ///
    /// 在条件为真时重复执行循环体。
    /// 与 if 类似，while 也是表达式（虽然其返回值通常被忽略）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// while (condition) {
    ///     body
    /// }
    /// ```
    ///
    /// # 控制流
    /// - `break` - 跳出循环（可带返回值）
    /// - `continue` - 跳到下一次迭代
    ///
    /// # 示例
    /// ```nuzo
    /// while (i < 10) {
    ///     i = i + 1
    /// }
    /// ```
    While {
        /// 循环条件（每次迭代前求值）
        condition: Box<Expr>,
        /// 循环体（反复执行的语句块）
        body: Block,
        /// while 关键字的位置
        span: Span,
    },

    /// 无限循环表达式（loop）
    ///
    /// 无条件重复执行循环体，只能通过 `break` 退出。
    /// 常用于实现复杂的循环逻辑或服务器主循环。
    ///
    /// # 语法形式
    /// ```nuzo
    /// loop {
    ///     body
    /// }
    /// ```
    ///
    /// # 使用场景
    /// - 事件处理主循环
    /// - 状态机实现
    /// - 复杂的退出条件（多个 break 点）
    ///
    /// # 示例
    /// ```nuzo
    /// loop {
    ///     let event = wait_for_event()
    ///     if (event.type == QUIT) { break }
    ///     handle(event)
    /// }
    /// ```
    Loop {
        /// 循环体（无限执行直到 break）
        body: Block,
        /// loop 关键字的位置
        span: Span,
    },

    /// 遍历循环表达式（for-in）
    ///
    /// 遍历可迭代对象（如数组）中的每个元素。
    ///
    /// # 语法形式
    /// ```nuzo
    /// for (variable in iterable) {
    ///     body
    /// }
    /// ```
    ///
    /// # 变量作用域
    /// 循环变量 `var_name` 在每次迭代时绑定到当前元素，
    /// 其作用域限制在循环体内。
    ///
    /// # 示例
    /// ```nuzo
    /// for (item in [1, 2, 3]) {
    ///     print(item)  // 输出: 1, 2, 3
    /// }
    ///
    /// for (i in 0..<10) {
    ///     print(i)      // 输出: 0 到 9
    /// }
    /// ```
    ForIn {
        /// 循环变量名
        var_name: String,
        /// 可迭代对象（数组、范围等）
        iterable: Box<Expr>,
        /// 循环体
        body: Block,
        /// for 关键字的位置
        span: Span,
    },

    /// 跳出循环表达式（break）
    ///
    /// 用于提前终止最近的循环（loop/while/for）。
    /// 可以携带一个可选的表达式作为循环的返回值。
    ///
    /// # 语法形式
    /// ```nuzo
    /// break           // 无值跳出
    /// break value     // 带值跳出
    /// ```
    ///
    /// # 示例
    /// ```nuzo
    /// let result = loop {
    ///     if (found) { break item }  // 带值跳出
    /// }
    /// ```
    Break {
        /// 可选的跳出值
        value: Option<Box<Expr>>,
        /// break 关键字的位置
        span: Span,
    },

    /// 继续循环表达式（continue）
    ///
    /// 跳过当前迭代的剩余部分，直接进入下一次迭代。
    /// 只能用于循环体内（loop/while/for）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// continue
    /// ```
    ///
    /// # 注意事项
    /// continue 不能携带返回值（与 break 不同）。
    Continue {
        /// continue 关键字的位置
        span: Span,
    },

    /// 返回表达式（return）
    ///
    /// 从当前函数返回一个可选值。
    /// 如果没有提供值，默认返回 nil。
    ///
    /// # 语法形式
    /// ```nuzo
    /// return          // 返回 nil
    /// return value    // 返回指定值
    /// ```
    ///
    /// # 语义
    /// - 在函数体内：立即从函数返回
    /// - 在闭包内：从闭包返回
    /// - 在顶层：编译错误（不允许在顶层使用 return）
    Return {
        /// 可选的返回值（None 表示返回 nil）
        value: Option<Box<Expr>>,
        /// return 关键字的位置
        span: Span,
    },

    /// try-catch 表达式（异常处理）
    ///
    /// Nuzo 的异常处理采用 try/catch/out/keep 四段式设计：
    /// - **try**：可能抛出异常的代码块
    /// - **catch**：捕获并处理异常（可选）
    /// - **keep**：无论是否异常都执行的清理块（可选，类似 finally）
    /// - **out**：在 try/catch 内部抛出异常
    ///
    /// # 语法形式
    /// ```nuzo
    /// // 基本 try-catch
    /// try {
    ///     risky_operation()
    /// } catch (e) {
    ///     handle_error(e)
    /// }
    ///
    /// // 带 keep 块（始终执行）
    /// try {
    ///     open_file()
    /// } catch (e) {
    ///     log_error(e)
    /// } keep {
    ///     cleanup()  // 无论是否异常都执行
    /// }
    /// ```
    ///
    /// # 作为表达式使用
    /// try-catch 是表达式，可以返回值：
    /// ```nuzo
    /// let result = try {
    ///         parse_config()
    ///     } catch (e) {
    ///         default_config()  // 异常时返回默认值
    ///     }
    /// ```
    Try {
        /// try 块（可能抛出异常的代码）
        body: Block,
        /// catch 子句（可选，用于捕获和处理异常）
        catch_clause: Option<Box<CatchClause>>,
        /// keep/finally 块（可选，无论是否异常都执行）
        keep_block: Option<Block>,
        /// try 关键字的位置
        span: Span,
    },

    /// 抛出异常表达式（out）
    ///
    /// 从当前 try 块中抛出一个异常，中断正常控制流。
    /// 异常会被最近的 catch 子句捕获。
    ///
    /// # 语法形式
    /// ```nuzo
    /// out "something went wrong"     // 抛出字符串异常
    /// out {code: 404, message: "Not Found"}  // 抛出字典异常
    /// out error_value               // 抛出任意值作为异常
    /// ```
    ///
    /// # 语义
    /// - 只能在 try 块内使用
    /// - 立即跳转到对应的 catch 子句
    /// - 如果没有匹配的 catch，异常会向上传播
    Out {
        /// 异常值（可以是字符串、字典或任意表达式）
        value: Box<Expr>,
        /// out 关键字的位置
        span: Span,
    },

    /// 函数定义表达式（具名或匿名）
    ///
    /// 定义一个可复用的代码块，可以命名（具名函数）或匿名。
    /// 函数是 Nuzo 的一等公民，可以作为值传递和存储。
    ///
    /// # 语法形式
    /// ```nuzo
    /// // 具名函数
    /// fn name(param1, param2) {
    ///     body
    /// }
    ///
    /// // 匿名函数
    /// fn(param1, param2) {
    ///     body
    /// }
    /// ```
    ///
    /// # 参数传递
    /// - 按值传递（语义分析阶段决定是否需要拷贝）
    /// - 不支持默认参数（目前）
    /// - 不支持可变参数/剩余参数（目前）
    ///
    /// # 返回值
    /// 函数的返回值是**块中最后一个表达式的值**，
    /// 或者通过 `return` 语句显式返回。
    Fn {
        /// 函数名称（None 表示匿名函数）
        name: Option<String>,
        /// 形式参数列表（参数名列表，无类型标注）
        params: Vec<String>,
        /// 函数体（语句块）
        body: Block,
        /// fn 关键字的位置
        span: Span,
    },

    /// Lambda 闭包表达式（箭头函数）
    ///
    /// 使用箭头语法定义的匿名函数，语法更简洁。
    /// 自动捕获外部作用域的变量（闭包语义）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// // 单参数简写形式
    /// x => x * 2
    ///
    /// // 多参数形式
    /// (a, b) => a + b
    ///
    /// // 无参数形式
    /// () => 42
    ///
    /// // 块体形式
    /// (x) => {
    ///     let temp = x * 2
    ///     temp + 1
    /// }
    /// ```
    ///
    /// # 与 Fn 的区别
    /// | 特性 | Closure | Fn |
    /// |------|---------|-----|
    /// | 语法 | `x => expr` | `fn(x) { ... }` |
    /// | 名称 | 总是匿名 | 可命名 |
    /// | 用途 | 短回调、函数式编程 | 完整函数定义 |
    Closure {
        /// 参数列表
        params: Vec<String>,
        /// 函数体
        body: Block,
        /// 箭头的位置
        span: Span,
    },

    /// 代码块表达式
    ///
    /// 由花括号包围的一系列语句。
    /// 作为表达式使用时，其值是**最后一个表达式的值**。
    ///
    /// # 语法形式
    /// ```nuzo/// {
    ///     statement1;
    ///     statement2;
    ///     last_expression  // 这是块的返回值
    /// }
    /// ```
    ///
    /// # 变量作用域
    /// 块引入新的作用域，内部声明的变量在外部不可见：
    /// ```nuzo
    /// let x = {
    ///     let y = 10  // y 只在块内可见
    ///     y * 2       // 块返回 20
    /// }
    /// // 这里不能访问 y
    /// ```
    Block {
        /// 块内的语句列表（按顺序执行）
        statements: Vec<Stmt>,
        /// 左花括号的位置
        span: Span,
    },

    /// 数组字面量
    ///
    /// 创建包含有序元素的可变长度序列。
    /// 元素可以是任意表达式。
    ///
    /// # 语法形式
    /// ```nuzo
    /// []              // 空数组
    /// [1, 2, 3]       // 整数数组
    /// ["a", "b"]      // 字符串数组
    /// [1, "two", true] // 异构数组
    /// ```
    ///
    /// # 索引访问
    /// 通过 [`Expr::Index`] 访问元素：`arr[0]`
    Array {
        /// 元素列表（按位置索引，从 0 开始）
        elements: Vec<Expr>,
        /// 左方括号的位置
        span: Span,
    },

    /// 字典（映射）字面量
    ///
    /// 创建键值对的无序集合。
    /// 键必须是字符串（标识符或字符串字面量）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// {}                          // 空字典
    /// {name: "Alice", age: 30}   // 键值对字典
    /// {"key": value}             // 字符串键
    /// ```
    ///
    /// # 访问方式
    /// - 通过字符串索引：`dict["key"]`
    /// - 通过点号访问：`dict.name`（仅限标识符键）
    Dict {
        /// 键值对列表（键是字符串，值是任意表达式）
        pairs: Vec<(String, Expr)>,
        /// 左花括号的位置
        span: Span,
    },

    /// 元组字面量
    ///
    /// 创建固定长度的有序元素集合。
    /// 与数组不同，元组的类型系统可能不同（未来扩展）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// ()           // 单元元组（特殊）
    /// (1,)         // 单元素元组（注意逗号）
    /// (1, 2)       // 二元组
    /// (1, "two", 3)// 三元组
    /// ```
    ///
    /// # 解构赋值（计划中）
    /// 未来支持：`let (a, b) = tuple`
    Tuple {
        /// 元素列表（固定长度）
        elements: Vec<Expr>,
        /// 左圆括号的位置
        span: Span,
    },

    /// 范围表达式
    ///
    /// 表示一个数值范围，常用于 for-in 循环和切片操作。
    ///
    /// # 语法形式
    /// ```nuzo
    /// start..end      // 包含范围 [start, end]
    /// start..<end     // 不包含范围 [start, end)
    /// ```
    ///
    /// # 示例
    /// ```nuzo
    /// 1..5    // 包含 1, 2, 3, 4, 5
    /// 1..<5   // 包含 1, 2, 3, 4
    /// 0..10   // 从 0 到 9
    /// ```
    Range {
        /// 起始值（包含）
        start: Box<Expr>,
        /// 结束值（根据 inclusive 决定是否包含）
        end: Box<Expr>,
        /// 是否包含结束值
        /// - true: `..` (闭区间)
        /// - false: `..<` (左闭右开)
        inclusive: bool,
        /// 第一个点的位置
        span: Span,
    },

    /// 短路逻辑与表达式
    ///
    /// 当左操作数为假时，不计算右操作数（短路）。
    /// 返回第一个假值或最后一个真值（类似 JavaScript）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// left && right
    /// left and right  // 中英文两种形式
    /// ```
    And {
        /// 左操作数
        left: Box<Expr>,
        /// 右操作数
        right: Box<Expr>,
        /// 运算符位置
        span: Span,
    },

    /// 短路逻辑或表达式
    ///
    /// 当左操作数为真时，不计算右操作数（短路）。
    /// 返回第一个真值或最后一个假值（类似 JavaScript）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// left || right
    /// left or right  // 中英文两种形式
    /// ```
    Or {
        /// 左操作数
        left: Box<Expr>,
        /// 右操作数
        right: Box<Expr>,
        /// 运算符位置
        span: Span,
    },

    /// 空值合并表达式（null coalescing）
    ///
    /// 当左操作数为 nil 时返回右操作数，否则返回左操作数。
    /// 左操作数只求值一次（避免重复求值副作用）。
    ///
    /// # 语法形式
    /// ```nuzo
    /// value ?? default
    /// ```
    ///
    /// # 语义等价
    /// ```nuzo
    /// // value ?? default 等价于：
    /// if (value != nil) { value } else { default }
    /// // 但 value 只求值一次
    /// ```
    ///
    /// # 示例
    /// ```nuzo
    /// let name = user.name ?? "Anonymous"
    /// let port = config.port ?? 8080
    /// ```
    NullCoalesce {
        /// 左操作数（可能为 nil 的值）
        left: Box<Expr>,
        /// 右操作数（默认值）
        right: Box<Expr>,
        /// 运算符位置
        span: Span,
    },

    /// 模式匹配表达式（match）
    ///
    /// 对一个值进行多路分支匹配，类似 Rust/ML 的 match 表达式。
    /// 每个分支包含一个模式和一个结果表达式。
    ///
    /// # 语法形式
    /// ```nuzo
    /// match value {
    ///     pattern1 => result1,
    ///     pattern2 => result2,
    ///     _ => default,
    /// }
    /// ```
    ///
    /// # 支持的模式类型
    /// - **字面量模式**：`0`, `true`, `"hello"` — 值相等即匹配
    /// - **变量绑定模式**：`n` — 匹配任意值并绑定到变量 n
    /// - **通配符模式**：`_` — 匹配任意值，不绑定
    /// - **范围模式**：`1..10` — 值在范围内即匹配
    ///
    /// # 示例
    /// ```nuzo
    /// let desc = match (n) {
    ///     0 => "zero",
    ///     1 => "one",
    ///     _ => "many"
    /// }
    ///
    /// // 带变量绑定
    /// match (http_status) {
    ///     200 => "OK",
    ///     404 => "Not Found",
    ///     code => "Unknown: " + code
    /// }
    /// ```
    Match {
        /// 被匹配的表达式（scrutinee）
        scrutinee: Box<Expr>,
        /// 匹配分支列表（按顺序匹配，第一个匹配的分支被执行）
        arms: Vec<MatchArm>,
        /// match 关键字的位置
        span: Span,
    },
}

pub type Block = Vec<Stmt>;

/// 模式匹配分支的模式类型
///
/// 表示 match 表达式中每个分支的匹配模式。
///
/// # 模式优先级
/// 当多个模式可能匹配同一值时，按 arms 中的声明顺序选择第一个匹配的分支。
#[derive(Debug, Clone)]
pub enum MatchPattern {
    /// 字面量模式：匹配值相等的字面量
    ///
    /// 示例：`0 => ...`, `true => ...`, `"hello" => ...`
    Literal(Expr),

    /// 范围模式：匹配在范围内的值
    ///
    /// 示例：`1..10 => ...`, `0..<100 => ...`
    Range {
        /// 范围起始（包含）
        start: Box<Expr>,
        /// 范围结束
        end: Box<Expr>,
        /// 是否包含结束值
        inclusive: bool,
    },

    /// 变量绑定模式：匹配任意值并绑定到变量名
    ///
    /// 示例：`n => ...`, `x => ...`
    ///
    /// 注意：变量绑定模式会匹配任意值，因此通常放在最后作为默认分支。
    Variable(String),

    /// 通配符模式：匹配任意值，不绑定变量
    ///
    /// 示例：`_ => ...`
    Wildcard,
}

/// 模式匹配分支
///
/// 表示 match 表达式中的一个分支，包含匹配模式和结果表达式。
#[derive(Debug, Clone)]
pub struct MatchArm {
    /// 匹配模式
    pub pattern: MatchPattern,
    /// 分支结果表达式
    pub body: Expr,
}

impl Expr {
    /// Get the span of this expression
    pub fn span(&self) -> &Span {
        match self {
            Expr::Number { span, .. } => span,
            Expr::String { span, .. } => span,
            Expr::Bool { span, .. } => span,
            Expr::Nil { span } => span,
            Expr::Ident { span, .. } => span,
            Expr::Binary { span, .. } => span,
            Expr::Unary { span, .. } => span,
            Expr::Call { span, .. } => span,
            Expr::Index { span, .. } => span,
            Expr::Field { span, .. } => span,
            Expr::If { span, .. } => span,
            Expr::While { span, .. } => span,
            Expr::Loop { span, .. } => span,
            Expr::ForIn { span, .. } => span,
            Expr::Break { span, .. } => span,
            Expr::Continue { span } => span,
            Expr::Return { span, .. } => span,
            Expr::Try { span, .. } => span,
            Expr::Out { span, .. } => span,
            Expr::Fn { span, .. } => span,
            Expr::Closure { span, .. } => span,
            Expr::Block { span, .. } => span,
            Expr::Array { span, .. } => span,
            Expr::Dict { span, .. } => span,
            Expr::Tuple { span, .. } => span,
            Expr::Range { span, .. } => span,
            Expr::And { span, .. } => span,
            Expr::Or { span, .. } => span,
            Expr::NullCoalesce { span, .. } => span,
            Expr::Match { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, nuzo_proc::MatchSync)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    /// 取模运算（`%` 符号和 `mod` 关键字均映射到此变体）
    Mod,
    Pow,
    Eq,
    Neq,
    Lt,
    Gt,
    LtEq,
    GtEq,
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::Mod => write!(f, "%"),
            BinaryOp::Pow => write!(f, "**"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::Neq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::LtEq => write!(f, "<="),
            BinaryOp::GtEq => write!(f, ">="),
        }
    }
}

// ============================================================================
// ExprVisitor — 泛型 AST 遍历 trait（零开销抽象）
// ============================================================================

/// 泛型 AST 遍历器 trait，统一所有对 `Expr` 树的递归遍历逻辑。
///
/// # 设计目标
/// 消除 `nuzo_ir::builder` 中 `FreeVarCollector` 和 `nuzo_compiler::functions` 中
/// `IdentifierCollector` / `CompilerAssignedVarCollector` 的重复 match 分发。
/// 现在所有 AST 遍历逻辑都通过实现此 trait 来完成，约减少 320 行重复代码。
///
/// # 零开销
/// 使用泛型 + 单态化（monomorphization），编译器为每个具体实现生成内联代码，
/// 无动态分发开销。等价于手写 match。
///
/// # 使用方式
/// 实现此 trait，覆盖需要自定义逻辑的方法。默认实现会递归遍历子节点。
/// `visit_expr` 是入口点，默认调用 `default_visit_expr`。
///
/// ```rust,ignore
/// struct MyVisitor { /* ... */ }
///
/// impl ExprVisitor for MyVisitor {
///     fn visit_ident(&mut self, name: &str, _span: &Span) {
///         // 自定义标识符处理逻辑
///     }
///     // 其他方法使用默认实现（递归遍历子节点）
/// }
/// ```
pub trait ExprVisitor {
    /// 访问标识符节点（叶子节点）
    ///
    /// 这是最常见的需要自定义的方法——自由变量收集、作用域分析等
    /// 都需要拦截标识符访问。
    fn visit_ident(&mut self, _name: &str, _span: &Span) {}

    /// 访问字面量节点（叶子节点）
    ///
    /// Number, String, Bool, Nil 统一走此方法。如果需要区分具体类型，
    /// 请在实现中 match `expr`。
    fn visit_literal(&mut self, _expr: &Expr) {}

    /// 访问赋值语句
    ///
    /// 默认实现：记录赋值目标标识符，然后遍历赋值值表达式。
    /// 覆盖此方法可自定义赋值变量收集逻辑。
    fn visit_assign(&mut self, target: &AssignTarget, value: &Expr, _span: &Span) {
        self.visit_expr(value);
        // 遍历赋值目标中的子表达式（如 obj.field = expr 中的 obj）
        match target {
            AssignTarget::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            AssignTarget::Field { object, .. } => {
                self.visit_expr(object);
            }
            AssignTarget::Ident { .. } => {}
        }
    }

    /// 访问函数/闭包定义
    ///
    /// 默认实现：不递归进入函数体（避免混入外层作用域的分析）。
    /// 覆盖此方法可自定义闭包捕获分析逻辑，例如需要跨层捕获分析时
    /// 可调用 `self.visit_block(body)` 递归进入函数体。
    fn visit_fn(&mut self, _name: Option<&str>, _params: &[String], _body: &Block, _span: &Span) {}

    /// 访问语句块
    ///
    /// 默认实现：遍历所有语句，对表达式语句调用 `visit_expr`，
    /// 对赋值语句调用 `visit_assign`。
    ///
    /// 覆盖此方法可自定义语句级遍历逻辑。
    fn visit_block(&mut self, statements: &[Stmt]) {
        for stmt in statements {
            match stmt {
                Stmt::Expr(expr) => self.visit_expr(expr),
                Stmt::Assign { target, value, span } => {
                    self.visit_assign(target, value, span);
                }
                Stmt::Import { .. } => {}
            }
        }
    }

    /// 入口方法：访问任意表达式
    ///
    /// 默认实现调用 `default_visit_expr`，递归遍历所有子节点。
    /// 覆盖此方法可拦截特定表达式类型。
    fn visit_expr(&mut self, expr: &Expr) {
        default_visit_expr(self, expr);
    }
}

/// 默认的递归遍历实现（自由函数，避免 trait object 限制）
///
/// 此函数遍历 `Expr` 的所有子节点，对叶子节点调用 `visit_ident` 或 `visit_literal`，
/// 对函数定义调用 `visit_fn`，其余通过 `visit_expr` 递归。
///
/// # 注意
/// 此函数不遍历 `Stmt`（语句级别遍历由调用方自行处理）。
pub fn default_visit_expr<V: ExprVisitor + ?Sized>(visitor: &mut V, expr: &Expr) {
    match expr {
        Expr::Number { .. } | Expr::String { .. } | Expr::Bool { .. } | Expr::Nil { .. } => {
            visitor.visit_literal(expr);
        }
        Expr::Ident { name, span } => {
            visitor.visit_ident(name, span);
        }

        Expr::Binary { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        Expr::Unary { operand, .. } => {
            visitor.visit_expr(operand);
        }
        Expr::Call { callee, args, .. } => {
            visitor.visit_expr(callee);
            for arg in args {
                visitor.visit_expr(arg);
            }
        }
        Expr::Index { object, index, .. } => {
            visitor.visit_expr(object);
            visitor.visit_expr(index);
        }
        Expr::Field { object, .. } => {
            visitor.visit_expr(object);
        }
        Expr::If { condition, then_branch, else_branch, .. } => {
            visitor.visit_expr(condition);
            visitor.visit_block(then_branch);
            if let Some(e) = else_branch {
                visitor.visit_expr(e);
            }
        }
        Expr::While { condition, body, .. } => {
            visitor.visit_expr(condition);
            visitor.visit_block(body);
        }
        Expr::Loop { body, .. } => {
            visitor.visit_block(body);
        }
        Expr::ForIn { iterable, body, .. } => {
            visitor.visit_expr(iterable);
            visitor.visit_block(body);
        }
        Expr::Break { value: Some(v), .. } => {
            visitor.visit_expr(v);
        }
        Expr::Break { value: None, .. } | Expr::Continue { .. } => {}
        Expr::Return { value: Some(v), .. } => {
            visitor.visit_expr(v);
        }
        Expr::Return { value: None, .. } => {}
        Expr::Try { body, .. } => {
            visitor.visit_block(body);
        }
        Expr::Out { value, .. } => {
            visitor.visit_expr(value);
        }
        Expr::Fn { name, params, body, span } => {
            visitor.visit_fn(name.as_deref(), params, body, span);
        }
        Expr::Closure { params, body, span } => {
            visitor.visit_fn(None, params, body, span);
        }
        Expr::Block { statements, .. } => {
            visitor.visit_block(statements);
        }
        Expr::Array { elements, .. } => {
            for el in elements {
                visitor.visit_expr(el);
            }
        }
        Expr::Dict { pairs, .. } => {
            for (_, val) in pairs {
                visitor.visit_expr(val);
            }
        }
        Expr::Tuple { elements, .. } => {
            for el in elements {
                visitor.visit_expr(el);
            }
        }
        Expr::Range { start, end, .. } => {
            visitor.visit_expr(start);
            visitor.visit_expr(end);
        }
        Expr::And { left, right, .. } | Expr::Or { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        Expr::NullCoalesce { left, right, .. } => {
            visitor.visit_expr(left);
            visitor.visit_expr(right);
        }
        Expr::Match { scrutinee, arms, .. } => {
            visitor.visit_expr(scrutinee);
            for arm in arms {
                match &arm.pattern {
                    MatchPattern::Literal(lit) => visitor.visit_expr(lit),
                    MatchPattern::Range { start, end, .. } => {
                        visitor.visit_expr(start);
                        visitor.visit_expr(end);
                    }
                    MatchPattern::Variable(_) | MatchPattern::Wildcard => {}
                }
                visitor.visit_expr(&arm.body);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
    Not,
}

// ============================================================================
// Tests — 验证 derive(ExprVisitor) 生成的代码与手写 default_visit_expr 等价
// ============================================================================

#[cfg(test)]
mod expr_visitor_derived_tests {
    use super::*;

    /// 访问日志收集器
    #[derive(Default, Clone, Debug, PartialEq, Eq)]
    struct CallLog {
        log: Vec<String>,
    }

    /// Path A：通过 trait 默认 `visit_expr` → `default_visit_expr`（手写版）
    struct HandwrittenPath(CallLog);

    impl ExprVisitor for HandwrittenPath {
        fn visit_ident(&mut self, name: &str, _span: &Span) {
            self.0.log.push(format!("ident:{}", name));
        }
        fn visit_literal(&mut self, _expr: &Expr) {
            self.0.log.push("literal".to_string());
        }
        fn visit_fn(&mut self, name: Option<&str>, params: &[String], _body: &Block, _span: &Span) {
            self.0.log.push(format!("fn:{}:{}", name.unwrap_or("<anon>"), params.len()));
        }
        // visit_expr 使用默认实现 → 调用 default_visit_expr
    }

    /// Path B：覆盖 `visit_expr` 改用 derive 生成的 `visit_children_derived`
    struct DerivedPath(CallLog);

    impl ExprVisitor for DerivedPath {
        fn visit_ident(&mut self, name: &str, _span: &Span) {
            self.0.log.push(format!("ident:{}", name));
        }
        fn visit_literal(&mut self, _expr: &Expr) {
            self.0.log.push("literal".to_string());
        }
        fn visit_fn(&mut self, name: Option<&str>, params: &[String], _body: &Block, _span: &Span) {
            self.0.log.push(format!("fn:{}:{}", name.unwrap_or("<anon>"), params.len()));
        }
        // 关键：覆盖 visit_expr，使用 derive 生成的版本
        fn visit_expr(&mut self, expr: &Expr) {
            expr.visit_children_derived(self);
        }
    }

    /// 对一个 Expr 同时跑手写路径和 derived 路径，断言日志完全一致
    fn assert_equivalent(expr: &Expr, context: &str) {
        let mut handwritten = HandwrittenPath(CallLog::default());
        handwritten.visit_expr(expr);

        let mut derived = DerivedPath(CallLog::default());
        derived.visit_expr(expr);

        assert_eq!(
            handwritten.0.log, derived.0.log,
            "[{}] behavioral mismatch\n  handwritten: {:?}\n  derived:     {:?}",
            context, handwritten.0.log, derived.0.log
        );
    }

    // ── 字面量与 Ident ─────────────────────────────────────────

    #[test]
    fn derived_matches_literals() {
        assert_equivalent(&Expr::Number { value: 42.0, span: Span::new(1, 1) }, "Number");
        assert_equivalent(
            &Expr::String { value: "hi".to_string(), span: Span::new(1, 1) },
            "String",
        );
        assert_equivalent(&Expr::Bool { value: true, span: Span::new(1, 1) }, "Bool");
        assert_equivalent(&Expr::Nil { span: Span::new(1, 1) }, "Nil");
        assert_equivalent(&Expr::Ident { name: "x".to_string(), span: Span::new(1, 1) }, "Ident");
    }

    // ── Binary / Unary / Call / Index / Field ─────────────────

    #[test]
    fn derived_matches_binary_unary_call_index_field() {
        // 1 + 2
        assert_equivalent(
            &Expr::Binary {
                left: Box::new(Expr::Number { value: 1.0, span: Span::new(1, 1) }),
                op: BinaryOp::Add,
                right: Box::new(Expr::Number { value: 2.0, span: Span::new(1, 5) }),
                span: Span::new(1, 3),
            },
            "Binary",
        );

        // -x
        assert_equivalent(
            &Expr::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(Expr::Ident { name: "x".to_string(), span: Span::new(1, 2) }),
                span: Span::new(1, 1),
            },
            "Unary",
        );

        // foo(y, "hi", [a, b])
        assert_equivalent(
            &Expr::Call {
                callee: Box::new(Expr::Ident { name: "foo".to_string(), span: Span::new(1, 1) }),
                args: vec![
                    Expr::Ident { name: "y".to_string(), span: Span::new(1, 5) },
                    Expr::String { value: "hi".to_string(), span: Span::new(1, 8) },
                    Expr::Array {
                        elements: vec![
                            Expr::Ident { name: "a".to_string(), span: Span::new(1, 14) },
                            Expr::Ident { name: "b".to_string(), span: Span::new(1, 17) },
                        ],
                        span: Span::new(1, 13),
                    },
                ],
                span: Span::new(1, 3),
            },
            "Call",
        );

        // arr[0].field
        assert_equivalent(
            &Expr::Field {
                object: Box::new(Expr::Index {
                    object: Box::new(Expr::Ident {
                        name: "arr".to_string(),
                        span: Span::new(1, 1),
                    }),
                    index: Box::new(Expr::Number { value: 0.0, span: Span::new(1, 5) }),
                    span: Span::new(1, 4),
                }),
                name: "field".to_string(),
                span: Span::new(1, 7),
            },
            "Index+Field",
        );
    }

    // ── 控制流：If / While / Loop / ForIn ──────────────────────

    #[test]
    fn derived_matches_control_flow() {
        // if (x > 0) { foo(y) } else { nil }
        assert_equivalent(
            &Expr::If {
                condition: Box::new(Expr::Binary {
                    left: Box::new(Expr::Ident { name: "x".to_string(), span: Span::new(1, 5) }),
                    op: BinaryOp::Gt,
                    right: Box::new(Expr::Number { value: 0.0, span: Span::new(1, 9) }),
                    span: Span::new(1, 7),
                }),
                then_branch: vec![Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::Ident {
                        name: "foo".to_string(),
                        span: Span::new(1, 14),
                    }),
                    args: vec![Expr::Ident { name: "y".to_string(), span: Span::new(1, 18) }],
                    span: Span::new(1, 16),
                })],
                else_branch: Some(Box::new(Expr::Nil { span: Span::new(1, 28) })),
                span: Span::new(1, 1),
            },
            "If-else",
        );

        // while (cond) { x }
        assert_equivalent(
            &Expr::While {
                condition: Box::new(Expr::Ident {
                    name: "cond".to_string(),
                    span: Span::new(1, 7),
                }),
                body: vec![Stmt::Expr(Expr::Ident {
                    name: "x".to_string(),
                    span: Span::new(1, 14),
                })],
                span: Span::new(1, 1),
            },
            "While",
        );

        // loop { x }
        assert_equivalent(
            &Expr::Loop {
                body: vec![Stmt::Expr(Expr::Ident {
                    name: "x".to_string(),
                    span: Span::new(1, 8),
                })],
                span: Span::new(1, 1),
            },
            "Loop",
        );

        // for (item in arr) { x }
        assert_equivalent(
            &Expr::ForIn {
                var_name: "item".to_string(),
                iterable: Box::new(Expr::Ident { name: "arr".to_string(), span: Span::new(1, 11) }),
                body: vec![Stmt::Expr(Expr::Ident {
                    name: "x".to_string(),
                    span: Span::new(1, 20),
                })],
                span: Span::new(1, 1),
            },
            "ForIn",
        );
    }

    // ── Break / Continue / Return ─────────────────────────────

    #[test]
    fn derived_matches_jump_expressions() {
        // break x
        assert_equivalent(
            &Expr::Break {
                value: Some(Box::new(Expr::Ident { name: "x".to_string(), span: Span::new(1, 7) })),
                span: Span::new(1, 1),
            },
            "Break with value",
        );
        // break (no value)
        assert_equivalent(&Expr::Break { value: None, span: Span::new(1, 1) }, "Break no value");
        // continue
        assert_equivalent(&Expr::Continue { span: Span::new(1, 1) }, "Continue");
        // return y
        assert_equivalent(
            &Expr::Return {
                value: Some(Box::new(Expr::Ident { name: "y".to_string(), span: Span::new(1, 8) })),
                span: Span::new(1, 1),
            },
            "Return with value",
        );
        // return (no value)
        assert_equivalent(&Expr::Return { value: None, span: Span::new(1, 1) }, "Return no value");
    }

    // ── Fn / Closure ──────────────────────────────────────────

    #[test]
    fn derived_matches_fn_and_closure() {
        // fn add(a, b) { a + b }
        assert_equivalent(
            &Expr::Fn {
                name: Some("add".to_string()),
                params: vec!["a".to_string(), "b".to_string()],
                body: vec![Stmt::Expr(Expr::Binary {
                    left: Box::new(Expr::Ident { name: "a".to_string(), span: Span::new(2, 3) }),
                    op: BinaryOp::Add,
                    right: Box::new(Expr::Ident { name: "b".to_string(), span: Span::new(2, 7) }),
                    span: Span::new(2, 5),
                })],
                span: Span::new(1, 1),
            },
            "Fn",
        );

        // (x) => { x * 2 }
        assert_equivalent(
            &Expr::Closure {
                params: vec!["x".to_string()],
                body: vec![Stmt::Expr(Expr::Binary {
                    left: Box::new(Expr::Ident { name: "x".to_string(), span: Span::new(1, 8) }),
                    op: BinaryOp::Mul,
                    right: Box::new(Expr::Number { value: 2.0, span: Span::new(1, 12) }),
                    span: Span::new(1, 10),
                })],
                span: Span::new(1, 1),
            },
            "Closure",
        );
    }

    // ── Block / Array / Dict / Tuple ──────────────────────────

    #[test]
    fn derived_matches_compound_literals() {
        // { x; "hi"; [1, 2] }
        assert_equivalent(
            &Expr::Block {
                statements: vec![
                    Stmt::Expr(Expr::Ident { name: "x".to_string(), span: Span::new(1, 3) }),
                    Stmt::Expr(Expr::String { value: "hi".to_string(), span: Span::new(1, 6) }),
                    Stmt::Expr(Expr::Array {
                        elements: vec![
                            Expr::Number { value: 1.0, span: Span::new(1, 14) },
                            Expr::Number { value: 2.0, span: Span::new(1, 17) },
                        ],
                        span: Span::new(1, 13),
                    }),
                ],
                span: Span::new(1, 1),
            },
            "Block",
        );

        // { name: "Alice", age: 30 }
        assert_equivalent(
            &Expr::Dict {
                pairs: vec![
                    (
                        "name".to_string(),
                        Expr::String { value: "Alice".to_string(), span: Span::new(1, 8) },
                    ),
                    ("age".to_string(), Expr::Number { value: 30.0, span: Span::new(1, 22) }),
                ],
                span: Span::new(1, 1),
            },
            "Dict",
        );

        // (1, "two", true)
        assert_equivalent(
            &Expr::Tuple {
                elements: vec![
                    Expr::Number { value: 1.0, span: Span::new(1, 2) },
                    Expr::String { value: "two".to_string(), span: Span::new(1, 5) },
                    Expr::Bool { value: true, span: Span::new(1, 12) },
                ],
                span: Span::new(1, 1),
            },
            "Tuple",
        );
    }

    // ── Range / And / Or / NullCoalesce ───────────────────────

    #[test]
    fn derived_matches_binary_like_expressions() {
        // 1..10
        assert_equivalent(
            &Expr::Range {
                start: Box::new(Expr::Number { value: 1.0, span: Span::new(1, 1) }),
                end: Box::new(Expr::Number { value: 10.0, span: Span::new(1, 4) }),
                inclusive: true,
                span: Span::new(1, 2),
            },
            "Range",
        );

        // a && b
        assert_equivalent(
            &Expr::And {
                left: Box::new(Expr::Ident { name: "a".to_string(), span: Span::new(1, 1) }),
                right: Box::new(Expr::Ident { name: "b".to_string(), span: Span::new(1, 5) }),
                span: Span::new(1, 3),
            },
            "And",
        );

        // a || b
        assert_equivalent(
            &Expr::Or {
                left: Box::new(Expr::Ident { name: "a".to_string(), span: Span::new(1, 1) }),
                right: Box::new(Expr::Ident { name: "b".to_string(), span: Span::new(1, 5) }),
                span: Span::new(1, 3),
            },
            "Or",
        );

        // a ?? b
        assert_equivalent(
            &Expr::NullCoalesce {
                left: Box::new(Expr::Ident { name: "a".to_string(), span: Span::new(1, 1) }),
                right: Box::new(Expr::Ident { name: "b".to_string(), span: Span::new(1, 5) }),
                span: Span::new(1, 3),
            },
            "NullCoalesce",
        );
    }

    // ── Try / Out ─────────────────────────────────────────────

    #[test]
    fn derived_matches_try_out() {
        // try { out "error" } keep { cleanup }
        assert_equivalent(
            &Expr::Try {
                body: vec![Stmt::Expr(Expr::Out {
                    value: Box::new(Expr::String {
                        value: "error".to_string(),
                        span: Span::new(1, 11),
                    }),
                    span: Span::new(1, 7),
                })],
                catch_clause: None,
                keep_block: Some(vec![Stmt::Expr(Expr::Ident {
                    name: "cleanup".to_string(),
                    span: Span::new(1, 25),
                })]),
                span: Span::new(1, 1),
            },
            "Try with keep",
        );

        // out err
        assert_equivalent(
            &Expr::Out {
                value: Box::new(Expr::Ident { name: "err".to_string(), span: Span::new(1, 5) }),
                span: Span::new(1, 1),
            },
            "Out",
        );
    }

    // ── Match（含所有模式类型）────────────────────────────────

    #[test]
    fn derived_matches_match_with_all_patterns() {
        // match (x) { 0 => "zero", 1..10 => "range", n => "var", _ => "wild" }
        let expr = Expr::Match {
            scrutinee: Box::new(Expr::Ident { name: "x".to_string(), span: Span::new(1, 7) }),
            arms: vec![
                MatchArm {
                    pattern: MatchPattern::Literal(Expr::Number {
                        value: 0.0,
                        span: Span::new(1, 12),
                    }),
                    body: Expr::String { value: "zero".to_string(), span: Span::new(1, 17) },
                },
                MatchArm {
                    pattern: MatchPattern::Range {
                        start: Box::new(Expr::Number { value: 1.0, span: Span::new(1, 25) }),
                        end: Box::new(Expr::Number { value: 10.0, span: Span::new(1, 28) }),
                        inclusive: true,
                    },
                    body: Expr::String { value: "range".to_string(), span: Span::new(1, 33) },
                },
                MatchArm {
                    pattern: MatchPattern::Variable("n".to_string()),
                    body: Expr::String { value: "var".to_string(), span: Span::new(1, 43) },
                },
                MatchArm {
                    pattern: MatchPattern::Wildcard,
                    body: Expr::String { value: "wild".to_string(), span: Span::new(1, 50) },
                },
            ],
            span: Span::new(1, 1),
        };
        assert_equivalent(&expr, "Match with all pattern types");
    }

    // ── 综合测试：嵌套复杂程序 ───────────────────────────────

    #[test]
    fn derived_matches_comprehensive_program() {
        // {
        //   fn double(x) { x * 2 }
        //   double(if (cond) { [1, 2] } else { [3, 4] })
        //   arr[0] = result
        // }
        let expr = Expr::Block {
            statements: vec![
                Stmt::Expr(Expr::Fn {
                    name: Some("double".to_string()),
                    params: vec!["x".to_string()],
                    body: vec![Stmt::Expr(Expr::Binary {
                        left: Box::new(Expr::Ident {
                            name: "x".to_string(),
                            span: Span::new(2, 3),
                        }),
                        op: BinaryOp::Mul,
                        right: Box::new(Expr::Number { value: 2.0, span: Span::new(2, 7) }),
                        span: Span::new(2, 5),
                    })],
                    span: Span::new(1, 3),
                }),
                Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::Ident {
                        name: "double".to_string(),
                        span: Span::new(3, 3),
                    }),
                    args: vec![Expr::If {
                        condition: Box::new(Expr::Ident {
                            name: "cond".to_string(),
                            span: Span::new(3, 14),
                        }),
                        then_branch: vec![Stmt::Expr(Expr::Array {
                            elements: vec![
                                Expr::Number { value: 1.0, span: Span::new(3, 25) },
                                Expr::Number { value: 2.0, span: Span::new(3, 28) },
                            ],
                            span: Span::new(3, 24),
                        })],
                        else_branch: Some(Box::new(Expr::Array {
                            elements: vec![
                                Expr::Number { value: 3.0, span: Span::new(3, 39) },
                                Expr::Number { value: 4.0, span: Span::new(3, 42) },
                            ],
                            span: Span::new(3, 38),
                        })),
                        span: Span::new(3, 10),
                    }],
                    span: Span::new(3, 9),
                }),
                Stmt::Assign {
                    target: AssignTarget::Index {
                        object: Box::new(Expr::Ident {
                            name: "arr".to_string(),
                            span: Span::new(4, 3),
                        }),
                        index: Box::new(Expr::Number { value: 0.0, span: Span::new(4, 7) }),
                    },
                    value: Expr::Ident { name: "result".to_string(), span: Span::new(4, 12) },
                    span: Span::new(4, 9),
                },
            ],
            span: Span::new(1, 1),
        };
        assert_equivalent(&expr, "Comprehensive program");
    }
}
