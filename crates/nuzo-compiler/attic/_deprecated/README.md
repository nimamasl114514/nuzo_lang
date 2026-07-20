# Deprecated AST Direct Compilation Path Modules

These modules were part of the **AST direct compilation path** (`compile_program` /
`compile_stmt` / `compile_expr`), which was deprecated and removed in v0.3.0.

All compilation now goes through the **IR path**:
`Source → AST → IR (IrBuilder) → Bytecode (CodeGenerator)`

## P2.6 迁移说明（2026-07-14）

这些文件原位于 `crates/backend/nuzo_compiler/src/_deprecated/`，
现移动到 `crates/backend/nuzo_compiler/attic/_deprecated/`。

**移动原因**：原位置在 `src/` 下会让 Rust 编译器扫描这些文件（虽然无 `mod` 引用不参与编译），
随时间积累尘埃。移动到 `attic/` 后明确表示"档案室"，与 `src/` 隔离。

**保留原因**：作为历史参考，记录 AST 直编译路径的设计与实现。如需恢复某模块，
可移回 `src/` 并在 `lib.rs` 添加 `mod` 引用。

## Removed Modules

| File | Lines | Original Responsibility |
|------|-------|------------------------|
| `expressions.rs` | 1,910 | Expression compilation |
| `statements.rs` | 629 | Statement compilation |
| `functions.rs` | 846 | Function/closure/array/dict/range compilation |
| `helpers.rs` | 86 | Bytecode emission primitives |
| `macros.rs` | 189 | Bytecode emission macros |
| `control_stack.rs` | 689 | Loop control stack |
| `scope_management.rs` | 312 | Scope management |
| `string_intern.rs` | 220 | Constant pool management |
| `patch_list.rs` | 94 | Jump address backpatching |
| `lsra.rs` | 261 | LSRA integration (AST-path-only) |
| **Total** | **5,236** | |

## Migration

Replace any usage of `Compiler::compile_program()` or `Compiler::compile_stmt()`
with `Compiler::compile()`:

```rust
// BEFORE (AST path — removed)
let mut compiler = Compiler::builder().source(source).build();
compiler.compile_program(&ast)?;
let chunk = compiler.into_chunk();

// AFTER (IR path)
let chunk = Compiler::compile(source)?;
```

These files are retained for reference only and are not compiled.
