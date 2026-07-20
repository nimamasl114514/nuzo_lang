# Nuzo Lang 语言规范 (Language Specification)

> 版本：0.6.0 | 状态：按当前实现描述（非提案）
>
> 本文档描述 Nuzo Lang 当前版本的真实语法、类型系统与语义行为。实现以 `crates/nuzo-frontend`（lexer + parser + AST）和 `crates/nuzo-vm`（运行时）为准。

---

## 1. 概述

### 1.1 语言定位

Nuzo Lang 是一门用 Rust 实现的轻量级动态脚本语言，核心特征：

- **动态类型**：变量类型在运行时确定，无静态检查阶段（可选 `.nuzo.stub` 类型存根）。
- **NaN-tagged 值系统**：所有值统一编码为 8 字节，小整数（Smi）/ 浮点 / 布尔 / nil 内联，复合类型走堆索引。
- **寄存器机 VM**：编译产物为寄存器字节码（51 opcodes），非栈机。
- **表达式优先**：绝大多数构造既是语句也是表达式，可出现在值位置。
- **双语关键字**：每个关键字同时支持英文与中文形式（如 `if` / `如果`、`fn` / `函数`）。
- **CJK 友好**：lexer / parser 全程 Unicode 感知，标识符允许中文。

### 1.2 设计哲学

| 原则 | 含义 |
|------|------|
| 表达式优先 | `if`、`match`、闭包都能作为值传递 |
| 双语关键字 | 降低中文用户入门门槛，不强制英文 |
| 动态类型 + 可选存根 | 默认无类型负担，需要时可加 `.nuzo.stub` 文件 |
| 寄存器字节码 | 比栈机更紧凑，减少 dispatch 次数 |
| 错误保留位置 | 编译/运行错误必须带源码位置，禁止降级为 `C0000` |

---

## 2. 词法规范

### 2.1 关键字（Keyword）

支持英文 / 中文双语，语义等价：

| 类别 | 英文 | 中文 |
|------|------|------|
| 条件 | `if` `else` | `如果` `否则` |
| 循环 | `while` `for` `in` `loop` `break` `continue` | `当` `遍历` `在` `循环` `跳出` `继续` |
| 函数 | `fn` `return` | `函数` `返回` |
| 异常 | `try` `catch` `out` `keep` | `尝试` `捕获` `抛出` `始终` |
| 模式匹配 | `match` | `匹配` |
| 字面量 | `true` `false` `nil` | `真` `假` `空` |
| 逻辑 | `and` `or` | `并且` `或者` |
| 模块 | `import` `as` | `导入` `作为` |
| 求值 | `lazy` | `懒` |

> 注：`and` / `or` 不短路；短路版本为 `&&` / `||`。

### 2.2 运算符

| 类别 | 运算符 |
|------|--------|
| 算术 | `+` `-` `*` `/` `%` `**`（右结合幂运算） |
| 比较 | `==` `!=` `<` `>` `<=` `>=` |
| 逻辑 | `&&` `\|\|` `!` |
| 赋值 | `=` `+=` `-=` `*=` `/=` |
| 范围 | `..`（含右端） `..<`（不含右端） |
| 管道 | `\|>`（左到右函数链式调用） |
| 空值合并 | `??`（左侧 nil 时取右侧） |
| 箭头 | `=>`（lambda 表达式） |

### 2.3 字面量

| 类型 | 语法 | 示例 |
|------|------|------|
| 整数 | 十进制字面量 | `42`、`-7`、`0` |
| 浮点 | 含 `.` 或 `e` | `3.14`、`1e10`、`-0.5` |
| 字符串 | 双引号或单引号 | `"hello"`、`'world'` |
| 布尔 | `true` / `false` | `true` |
| nil | `nil` | `nil` |
| 数组 | `[expr, expr, ...]` | `[1, 2, 3]` |
| 字典 | `{key: value, ...}` | `{"a": 1, "b": 2}` |

