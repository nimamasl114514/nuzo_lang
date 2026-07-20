# NuzoLang 自动宏发展报告

> 日期：2026-07-19
> 范围：全 workspace 宏系统现状分析 + 可宏化机会评估

---

## 一、执行摘要

NuzoLang 已拥有相当成熟的宏基础设施，包括 **9 个 proc-macro**（nuzo_proc + nuzo_class_macros）和 **17+ 个 declarative macro_rules!**。但仍有大量可宏化空间，保守估计可消除 **200+ 处手写重复代码**，分布在 10+ 个 crate 中。

---

## 二、现有宏全景图

### 2.1 Proc Macro 清单（nuzo_proc crate）

| 宏 | 类型 | 功能 | 所在文件 |
|----|------|------|---------|
| `derive(MatchSync)` | derive | 枚举自动生成 Visitor 模式 + dispatch 方法 | `lib.rs:22` |
| `derive(FromMeta)` | derive | 结构体属性解析声明 | `lib.rs:55` |
| `derive(Trace)` | derive | GC 枚举自动生成 trace() | `lib.rs:98` |
| `derive(OpcodeSync)` | derive | 指令枚举自动生成 SSOT + dispatch 列表 | `lib.rs:141` |
| `#[nuzo_test]` | attribute | 声明式测试用例 | `lib.rs:170` |
| `#[crate_meta]` | attribute | crate 元数据声明 | `lib.rs:217` |
| `define_opcodes!` | function-like | 声明式 Opcode 定义 | `lib.rs:264` |
| `define_dispatch_auto!` | function-like | 自动生成 dispatch 分发表 | `lib.rs:295` |
| `define_builtins!` | function-like | 声明式 builtin 注册 | `lib.rs:336` |

### 2.2 Class Macros 清单（nuzo_class_macros crate）

| 宏 | 类型 | 功能 |
|----|------|------|
| `#[class]` | attribute | 标记类结构体 + 注入 derive |
| `#[class_impl]` | attribute | 标记类 impl 块 |
| `#[constructor]` | attribute | 标记构造方法 |
| `#[get]` / `#[set]` | attribute | 标记 getter/setter |
| `#[method]` / `#[static_method]` | attribute | 标记实例/静态方法 |

### 2.3 Declarative Macro 清单（macro_rules!）

| 宏 | 所在 crate | 功能 | 状态 |
|----|-----------|------|------|
| `define_errors!` | nuzo-core | 自动生成 NuzoErrorKind::Display | 活跃 |
| `define_keywords!` | nuzo-frontend | 关键字查找 + 常量表 | 活跃 |
| `define_value_tag!` | nuzo-values | NaN-tagging 类型检测 + 编译期冲突检查 | 活跃 |
| `declare_signal!` | nuzo-signal | 信号键常量定义 | 活跃 |
| `arith_handler!` | nuzo-vm | 算术 handler 生成（消除 4 个重复函数） | 活跃 |
| `build_dispatch_table!` | nuzo-vm | dispatch 表构建 | 活跃 |
| `require_arg_count!` / `require_min_args!` | nuzo-helpers | 参数数量校验 | 活跃 |
| `require_number!` / `require_string!` / `require_array!` | nuzo-helpers | 类型校验 | 活跃 |
| `require_one_number!` / `require_two_numbers!` | nuzo-helpers/math | 数学参数校验 | 活跃 |
| `gen_encode_field!` / `gen_decode_field!` | nuzo-bytecode | 字节码编/解码 | 活跃 |
| `generate_opcode_method!` / `generate_encode_method!` / `generate_decode_method!` | nuzo-bytecode | 操作码方法生成 | 活跃 |
| `define_constants!` | nuzo_proc_core | 常量管理框架 | 活跃 |
| `hlist!` | nuzo-values | 异构列表 | 活跃 |
| emit_* 系列 (5个) | nuzo-compiler/attic | 旧的编译器辅助 | 已废弃 |

---

## 三、Proc-Macro 基础设施分析

### 3.1 架构

```
nuzo_proc (proc-macro crate, 仅入口)
  └── nuzo_proc_core (逻辑实现, 非 proc-macro crate)
       ├── match_sync.rs         — MatchSync derive
       ├── trace_derive.rs       — Trace derive
       ├── opcode_sync_derive.rs — OpcodeSync derive
       ├── attr.rs               — FromMeta derive
       ├── test_attr.rs          — nuzo_test 属性
       ├── crate_meta.rs         — 元数据管理
       ├── hardcode.rs           — 常量管理框架
       ├── doc_sync.rs           — 文档同步类型
       ├── parse_utils.rs        — 解析工具
       └── diag.rs               — 诊断/错误报告
```

