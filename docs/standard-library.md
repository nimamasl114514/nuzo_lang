# Nuzo Lang 标准库参考

Nuzo 的标准库由一组 **助手模块（Helper Modules）** 组成，在语言启动时自动挂载到全局命名空间。所有模块提供纯函数式 API，不引入副作用（IO 模块除外）。

> **获取函数列表**：运行 `nuzo --list-helpers` 可打印所有可用函数及其签名。

---

## 目录

| 模块 | 函数数 | 说明 |
|------|--------|------|
| [math](#math-模块) | 15 | 数学运算、三角函数、随机数 |
| [string](#string-模块) | 15 | 字符串分割、转换、搜索、格式化 |
| [array](#array-模块) | 5 | 数组搜索、切片、排序 |
| [dict](#dict-操作) | 5 | 字典键值操作函数 |
| [io](#io-模块) | 4 | 文件读写、标准输入 |
| [sys](#sys-模块) | 10 | 系统交互：环境变量、进程、文件系统 |
| [time](#time-模块) | 4 | 时间戳、延迟、性能计时 |
| [convert](#convert-模块) | 12 | 类型转换与判断 |
| [debug](#debug-模块) | 4 | 调试输出、性能计时 |
| [双语支持](#双语支持) | — | NUZO_LANG 环境变量与中英文别名 |

---

## math 模块

### 概述

提供 15 个数学运算函数，覆盖基础算术、三角函数、统计和随机数。所有运算基于 **IEEE 754 双精度浮点数（f64）**，确保跨平台一致性。

**精度**：约 15–17 位有效数字 | **范围**：±1.8 × 10^308

---

### 基础运算（5 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `abs` | `abs(x) → number` | 绝对值 |
| `floor` | `floor(x) → number` | 向下取整 |
| `ceil` | `ceil(x) → number` | 向上取整 |
| `round` | `round(x) → number` | 四舍五入 |
| `sqrt` | `sqrt(x) → number` | 平方根，x ≥ 0 |

```nuzo
abs(-5)         // → 5.0
floor(3.7)      // → 3.0
ceil(3.2)       // → 4.0
round(3.5)      // → 4.0
sqrt(16.0)      // → 4.0
sqrt(-1)        // → Error: type mismatch (负数)
```

---

### 幂与对数（2 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `pow` | `pow(base, exp) → number` | base 的 exp 次幂 |
| `log` | `log(x) → number` | 自然对数（底数 e），x > 0 |

```nuzo
pow(2, 10)      // → 1024.0
pow(3, 0.5)     // → 1.732... (开方)
log(2.71828)    // → ~1.0
log(0)          // → Error: type mismatch (非正数)
```

> **注意**：`pow(10, 1000)` 会返回 `Infinity`（符合 IEEE 754 规范，不报错）。

---

### 统计（2 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `min` | `min(a, b) → number` | 返回较小值 |
| `max` | `max(a, b) → number` | 返回较大值 |

```nuzo
min(3, 7)       // → 3.0
max(-1, 0)      // → 0.0
min(3.14, 2.71) // → 2.71
```

---

### 三角函数（3 个）

**所有角度单位均为弧度（radians）**。

| 函数 | 签名 | 说明 |
|------|------|------|
| `sin` | `sin(x) → number` | 正弦 |
| `cos` | `cos(x) → number` | 余弦 |
| `tan` | `tan(x) → number` | 正切 |

```nuzo
sin(0)              // → 0.0
cos(pi())           // → -1.0
tan(pi() / 4)       // → 1.0

// 角度转弧度：radians = degrees * pi() / 180
let degrees = 90
let radians = degrees * pi() / 180
sin(radians)        // → 1.0
```

---

### 随机数与常量（3 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `random` | `random() → number` | [0, 1) 伪随机数（Xorshift64） |
| `pi` | `pi() → number` | 圆周率 π ≈ 3.141592653589793 |
| `e` | `e() → number` | 自然常数 e ≈ 2.718281828459045 |

```nuzo
let r = random()         // → [0.0, 1.0) 之间的数
let area = pi() * 5 * 5  // → 78.5398... (半径 5 的圆面积)

// 生成指定范围的随机整数
let min = 1
let max = 100
let roll = floor(random() * (max - min + 1)) + min  // [1, 100]
```

> **注意**：随机数种子固定为 `0x0123456789ABCDEF`，跨平台可复现。不适合加密场景。

---

## string 模块

### 概述

提供 12 个字符串处理函数，**所有操作基于 Unicode 码点（非字节）**，完整支持 UTF-8、Emoji 和多国语言文本。

```
"Hello 世界 🌍"
 字符长度: 9 characters    ← 所有函数按字符计数
 字节长度: 14 bytes        ← 底层 UTF-8 存储
```

---

### 分割与连接（2 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `split` | `split(s, sep) → array` | 按分隔符分割字符串 |
| `join` | `join(arr, sep) → string` | 数组元素连接为字符串 |

```nuzo
split("Alice,30,Bob", ",")
// → ["Alice", "30", "Bob"]

join(["a", "b", "c"], "-")
// → "a-b-c"

// 路径拼接
join(["/home", "user", "docs"], "/")
// → "/home/user/docs"
```

---

### 大小写与空白（3 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `upper` | `upper(s) → string` | 转大写 |
| `lower` | `lower(s) → string` | 转小写 |
| `trim` | `trim(s) → string` | 去除首尾空白字符 |

```nuzo
upper("hello")      // → "HELLO"
lower("HELLO")      // → "hello"
trim("  spaced  ")  // → "spaced"

// Unicode 大小写转换
upper("café")       // → "CAFÉ"
```

---

### 搜索与替换（3 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `replace` | `replace(s, old, new) → string` | 全局替换子串 |
| `starts_with` | `starts_with(s, prefix) → bool` | 检测前缀 |
| `ends_with` | `ends_with(s, suffix) → bool` | 检测后缀 |

```nuzo
replace("foo-bar-baz", "-", "_")
// → "foo_bar_baz"

starts_with("hello.txt", "hello")   // → true
ends_with("hello.txt", ".txt")      // → true

// URL 检查
let url = "https://example.com"
if starts_with(url, "https://") {
    println("安全连接")
}
```

---

### 变换与提取（3 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `reverse` | `reverse(s) → string` | 按字符反转 |
| `repeat` | `repeat(s, n) → string` | 重复 n 次（上限 10000） |
| `substring` | `substring(s, start, end) → string` | 提取子串（字符索引） |

```nuzo
reverse("Nuzo")         // → "ozuN"
reverse("你好")          // → "好你"

repeat("ab", 3)         // → "ababab"

// 子串：包含 start，不包含 end
substring("Nuzo 语言", 0, 4)    // → "Nuzo"
substring("Nuzo 语言", 5, 7)    // → "语言"

// Emoji 安全
substring("👋🌍💻", 0, 2)    // → "👋🌍"
```

---

### 空值检测（1 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `is_empty` | `is_empty(x) → bool` | 检测空值（支持 string / array / dict） |

```nuzo
is_empty("")          // → true
is_empty("hello")     // → false

is_empty([])          // → true
is_empty([1])         // → false

is_empty({})          // → true
is_empty({"a": 1})    // → false
```

---

### 格式化（3 个）

提供 printf 风格的字符串格式化函数，支持 `{}` 占位符。**与 debug 模块的 `format` 函数行为一致**，此处为正式的字符串处理 API。

| 函数 | 签名 | 说明 |
|------|------|------|
| `format` | `format(template, args...) → string` | 格式化字符串，返回结果 |
| `printf` | `printf(template, args...) → nil` | 格式化后输出到 stdout（无换行）|
| `printlnf` | `printlnf(template, args...) → nil` | 格式化后输出到 stdout（带换行）|

```nuzo
// format: 返回格式化字符串
let name = "Alice"
let age = 30
let msg = format("姓名: {}, 年龄: {}", name, age)
// → "姓名: Alice, 年龄: 30"

// printf: 输出不换行
printf("进度: {}/{}", 3, 10)   // 输出: 进度: 3/10

// printlnf: 输出并换行
printlnf("用户 {} 登录成功", name)  // 输出: 用户 Alice 登录成功\n

// 多种类型自动转换
printlnf("数组: {}, 字典: {}", [1, 2, 3], {"a": 1})
// 输出: 数组: [1, 2, 3], 字典: {"a": 1}
```

> **占位符规则**：`{}` 按顺序匹配参数，参数数量不足时报错，多余参数被忽略。

---

## array 模块

### 概述

提供 5 个数组操作函数。**所有函数返回新数组**（不可变风格），不修改原数组。

---

### 函数列表

| 函数 | 签名 | 说明 | 复杂度 |
|------|------|------|--------|
| `index_of` | `index_of(arr, val) → number` | 首次出现索引，未找到返回 -1 | O(n) |
| `slice` | `slice(arr, start, end) → array` | 提取子数组（含 start，不含 end）| O(k) |
| `concat` | `concat(arr1, arr2) → array` | 连接两个数组 | O(n+m) |
| `unique` | `unique(arr) → array` | 去重（保留顺序）| O(n²) |
| `sort` | `sort(arr) → array` | 升序排序（仅数字）| O(n log n) |

```nuzo
// index_of
let fruits = ["apple", "banana", "cherry"]
index_of(fruits, "banana")    // → 1
index_of(fruits, "durian")    // → -1 (不存在)

// slice
let nums = [0, 1, 2, 3, 4, 5]
slice(nums, 1, 4)             // → [1, 2, 3]

// concat
concat([1, 2], [3, 4])        // → [1, 2, 3, 4]

// unique
unique([1, 2, 2, 3, 3, 3])    // → [1, 2, 3]

// sort (仅支持数字)
sort([3, 1, 4, 1, 5])         // → [1, 1, 3, 4, 5]
```

> **注意**：`sort` 仅支持纯数字数组，混入非数字元素将报错。

---

## dict 操作

Nuzo 的字典通过 **内置语法** 直接操作，无需导入额外模块：

```nuzo
// 创建
let d = {"name": "Alice", "age": 30}

// 访问
d["name"]           // → "Alice"
d["score"] = 95     // 设置（键不存在时自动创建）

// 删除
remove(d, "score")  // 从字典中移除键

// 内置函数
keys(d)             // → ["name", "age", "score"] (返回字符串数组)
len(d)              // → 3 (键数量)
typeof(d)           // → "dict"
```

| 操作 | 方式 | 说明 |
|------|------|------|
| 读取 | `dict[key]` | 键不存在返回 nil |
| 写入 | `dict[key] = value` | 自动创建或覆盖 |
| 删除 | `remove(dict, key)` | 移除键值对 |
| 判键 | `key in dict` | 检查键是否存在 |
| 遍历 | `for k, v in dict` | 迭代键值对 |
| 键列表 | `keys(dict)` | 返回键数组 |
| 数量 | `len(dict)` | 返回键数量 |
| 类型判断 | `is_dict(dict)` | 判断是否为字典 |
| 清空 | 无内置，逐键 remove | — |

### 函数列表（5 个）

以下函数专门用于字典操作，**所有函数返回新值**（不修改原字典，`extend` 返回新字典）：

| 函数 | 签名 | 说明 |
|------|------|------|
| `keys` | `keys(dict) → array` | 返回所有键组成的数组 |
| `values` | `values(dict) → array` | 返回所有值组成的数组 |
| `has_key` | `has_key(dict, key) → bool` | 检查键是否存在（等价于 `key in dict`）|
| `has_value` | `has_value(dict, value) → bool` | 检查值是否存在 |
| `extend` | `extend(dict1, dict2) → dict` | 合并两个字典（dict2 覆盖 dict1 的同名键）|

```nuzo
let user = {"name": "Alice", "age": 30, "city": "北京"}

// keys: 获取所有键
keys(user)       // → ["name", "age", "city"]

// values: 获取所有值
values(user)     // → ["Alice", 30, "北京"]

// has_key: 检查键
has_key(user, "name")    // → true
has_key(user, "score")   // → false

// has_value: 检查值
has_value(user, 30)      // → true
has_value(user, "Bob")   // → false

// extend: 合并字典
let defaults = {"theme": "light", "lang": "en"}
let custom = {"lang": "zh", "font": "sans"}
let merged = extend(defaults, custom)
// → {"theme": "light", "lang": "zh", "font": "sans"}
// 注意：custom 的 "lang" 覆盖了 defaults 的 "lang"
```

> **注意**：
> - `keys` 和 `values` 返回的数组顺序与字典插入顺序一致
> - `has_value` 使用值相等比较（数字 1 与 1.0 相等）
> - `extend` 不修改原字典，返回新字典

---

## io 模块

### 概述

提供 4 个 IO 函数，支持文件读写和标准输入。**所有文件以 UTF-8 编码**读写。IO 错误统一包装为 `NuzoError::Internal`。

> **安全提示**：
> - 当前实现无沙箱限制，可访问任意文件路径
> - 无文件大小限制，`read_file` 一次性读入内存
> - 所有 IO 操作同步阻塞当前线程

---

### 函数列表

| 函数 | 签名 | 说明 |
|------|------|------|
| `input` | `input(prompt?) → string` | 从 stdin 读取一行 |
| `read_file` | `read_file(path) → string` | 读取文件全部内容 |
| `write_file` | `write_file(path, content) → nil` | 覆盖写入文件 |
| `append_file` | `append_file(path, content) → nil` | 追加到文件（自动创建父目录） |

```nuzo
// 用户交互
let name = input("请输入姓名: ")
println("你好, " + name)

// 文件读写
let data = read_file("config.txt")
let lines = split(data, "\n")

// 写入
let output = join(lines, "\t")
write_file("output.txt", output)

// 日志追加
append_file("log.txt", "[" + str(now()) + "] 任务完成\n")
```

---

## sys 模块

### 概述

提供 10 个系统交互函数，覆盖命令行参数、环境变量、进程控制和文件系统操作。**所有路径操作基于 UTF-8 编码**，文件系统错误统一包装为 `NuzoError::Internal`。

> **安全提示**：
> - 文件系统操作无沙箱限制，可访问任意路径
> - `exit` 会立即终止进程，不执行 deferred 函数
> - `list_dir` 返回的路径不保证排序顺序

---

### 命令行与环境（3 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `args` | `args() → array` | 返回命令行参数数组（含程序名）|
| `env` | `env() → dict` | 返回所有环境变量（键值对字典）|
| `getenv` | `getenv(name) → string` | 获取指定环境变量，不存在返回空字符串 |

```nuzo
// 命令行参数
let argv = args()
// 运行 `nuzo run script.nuzo --port 8080` 时:
// argv = ["nuzo", "run", "script.nuzo", "--port", "8080"]

// 获取单个参数
if len(argv) >= 2 {
    let script = argv[1]
    println("运行脚本: " + script)
}

// 所有环境变量
let envs = env()
println("PATH: " + envs["PATH"])

// 单个环境变量
let home = getenv("HOME")
if home != "" {
    println("主目录: " + home)
}

// Nuzo 双语模式（详见"双语支持"章节）
let lang_mode = getenv("NUZO_LANG")  // "zh" | "en" | "both"
```

---

### 进程控制（1 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `exit` | `exit(code) → nil` | 立即退出进程，code 为退出码 |

```nuzo
// 正常退出
if config_error {
    eprintln("配置错误，无法继续")
    exit(1)   // 非零退出码表示错误
}

// 成功退出
exit(0)
```

> **注意**：`exit` 不会执行任何 deferred 清理逻辑，请确保在此之前完成资源释放。

---

### 文件系统（5 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `list_dir` | `list_dir(path) → array` | 列出目录下的文件和子目录名 |
| `mkdir` | `mkdir(path) → nil` | 创建目录（含父目录）|
| `exists` | `exists(path) → bool` | 检查路径是否存在 |
| `remove` | `remove(path) → nil` | 删除文件或空目录 |
| `rename` | `rename(old, new) → nil` | 重命名或移动文件/目录 |

```nuzo
// 检查文件是否存在
if exists("config.json") {
    let config = read_file("config.json")
    println("加载配置: " + config)
} else {
    eprintln("警告: 配置文件不存在，使用默认值")
}

// 列出目录内容
let entries = list_dir(".")
for entry in entries {
    println(entry)
}

// 创建目录
mkdir("output/logs")    // 递归创建父目录

// 重命名文件
rename("temp.txt", "final.txt")

// 删除文件
remove("temp.txt")

// 注意：remove 是多态函数
// - 传入字典时移除键（见 dict 操作章节）
// - 传入字符串路径时删除文件
```

> **注意**：`remove` 是多态函数，传入字典时移除键，传入字符串路径时删除文件。语义根据参数类型自动判断。

---

### 标准错误输出（1 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `eprintln` | `eprintln(args...) → nil` | 输出到 stderr（带换行）|

```nuzo
// 错误日志输出到 stderr，不干扰正常 stdout
eprintln("错误: 文件未找到")
eprintln("警告: ", "内存不足", ", 剩余: ", 1024, " 字节")

// 与 print/println 的区别:
// print/println → stdout (标准输出，用于正常结果)
// eprintln      → stderr (标准错误，用于日志和错误)
```

---

## time 模块

### 概述

提供 4 个时间函数，基于 **Unix 纪元（1970-01-01 00:00:00 UTC）**。所有时间为 **UTC**，`clock()` 使用单调时钟（不受系统时间调整影响）。

---

### 函数列表

| 函数 | 签名 | 说明 | 精度 |
|------|------|------|------|
| `now` | `now() → number` | Unix 时间戳（秒）| 纳秒级 |
| `timestamp` | `timestamp() → number` | 毫秒时间戳 | 毫秒级 |
| `sleep` | `sleep(seconds) → nil` | 阻塞线程指定秒数 | 微秒级 |
| `clock` | `clock() → number` | 进程运行时间（秒）| 纳秒级 |

```nuzo
// 获取当前时间
let current = now()             // → 1704067200.123456789
let millis = timestamp()        // → 1704067200123

// 性能计时
let start = clock()
// ... 执行耗时操作 ...
let elapsed = clock() - start
println("耗时: " + str(elapsed) + " 秒")

// 延迟
sleep(1.5)  // 暂停 1.5 秒（会阻塞当前线程）
```

> **注意**：`sleep` 参数必须 ≥ 0，负数将报错。

---

## convert 模块

### 概述

提供 12 个类型转换和判断函数，覆盖类型转换、类型判断、字符串输出和打印。**`print` 实际注册为内置函数，但逻辑归属 convert 模块。**

---

### 类型转换（4 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `int` | `int(x) → number` | 向零截断转整数 |
| `float` | `float(x) → number` | 转浮点数 |
| `bool` | `bool(x) → bool` | 转布尔值 |
| `num` | `num(s) → number` | 字符串解析数字 |

```nuzo
// int: 向零截断
int(3.7)        // → 3.0
int(-2.9)       // → -2.0
int("42")       // → 42.0
int("3.14")     // → 3.0
int(true)       // → 1.0
int(nil)        // → 0.0

// float
float(3)        // → 3.0
float(true)     // → 1.0
float("3.14")   // → 3.14

// bool: 真值规则见下方
bool(0)         // → false
bool("")        // → false
bool([])        // → true  (空数组是 truthy)
bool(-1)        // → true

// num: 数字解析
num("42")       // → 42.0
num("3.14")     // → 3.14
num("1e3")      // → 1000.0
num(42)         // → 42.0  (直接返回)
```

---

### 类型判断（6 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `is_nil` | `is_nil(x) → bool` | 判断 nil |
| `is_number` | `is_number(x) → bool` | 判断数字 |
| `is_string` | `is_string(x) → bool` | 判断字符串 |
| `is_array` | `is_array(x) → bool` | 判断数组 |
| `is_dict` | `is_dict(x) → bool` | 判断字典 |
| `is_closure` | `is_closure(x) → bool` | 判断闭包/函数 |

```nuzo
is_nil(nil)             // → true
is_number(42)           // → true
is_number("42")         // → false (字符串)
is_string("hello")      // → true
is_array([1, 2])        // → true
is_dict({"a": 1})       // → true
is_closure(fn)          // → true (用户定义的函数)
```

---

### 输出（2 个）

| 函数 | 签名 | 说明 |
|------|------|------|
| `str` | `str(x) → string` | 转为字符串表示 |
| `print` | `print(args...) → nil` | 打印到 stdout（无换行） |

```nuzo
str(42)             // → "42"
str(true)           // → "true"
str(nil)            // → "nil"
str([1, 2, 3])      // → "[1, 2, 3]"

print("Hello, ")
print("World!")     // 输出: Hello, World!
println()           // 输出换行
```

---

### 真值规则（Truthiness Rules）

| Falsy 值 | Truthy 值 |
|----------|-----------|
| `false` | 所有非零数字（正数、负数、Infinity）|
| `nil` | 非空字符串 |
| `0` | 数组（包括空数组 `[]`）|
| `""`（空字符串）| 字典（包括空字典 `{}`）|
| | 闭包/函数 |

---

## debug 模块

### 概述

提供 4 个调试工具函数，所有调试输出发送到 **stderr**（标准错误流），不干扰正常 stdout 输出。

| 函数 | 签名 | 说明 |
|------|------|------|
| `dump` | `dump(value) → nil` | 打印值的详细信息 |
| `format` | `format(template, args...) → string` | 格式化字符串 |
| `time` | `time(label) → nil` | 启动带标签计时器 |
| `time_end` | `time_end(label) → nil` | 结束计时器并打印耗时 |

```nuzo
// dump: 查看变量完整信息
let data = {"name": "Alice", "scores": [95, 87]}
dump(data)
// [dump] type=dict, value={"name": "Alice", "scores": [95, 87]}

// format: printf 风格格式化
let name = "Nuzo"
let version = "1.0"
let msg = format("欢迎使用 {} 版本 {}", name, version)
// → "欢迎使用 Nuzo 版本 1.0"

// time / time_end: 性能计时
time("排序开始")
sort(large_array)
time_end("排序开始")
// [time] 排序开始: 0.023456 秒
```

---

## 双语支持

### 概述

Nuzo Lang 支持中英文双语函数命名，通过环境变量 `NUZO_LANG` 控制启用模式。所有标准库函数均提供中文别名，便于中文用户使用。

---

### NUZO_LANG 环境变量

| 值 | 模式 | 说明 |
|----|------|------|
| `zh` | 中文模式 | 仅注册中文别名，英文函数名不可用 |
| `en` | 英文模式 | 仅注册英文函数名，中文别名不可用 |
| `both` | 双语模式（默认）| 中英文函数名均可用 |

```bash
# Linux/macOS 设置语言模式
export NUZO_LANG=zh     # 仅中文
export NUZO_LANG=en     # 仅英文
export NUZO_LANG=both   # 双语（默认，未设置时也是此模式）

# Windows PowerShell
$env:NUZO_LANG = "zh"
```

```nuzo
// NUZO_LANG=both 时以下两种写法等价
println("Hello")      // 英文
打印换行("Hello")      // 中文别名

// NUZO_LANG=zh 时只能用中文
打印换行("Hello")      // ✓
println("Hello")      // ✗ 错误: 未定义函数

// NUZO_LANG=en 时只能用英文
println("Hello")      // ✓
打印换行("Hello")      // ✗ 错误: 未定义函数
```

---

### 中英文别名对照表

> **说明**：以下仅列出常用函数的中文别名，完整列表请运行 `nuzo --list-helpers`。

#### 输出与打印

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `print` | `打印` | convert |
| `println` | `打印换行` | 内置 |
| `printf` | `格式打印` | string |
| `printlnf` | `格式打印换行` | string |
| `eprintln` | `错误打印换行` | sys |
| `dump` | `转储` | debug |

#### 数学运算

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `abs` | `绝对值` | math |
| `floor` | `向下取整` | math |
| `ceil` | `向上取整` | math |
| `round` | `四舍五入` | math |
| `sqrt` | `平方根` | math |
| `pow` | `幂运算` | math |
| `random` | `随机数` | math |

#### 字符串处理

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `split` | `分割` | string |
| `join` | `连接` | string |
| `upper` | `转大写` | string |
| `lower` | `转小写` | string |
| `trim` | `去空白` | string |
| `replace` | `替换` | string |
| `reverse` | `反转` | string |
| `format` | `格式化` | string |

#### 数组与字典

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `keys` | `键列表` | dict |
| `values` | `值列表` | dict |
| `has_key` | `含键` | dict |
| `has_value` | `含值` | dict |
| `extend` | `合并` | dict |
| `sort` | `排序` | array |
| `unique` | `去重` | array |
| `concat` | `拼接` | array |

#### 系统交互

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `args` | `参数` | sys |
| `env` | `环境` | sys |
| `getenv` | `取环境` | sys |
| `exit` | `退出` | sys |
| `list_dir` | `列目录` | sys |
| `mkdir` | `建目录` | sys |
| `exists` | `存在` | sys |
| `rename` | `重命名` | sys |

#### 类型转换与判断

| 英文 | 中文别名 | 模块 |
|------|---------|------|
| `int` | `转整数` | convert |
| `float` | `转浮点` | convert |
| `bool` | `转布尔` | convert |
| `str` | `转字符串` | convert |
| `len` | `长度` | 内置 |
| `typeof` | `类型名` | 内置 |
| `is_nil` | `是空值` | convert |
| `is_number` | `是数字` | convert |
| `is_string` | `是字符串` | convert |

---

### 使用建议

1. **教学场景**：推荐 `NUZO_LANG=zh`，降低英文门槛
2. **生产环境**：推荐 `NUZO_LANG=en`，与国际生态兼容
3. **开发调试**：推荐 `NUZO_LANG=both`（默认），灵活切换

---

## 内置函数（非模块）

以下函数不归属于任何助手模块，在语言启动时直接挂载：

| 函数 | 签名 | 说明 |
|------|------|------|
| `println` | `println(args...) → nil` | 打印到 stdout（带换行）|
| `type_of` | `type_of(value) → number` | 返回类型代码（数字）|
| `typeof` | `typeof(value) → string` | 返回类型名称（字符串）|
| `assert` | `assert(cond, msg?) → nil` | 条件断言 |
| `len` | `len(value) → number` | 获取长度（字符串/数组/字典）|
| `push` | `push(arr, val) → nil` | 数组末尾追加（原地修改）|
| `pop` | `pop(arr) → value` | 移除并返回末尾元素 |
| `keys` | `keys(dict) → array` | 返回字典的所有键 |
| `trampoline` | `trampoline(fn, arg) → value` | 安全递归执行器（防栈溢出）|
| `remove` | `remove(dict, key) → nil` | 从字典中移除键 |

---

## 附录

### 类型代码表

`type_of()` 返回的数值代码：

| 代码 | 类型 | `typeof()` 返回值 |
|------|------|-------------------|
| 0 | unknown | `"unknown"` |
| 1 | number | `"number"` |
| 2 | bool | `"bool"` |
| 3 | nil | `"nil"` |
| 4 | string | `"string"` |
| 5 | array | `"array"` |
| 6 | object | `"object"` |

### 错误处理约定

| 错误类型 | 触发场景 | 示例 |
|----------|----------|------|
| `TypeMismatch` | 参数类型不匹配 | `sqrt(-1)` |
| `InvalidArgumentCount` | 参数数量错误 | `abs()` 无参数 |
| `Internal` | 底层系统错误 | `read_file` 文件不存在 |
| `AssertFailed` | 断言条件不满足 | `assert(false)` |
| `ArithmeticOverflow` | 算术溢出 | `repeat("x", 99999999)` |

### 模块文件索引

| 模块 | 源文件 |
|------|--------|
| math | `crates/nuzo_helpers/src/math.rs` |
| string | `crates/nuzo_helpers/src/string.rs` |
| array | `crates/nuzo_helpers/src/array.rs` |
| io | `crates/nuzo_helpers/src/io.rs` |
| sys | `crates/nuzo_helpers/src/sys.rs` |
| time | `crates/nuzo_helpers/src/time.rs` |
| convert | `crates/nuzo_helpers/src/convert.rs` |
| debug | `crates/nuzo_helpers/src/debug.rs` |
| builtins | `crates/nuzo_helpers/src/builtins.rs` |