**Smi 范围**：整数 [-2^47, 2^47) 内联编码为 Smi，溢出自动提升为 `f64`。

**浮点 -0.0**：内部必须归一化为 `0.0`（normalize），避免 NaN-tagged 位模式冲突。

---

## 3. 语法规范

### 3.1 EBNF 概览

```ebnf
program        = { statement } ;
statement      = let_stmt | const_stmt | fn_stmt
               | if_stmt | while_stmt | for_stmt
               | match_stmt | try_stmt
               | return_stmt | break_stmt | continue_stmt
               | import_stmt | expr_stmt ;

let_stmt       = ( "let" | "变量" ) IDENT "=" expr ;
const_stmt     = ( "const" ) IDENT "=" expr ;
fn_stmt        = ( "fn" | "函数" ) IDENT "(" params ")" block ;
params         = [ IDENT { "," IDENT } ] ;

if_stmt        = ( "if" | "如果" ) expr block
                 [ ( "else" | "否则" ) ( if_stmt | block ) ] ;
while_stmt     = ( "while" | "当" ) expr block ;
for_stmt       = ( "for" | "遍历" ) IDENT ( "in" | "在" ) expr block ;
match_stmt     = ( "match" | "匹配" ) expr "{" { match_arm } "}" ;
match_arm      = pattern "=>" expr [ "," ] ;

try_stmt       = ( "try" | "尝试" ) block
                 ( "catch" | "捕获" ) [ IDENT ] block
                 [ ( "keep" | "始终" ) block ] ;

expr           = pipeline_expr ;
pipeline_expr  = null_coalesce_expr { "|>" null_coalesce_expr } ;
null_coalesce  = logic_or { "??" logic_or } ;
block          = "{" { statement } "}" ;
```

### 3.2 注释

```nuzo
// 单行注释：从 // 到行尾

/* 块注释：可跨行，
   不支持嵌套 */

/// 文档注释：用于 builtin / 模块说明
/// 可多行，会被文档工具收集
```

### 3.3 变量绑定

```nuzo
let x = 10;            // 可变绑定
let y = "hello";
const PI = 3.14159;    // 不可变（编译期常量优化）

x = x + 1;             // 重新赋值
x += 5;                // 复合赋值
```

### 3.4 函数定义

#### 3.4.1 命名函数

```nuzo
fn add(a, b) {
    return a + b;
}

// 中文等价
函数 加(a, b) {
    返回 a + b;
}
```

#### 3.4.2 闭包 / Lambda

```nuzo
let square = fn(x) { return x * x; };
let double = |x| x * 2;          // 箭头 lambda（单表达式）
let compose = |f, g| |x| f(g(x)); // 高阶
```

#### 3.4.3 捕获模式

闭包捕获外层变量有两种模式（由编译器根据使用方式自动选择）：

| 模式 | 语义 | 实现 |
|------|------|------|
| `ByValue` | 不可变捕获：闭包创建时复制值 | `HeapObject::Box(Value)` |
| `ByBox` | 可变捕获：通过堆 Box 共享变量 | `HeapObject::Box(Value)`，多闭包指向同一 Box |

```nuzo
let counter = 0;
let inc = fn() { counter = counter + 1; };  // ByBox，可改外层
inc();
inc();
// counter == 2
```

### 3.5 控制流

#### 3.5.1 if / else

```nuzo
let grade = if score >= 90 { "A" }
            else if score >= 60 { "B" }
            else { "F" };
```

`if` 是表达式，返回匹配分支的最后一行。

#### 3.5.2 while

```nuzo
let i = 0;
while i < 10 {
    if i == 5 { break; }
    if i % 2 == 0 { i += 1; continue; }
    print(i);
    i += 1;
}
```

#### 3.5.3 for-in