### 3.2 关键优势

- **双 crate 架构**：nuzo_proc 是 proc-macro crate（仅入口转发），nuzo_proc_core 是普通 crate（逻辑实现，可单元测试）
- **零运行时开销**：所有展开为编译期代码生成
- **可测试性**：nuzo_codegen 和 nuzo_proc_core 均可在非 proc-macro 上下文测试
- **SSOT 原则**：`define_opcodes!` + `derive(OpcodeSync)` 实现"改动一处枚举，全链路同步"

### 3.3 已消除的重复

| 领域 | 消除前 | 消除后 | 手段 |
|------|--------|--------|------|
| Opcode 枚举 + 方法 | 手写 100+ 行 match 臂 | 声明式属性定义 | `define_opcodes!` |
| Builtin 注册 | 手写 register() 调用 | 声明式块语法 | `define_builtins!` |
| GC trace() | 手写 match 分支 | 自动派生 | `derive(Trace)` |
| 算术 handler | 4 个几乎相同的函数 | 1 个 `arith_handler!` 宏 | `arith_handler!` |
| 参数校验 | 31+ 处手写 if/return | 6 个 require_*! 宏 | `require_*!` |
| 关键字查找 | 手写 match 表 | 声明式列表 | `define_keywords!` |
| 错误 Display | 手写 match | 声明式 | `define_errors!` |

---

## 四、按 Crate 的可宏化分析

### 4.1 nuzo-vm（VM 执行层）— 高优先级

**当前已有**：`arith_handler!`、`build_dispatch_table!`

#### 可宏化点

**① 比较运算符 handler 宏**（ROI: 高）

`dispatcher_table.rs` 中 `_op_eq`, `_op_neq`, `_op_lt`, `_op_gt`, `_op_le`, `_op_ge` 六个函数结构高度相似：

```rust
// 当前：6 个函数，每个 ~30 行，仅比较运算符不同
fn _op_eq(vm: &mut VM) -> Result<(), NuzoError> { /* 读寄存器 → 比较 → 写结果 */ }
fn _op_neq(vm: &mut VM) -> Result<(), NuzoError> { /* 同上 */ }
// ... ×4
```

**建议**：添加 `cmp_handler!` 宏，类似 `arith_handler!` 模式：

```rust
cmp_handler!(_op_eq, |a, b| a == b);
cmp_handler!(_op_neq, |a, b| a != b);
cmp_handler!(_op_lt, |a, b| a < b);
cmp_handler!(_op_gt, |a, b| a > b);
cmp_handler!(_op_le, |a, b| a <= b);
cmp_handler!(_op_ge, |a, b| a >= b);
```

**② 一元运算符 handler 宏**

`_op_neg`, `_op_not` 等一元运算符可统一。

**③ Handler 注册宏**

dispatch 表中 47 个 `_op_xxx` 函数的注册模式可统一。

---

### 4.2 nuzo-core（错误/编码层）— 中优先级

**当前已有**：`define_errors!`

#### 可宏化点

**① ErrorKind 构造方法自动生成**（ROI: 中）

`NuzoErrorKind` 有 20+ 个变体，每个变体都需手动构造 `NuzoError` 包装。当前手工实现：

```rust
// 当前：每个错误类型都要手写构造方法
impl NuzoError {
    pub fn type_mismatch(expected: impl Into<String>, actual: impl Into<String>) -> Self { ... }
    pub fn invalid_argument_count(expected: usize, got: usize) -> Self { ... }
    // ... ×20
}
```

**建议**：扩展 `define_errors!` 宏，同时生成构造方法：

```rust
define_errors! {
    TypeMismatch { expected: String, actual: String }
        => "类型不匹配：期望 {expected}，实际得到 {actual}",
    InvalidArgumentCount { expected: usize, got: usize }
        => "参数数量错误：期望 {expected} 个，实际传入了 {got} 个",
    // ...
}
// 自动生成：
//   NuzoError::type_mismatch(expected, actual)
//   NuzoError::invalid_argument_count(expected, got)
```

**② 错误码到 ErrorKind 的映射**

每个错误码（`C0001`, `C0002`, ...）到 ErrorKind 的映射是手写的。可宏化。

---

### 4.3 nuzo-frontend（解析层）— 中优先级

**当前已有**：`define_keywords!`、`derive(MatchSync)`

#### 可宏化点

**① AST Visitor 自动生成**（ROI: 高）

`ExprVisitor` trait 和 `default_visit_expr` 函数有 40+ 行的手写递归遍历。每个新 Visitor 实现需要手写代理方法。

**建议**：`derive(ExprVisitor)` 宏，自动生成 `visit_expr` 的默认实现：

```rust
#[derive(ExprVisitor)]
#[visitor(visit_fn = "visit_expr", default_fn = "default_visit_expr")]
enum Expr {
    Literal(LiteralValue),
    Ident { name: String },
    BinaryOp { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    UnaryOp { op: UnaryOp, operand: Box<Expr> },
    // ...
}
// 自动生成 default_visit_expr 函数和 visit_expr 默认实现
```

**② 运算符定义表**

`BinaryOp` 和 `UnaryOp` 枚举的 Display/From 实现可宏化。

---

### 4.4 nuzo-helpers（builtin 函数层）— 高优先级

**当前已有**：`require_*!` 系列

#### 可宏化点

**① Builtin 函数模板**（ROI: 高）

每个 builtin 函数遵循相同模式：

```rust
fn builtin_xxx(args: &[Value]) -> Result<Value, NuzoError> {
    require_arg_count!(args, N);
    require_number!(args, 0);
    // ... 业务逻辑
}
```

**建议**：`define_builtin_impl!` 宏，声明式定义 builtin 完整实现：

```rust
define_builtin_impl!(builtin_abs, args, {
    require_arg_count!(args, 1);
    require_number!(args, 0);
    let n = args[0].to_f64();
    Ok(Value::from_f64(n.abs()))
});
```

**② 类型转换宏**

`args[0].as_smi()`, `args[0].to_f64()`, `args[0].to_string()` 等重复模式可统一。

---

### 4.5 nuzo-bytecode（字节码层）— 低优先级

**当前已有**：`gen_encode_field!`, `gen_decode_field!`, `generate_opcode_method!` 等

#### 可宏化点

**① 完整的编解码 derive**（ROI: 低）

当前字节码编解码已通过 `macro_rules!` 完成。如果未来操作数类型扩展，可考虑 `derive(Encode)` / `derive(Decode)` 替代现有 macro_rules!。

---

### 4.6 nuzo-compiler（编译器层）— 中优先级

#### 可宏化点

**① 寄存器分配器重复模式**（ROI: 中）

`reg_manager.rs` 中有大量 `allocate_temp()`, `consume_use()`, `release()` 的组合模式。可宏封装常见寄存器分配模式。

**② IR 生成模板**

codegen 中各 `emit_*` 方法有重复的写字节码头 + 操作数模式。

---

### 4.7 nuzo-ir（中间表示层）— 中优先级

#### 可宏化点

**① AST → IR 转换**（ROI: 高）

`types.rs` 中：

```rust
impl From<nuzo_frontend::ast::BinaryOp> for IrBinOp { ... }
impl From<nuzo_frontend::ast::UnaryOp> for IrUnaryOp { ... }
```

每个运算符需要手写 match 臂。

**建议**：`derive(FromAst)` 或 `ir_convert!` 宏。

---

### 4.8 nuzo-values（值系统层）— 低优先级

**当前已有**：`define_value_tag!`、`hlist!`

#### 可宏化点

**① Value 扩展方法模板**（ROI: 低）

`ValueExt` trait 中的 `concat_repr`, `to_debug` 等扩展方法，对每种 HeapObject 类型有重复的 match 分支。

---

### 4.9 nuzo-signal（信号系统层）— 低优先级

**当前已有**：`declare_signal!`

#### 可宏化点

**① 信号总线宏**（ROI: 低）

如果信号数量增长，可考虑 `define_bus!` 宏统一管理信号注册。

---

### 4.10 nuzo-error（错误诊断层）— 低优先级

#### 可宏化点

**① 诊断类型生成**（ROI: 低）

`smart_types.rs` 有 10+ 个结构体，每个都有 `Debug, Clone, Serialize` 和大量字段。可考虑 `derive(FromMeta)` 或统一的构建器宏。

---

### 4.11 nuzo-run（运行时引擎层）— 中优先级