```nuzo
for x in [1, 2, 3] {
    print(x);
}

for k in {1: "a", 2: "b"} {
    print(k);              // 遍历键
}

for n in 1..5 {            // 范围 [1,2,3,4,5]
    print(n);
}

for n in 1..<5 {           // 范围 [1,2,3,4]
    print(n);
}
```

### 3.6 模式匹配

```nuzo
match x {
    0 => "zero",
    1 | 2 | 3 => "small",
    n if n < 100 => "medium",   // 带守卫
    _ => "large",               // 通配
}
```

`match` 是表达式，返回匹配分支的值。`_` 匹配任意值。分支间用逗号分隔。

### 3.7 错误处理

#### 3.7.1 try-catch-keep-out

```nuzo
try {
    let data = read_file(path);
    process(data);
} catch err {
    print("error: " + err.message);
} keep {
    // 始终执行（类似 finally）
    cleanup();
}

out "something went wrong";   // 抛出异常
out Exception({                // 抛出带元数据的异常
    message: "div by zero",
    code: "DivisionByZero",
});
```

异常对象结构（`HeapObject::Exception`）：
- `message`：错误消息（必需）
- `code`：错误码字符串，如 `"TypeError"`（必需）
- `stack`：调用栈帧（VM 自动填充）
- `location`：`{file, line, column}`（VM 自动填充）
- `context`：用户附加上下文字典
- `cause`：可选，异常链前一个异常

#### 3.7.2 空值合并 `??`

```nuzo
let name = user.name ?? "anonymous";   // user.name 为 nil 时取 "anonymous"
let port = config.port ?? 8080;
```

### 3.8 管道操作符 `|>`

```nuzo
[1, 2, 3]
    |> |xs| map(xs, |x| x * 2)
    |> |xs| filter(xs, |x| x > 2)
    |> |xs| reduce(xs, 0, |a, b| a + b);
// 等价于 reduce(filter(map([1,2,3], |x| x*2), |x| x>2), 0, |a,b| a+b)
```

左到右链式调用，左侧表达式作为右侧函数的第一个参数。

---

## 4. 类型系统

### 4.1 动态类型

Nuzo 是动态类型语言：

- 变量声明无需类型注解
- 类型在运行时由 `Value` 的 NaN-tag 决定
- 无静态类型检查阶段

### 4.2 可选类型存根 `.nuzo.stub` (v7)

为 IDE 补全与文档工具提供可选类型信息：

```nuzo
// foo.nuzo
fn add(a, b) { return a + b; }
```

```nuzo
// foo.nuzo.stub
fn add(a: i64, b: i64) -> i64;
```

存根文件特性：

- 不影响运行时行为
- 由外部工具（如 `nuzo_callgraph` 或 LSP）消费
- 当前 v7 版本：仅函数签名 + 内置类型（`i64` / `f64` / `string` / `bool` / `nil` / `Array<T>` / `Map<K,V>`）
- 不支持用户自定义类型注解（class 类型走 `nuzo_class` crate 的 Rust 端定义）

---

## 5. 内置类型

### 5.1 NaN-tagged 表示

所有值统一为 8 字节，由 `nuzo_values::Value` 表示：

| 类型 | 表示方式 | 示例 |
|------|---------|------|
| `nil` | 特殊 NaN 位模式 | `nil` |
| `bool` | 特殊 NaN 位模式 | `true` / `false` |
| `i64` (Smi) | 内联，范围 [-2^47, 2^47) | `42` |
| `f64` | 直接 NaN-tagged | `3.14` |
| `string` | 堆索引 → `HeapObject::String`?（实际：字符串走堆池） | `"hello"` |
| `array` | 堆索引 → `HeapObject::Array(Vec<Value>)` | `[1, 2, 3]` |
| `dict` | 堆索引 → `HeapObject::Dict(NuzoDict)` | `{"a": 1}` |
| `range` | 堆索引 → `HeapObject::Range { start, end, range_end }` | `1..5` |
| `closure` | 堆索引 → `HeapObject::Closure { prototype, captured, parent_env }` | `fn(x){x}` |
| `builtin` | 堆索引 → `HeapObject::BuiltinFn { name, arity, func }` | `print` |
| `exception` | 堆索引 → `HeapObject::Exception { ... }` | try-catch 中的 `err` |
| `box` | 堆索引 → `HeapObject::Box(Value)` | ByBox 闭包捕获 |