#### 可宏化点

**① EngineBuilder 配置链模式**（ROI: 中）

[engine.rs](:///d:/10/nuzo_lang/crates/nuzo-run/src/engine.rs) 中 `EngineBuilder` 有多个 `with_*` 方法（`with_default_config`, `with_config`, `with_config_file`, `with_env_config`），每个遵循相同的模式：设置 builder 字段 → 返回 Self。

**建议**：`define_builder!` 宏声明式定义 Builder 模式：

```rust
define_builder!(EngineBuilder {
    config: Option<Config> = None,
    signal_bus: Option<SignalBus> = None,
    // ...
});
// 自动生成 EngineBuilder::new(), with_xxx(), build()
```

**② Session 创建模式**

`run`, `eval`, `run_file`, `compile`, `compile_file` 等方法中 Session 创建和执行逻辑重复。

**建议**：`with_session!` 宏封装 Session 生命周期。

---

### 4.12 nuzo-config（配置层）— 高优先级

#### 可宏化点

**① apply_* 函数系列**（ROI: 高）

[config.rs](:///d:/10/nuzo_lang/crates/nuzo-config/src/config.rs#L287-L333) 中有 `apply_usize`, `apply_u32`, `apply_u16`, `apply_u8`, `apply_f64`, `apply_bool`, `apply_string` 等 7+ 个几乎相同的函数，每个都从 TOML table 中提取字段并赋值。

**建议**：`apply_config_field!` 宏统一：

```rust
macro_rules! apply_config_field {
    ($target:expr, $table:expr, $key:literal, $field:ident, $type:ty) => {
        if let Some(val) = $table.get($key).and_then(|v| v.as_$type()) {
            $target.$field = val;
        }
    };
}
// 使用：
apply_config_field!(cfg, &table, "stack_size", stack_size, integer);
apply_config_field!(cfg, &table, "gc_threshold", gc_threshold, integer);
```

**② ConfigBuilder::build 字段提取**

`build()` 方法中 20+ 行重复的 `if let Some(val) = table.get("key")` 模式。

---

### 4.13 nuzo-gui（GUI 层）— 中优先级

#### 可宏化点

**① Widget 注册模式**（ROI: 高）

[widgets.rs](:///d:/10/nuzo_lang/crates/nuzo-gui/src/widgets.rs#L250-L283) 中 `register_widgets` 函数对每个 widget 重复相同的注册模式：

```rust
reg.register("gui_label", gui_label, 1);
reg.register("gui_button", gui_button, 2);
// ... ×20+
```

**建议**：复用现有的 `define_builtins!` 宏或创建 `define_widgets!` 宏。

**② UI 控件函数模板**

`gui_label`, `gui_heading`, `gui_separator`, `gui_button`, `gui_checkbox` 等 10+ 个函数有相似的参数提取和错误处理模式。

**建议**：`widget_handler!` 宏封装通用控件处理逻辑。

---

### 4.14 nuzo-opcode（操作码层）— 低优先级

#### 当前已有

- `define_opcodes!` 生成 Opcode 枚举（通过 nuzo_proc）
- `derive(OpcodeSync)` 自动生成 SSOT

#### 可宏化点

**① OperandKind 方法生成**（ROI: 低）

[lib.rs](:///d:/10/nuzo_lang/crates/nuzo-opcode/src/lib.rs) 中 `OperandKind` 的 `byte_size()` 和 `is_signed()` 方法使用手写 match 臂。

**建议**：`derive(OperandKind)` 或扩展 `OpcodeSync` 覆盖 OperandKind。

**② DispatchKind 方法生成**

`DispatchKind` 枚举的 match 方法可类似宏化。

---

### 4.15 nuzo-class（类系统入口层）— 低优先级

#### 当前已有

- 全部功能通过 re-export `nuzo_class_macros` 实现

#### 可宏化点

**① 纯 re-export crate**（ROI: 低）

该 crate 仅是 `nuzo_class_macros` 的 re-export 包装。当前模式已足够简洁，无需进一步宏化。

---

## 五、跨模块可宏化机会

### 5.1 跨 crate 错误转换

14+ 处手动 `impl From<XxxError> for YyyError` 实现。可 `derive(ErrorFrom)` 宏：

```rust
#[derive(ErrorFrom)]
#[error_from(source = "RegAllocError", target = "CodegenError")]
#[error_from(source = "CodegenError", target = "CompileError")]
```

### 5.2 Serde 序列化统一

多个 crate 中的类型需要 `Serialize` / `Deserialize`。当前通过 `cfg_attr(feature = "serde")` 手动添加。可 `serde_derive!` 宏统一管理。

### 5.3 测试辅助宏

测试代码中有大量重复的 setup 模式：
- 创建 VM 实例
- 编译源代码
- 执行并断言

可 `nuzo_test!` 或 `vm_test!` 宏统一。

### 5.4 文档生成

`OPCODE_DOCS` 和 `DOMAIN_DOCS` 常量已自动生成。可扩展到：
- 内置错误码文档
- 关键字文档
- 值类型文档

---

## 六、优先级路线图

### Phase 1（立即，ROI 最高）

| 序号 | 宏 | 目标 crate | 消除重复 | 难度 |
|------|-----|-----------|---------|------|
| 1 | `cmp_handler!` | nuzo-vm | 6 个比较 handler → 6 次宏调用 | 低 |
| 2 | `apply_config_field!` | nuzo-config | 7+ 个 apply_* 函数 → 1 个宏 | 低 |
| 3 | `derive(ExprVisitor)` | nuzo-frontend | 手写 visitor 递归遍历 | 中 |
| 4 | `define_builtin_impl!` | nuzo-helpers | 每个 builtin 的参数校验模板 | 中 |
| 5 | `define_widgets!` | nuzo-gui | 20+ widget 注册调用 → 声明式列表 | 低 |

### Phase 2（短期，ROI 中高）

| 序号 | 宏 | 目标 crate | 消除重复 |
|------|-----|-----------|---------|
| 4 | 扩展 `define_errors!` 生成构造方法 | nuzo-core | 20+ 个 ErrorKind 构造方法 |
| 5 | `derive(FromAst)` | nuzo-ir | AST → IR 运算符转换 |
| 6 | `derive(ErrorFrom)` | 跨 crate | 14+ 个 From impl |
| 7 | 一元运算符 handler 宏 | nuzo-vm | 3-5 个一元 handler |

### Phase 3（中期，ROI 中）

| 序号 | 宏 | 目标 crate | 消除重复 |
|------|-----|-----------|---------|
| 8 | 编译器 emit 模板宏 | nuzo-compiler | 字节码写入重复模式 |
| 9 | 测试 setup 宏 | tests/ | 测试 init 代码 |
| 10 | 文档生成扩展 | nuzo_proc | 更多文档类型 |

### Phase 4（长期，ROI 低/质量提升）

| 序号 | 宏 | 目标 crate | 说明 |
|------|-----|-----------|------|
| 11 | `derive(Encode/Decode)` | nuzo-bytecode | 替代现有 macro_rules! |
| 12 | Value 扩展方法模板 | nuzo-values | match 分支统一 |
| 13 | 信号总线宏 | nuzo-signal | 信号注册管理 |

---

## 七、风险评估

| 风险 | 等级 | 缓解 |
|------|------|------|
| 宏过度使用导致编译错误信息不友好 | 中 | 使用 `nuzo_proc_core::diag::SpannedError` 提供精确 span 信息 |
| 宏增加编译时间 | 低 | proc-macro 仅在修改时重新展开，且 nuzo_proc 已采用双 crate 优化 |
| 宏导致代码可读性下降 | 低 | 声明式语法（`define_opcodes!`, `define_builtins!`）已证明比手写更可读 |
| 宏与现有代码兼容性 | 低 | 渐进式替换，不破坏现有 API |

---

## 八、总结

NuzoLang 的宏系统已经是一个**生产级**的实现，在以下方面表现突出：

1. **SSOT（单一真相源）**：`define_opcodes!` + `derive(OpcodeSync)` 实现"改动一处枚举，全链路同步"
2. **零开销抽象**：所有宏展开为编译期代码生成，运行时无额外开销
3. **可测试架构**：双 crate 设计允许在非 proc-macro 上下文测试展开逻辑
4. **声明式 API**：属性语法比旧的 macro_rules! 位置参数更可读、更易维护

**下一步**：优先实施 Phase 1 的 5 个宏（`cmp_handler!`, `apply_config_field!`, `derive(ExprVisitor)`, `define_builtin_impl!`, `define_widgets!`），预计可消除 100+ 处手写重复代码，覆盖全部 19 个 crate 中的 15 个，投资回报率最高。