**构造规则**：必须使用 `Value::from_number()` / `Value::from_bool()` / `Value::from_nil()` 等构造器。**禁止 transmute 或手写位模式**。

### 5.2 HeapObject 完整变体

见 `crates/nuzo-values/src/heap.rs:127`：

```rust
pub enum HeapObject {
    Array(Vec<Value>),
    Dict(NuzoDict),
    Range { start: f64, end: f64, range_end: RangeEnd },
    Closure { prototype: Arc<FunctionPrototype>, captured: Vec<CapturedVar>, parent_env: Option<Arc<HeapObject>> },
    Box(Value),
    BuiltinFn { name: String, arity: usize, func: BuiltinFnPtr },
    Exception { message, code, stack, location, context, cause },
}
```

新增 HeapObject 变体**必须实现 `HeapObjectOps` trait**（含 `trace_gc_refs`），否则 GC 时内存损坏。

---

## 6. 内置函数分类

builtin 函数注册于 `crates/nuzo-helpers/src/builtins.rs::register()`，按 domain 模块组织：

| Domain | 文件 | 函数示例 |
|--------|------|---------|
| `array` | `array.rs` | `push` / `pop` / `len` / `map` / `filter` / `reduce` / `slice` / `concat` / `reverse` / `sort` |
| `string` | `string.rs` | `len` / `substr` / `upper` / `lower` / `split` / `join` / `trim` / `replace` / `find` / `repeat` |
| `math` | `math.rs` | `abs` / `floor` / `ceil` / `round` / `sqrt` / `pow` / `sin` / `cos` / `tan` / `log` / `min` / `max` |
| `io` | `io.rs` | `print` / `println` / `read_file` / `write_file` / `read_line` |
| `time` | `time.rs` | `now` / `sleep` / `format_time` |
| `convert` | `convert.rs` | `to_string` / `to_int` / `to_float` / `to_bool` / `parse_int` / `parse_float` |
| `debug` | `debug.rs` | `dbg` / `assert` / `type_of` / `dump` |

调用方式：`println(arr)`、`map(arr, |x| x * 2)`。

---

## 7. 语义规范

### 7.1 作用域规则

- **块级作用域**：`{}` 内的 `let` 绑定在块外不可见。
- **函数作用域**：参数与函数体内 `let` 同属函数作用域。
- **闭包捕获**：闭包可访问外层词法作用域变量，按 `ByValue` / `ByBox` 模式捕获。
- **多层捕获**：通过 `parent_env` 链支持 3+ 层嵌套闭包（HOF 返回闭包场景）。

### 7.2 尾调用优化 (TCO)

VM 在 dispatch 阶段检测尾调用并生成 `TailCall` opcode，复用当前调用帧，避免栈增长。

- 命中条件：`return f(args)` 形式，且 `f(args)` 是 return 语句的唯一表达式
- 未命中：递归调用栈持续增长，可能触发 8 MB 栈溢出（由 VEH handler 捕获）

```nuzo
fn loop(n) {
    if n == 0 { return 0; }
    return loop(n - 1);   // TCO 命中，恒定栈
}
```

### 7.3 GC 触发条件

- **分配阈值**：自上次 GC 以来堆分配字节数超过阈值
- **显式触发**：`gc_stress_boundary.nuzo` 等测试通过高频分配触发
- **安全点**：仅在 opcode dispatch 间隙检查，**禁止 GC 安全点间持有裸指针**

GC 流程：mark（从根集合出发，递归标记 reachable heap object）→ sweep（释放未标记对象）→ compact（可选）。

### 7.4 错误传播

- 运行时错误（除零、类型错误、栈溢出、index OOB）通过 `NuzoError` 沿调用栈向上传播
- 被 `try { ... } catch err { ... }` 捕获后转为 `Exception` 对象
- 未被捕获的错误导致 VM 返回 `Err(NuzoError)`，由 CLI 渲染并退出码 1

---

## 8. 示例代码

### 8.1 Hello World

```nuzo
println("Hello, World!");
```

### 8.2 Fibonacci（TCO 命中）

```nuzo
fn fib(n) {
    if n < 2 { return n; }
    return fib(n - 1) + fib(n - 2);
}

fn fib_iter(n, a, b) {
    if n == 0 { return a; }
    return fib_iter(n - 1, b, a + b);   // TCO
}

for i in 0..<20 {
    print(fib(i));
    print(" ");
}
println("");
```

### 8.3 闭包捕获（ByBox）

```nuzo
fn make_counter() {
    let count = 0;
    let inc = fn() { count = count + 1; return count; };
    let get = fn() { return count; };
    return { inc: inc, get: get };
}

let c = make_counter();
c.inc();
c.inc();
println(c.get());   // 2
```

### 8.4 模式匹配

```nuzo
fn classify(n) {
    return match n {
        0 => "zero",
        1 | 2 | 3 => "small",
        x if x < 100 => "medium",
        _ => "large",
    };
}

for n in [0, 2, 50, 999] {
    println(classify(n));
}
```

### 8.5 错误处理

```nuzo
fn divide(a, b) {
    if b == 0 {
        out Exception({
            message: "division by zero",
            code: "DivisionByZero",
            context: { a: a, b: b },
        });
    }
    return a / b;
}

try {
    let r = divide(10, 0);
    println("result: " + r);
} catch err {
    println("caught: " + err.message);
    println("code: " + err.code);
} keep {
    println("cleanup");
}
```

### 8.6 管道与高阶函数

```nuzo
let result = [1, 2, 3, 4, 5]
    |> |xs| map(xs, |x| x * x)
    |> |xs| filter(xs, |x| x > 5)
    |> |xs| reduce(xs, 0, |a, b| a + b);
println(result);   // 50 (9 + 16 + 25)
```

### 8.7 双语关键字混用

```nuzo
函数 阶乘(n) {
    如果 n < 2 { 返回 1; }
    返回 n * 阶乘(n - 1);
}

遍历 i 在 1..=5 {
    println(阶乘(i));
}
```

### 8.8 空值合并

```nuzo
let config = { host: "localhost" };
let host = config.host ?? "127.0.0.1";
let port = config.port ?? 8080;
println(host + ":" + port);   // localhost:8080
```

---

## 9. 参考实现

| 文档章节 | 实现位置 |
|---------|---------|
| 词法 | `crates/nuzo-frontend/src/lexer.rs` |
| 语法 | `crates/nuzo-frontend/src/parser.rs` |
| AST | `crates/nuzo-frontend/src/ast.rs` |
| Token | `crates/nuzo-frontend/src/token.rs` |
| 编译 | `crates/nuzo-compiler/src/compiler.rs` |
| IR | `crates/nuzo-ir/src/` |
| 字节码 | `crates/nuzo-bytecode/src/opcode.rs` |
| VM 主循环 | `crates/nuzo-vm/src/vm.rs` |
| 指令分发 | `crates/nuzo-vm/src/dispatch.rs` |
| 值系统 | `crates/nuzo-values/src/{value.rs,heap.rs,function.rs}` |
| GC | `crates/nuzo-vm/src/gc.rs` |
| builtin | `crates/nuzo-helpers/src/builtins.rs` |
| 错误 | `crates/nuzo-error/src/diagnostic.rs` |

权威调用关系见 [CALL_GRAPH.md](../CALL_GRAPH.md)。
