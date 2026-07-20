//! # I/O 辅助函数
//!
//! 本模块提供**文件和输入输出操作**功能集，支持文件读写、用户交互等场景。
//! 所有 IO 操作都遵循**错误优先原则**，确保异常情况可被正确捕获和处理。
//!
//! ## 可用函数（4 个）
//!
//! | 函数 | 签名 | 说明 | 错误处理 |
//! |------|------|------|----------|
//! | `input` | `input(prompt?) → string` | 从标准输入读取一行 | ✅ I/O 错误包装 |
//! | `read_file` | `read_file(path) → string` | 读取文件全部内容 | ✅ 文件不存在/权限错误 |
//! | `write_file` | `write_file(path, content) → nil` | 写入文件（覆盖）| ✅ 创建失败/写入失败 |
//! | `append_file` | `append_file(path, content) → nil` | 追加到文件末尾 | ✅ 自动创建目录 |
//!
//! ## 错误处理机制
//!
//! ### 统一错误类型
//!
//! 所有 IO 错误都被包装为 [`NuzoError::Internal`]，携带描述性消息：
//!
//! ```text
//! Error: Internal error: read_file("config.txt") failed:
//!        系统找不到指定的文件。 (os error 2)
//! ```
//!
//! ### 常见错误场景
//!
//! | 操作 | 可能的错误 | 处理建议 |
//! |------|-----------|----------|
//! | `read_file` | 文件不存在、无读取权限、路径非法 | 检查路径和权限 |
//! | `write_file` | 目录不存在、无写入权限、磁盘满 | 先检查/创建目录 |
//! | `append_file` | 同上 + 自动创建父目录 | 无需手动创建目录 |
//! | `input` | stdin 关闭、编码错误 | 通常在 REPL 场景使用 |
//!
//! # 文件路径规范
//!
//! - **相对路径**：相对于进程工作目录解析
//! - **绝对路径**：Windows (`C:\...`) / Unix (`/home/...`)
//! - **路径分隔符**：自动适配操作系统（`\` 或 `/`）
//! - **UTF-8 编码**：所有文件内容以 UTF-8 读写
//!
//! # 使用示例
//!
//! ```nuzo
//! // 用户交互
//! let name = input("请输入姓名: ")
//! println("你好, " + name)
//!
//! // 文件读写
//! let content = read_file("data.txt")
//! let processed = upper(content)
//! write_file("output.txt", processed)
//!
//! // 日志追加
//! append_file("log.txt", "[" + str(now()) + "] 用户登录\n")
//! ```
//!
//! # 安全性说明
//!
//! ⚠️ **重要提示**：
//! - **无沙箱限制**：当前实现可访问任意文件路径
//! - **无大小限制**：`read_file` 会一次性读入整个文件（注意内存）
//! - **同步阻塞**：所有 IO 操作会阻塞当前线程
//! - **编码假设**：默认 UTF-8，二进制文件可能损坏
//!
//! # 性能特征
//!
//! - **缓冲 IO**：使用操作系统默认缓冲区大小
//! - **原子性**：小文件写入通常是原子的（< 4KB）
//! - **目录创建**：`append_file` 自动递归创建父目录

use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path};

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{NIL, NuzoError, ValueExt};

/// Maximum allowed file size for read_file (16 MB).
const MAX_READ_FILE_BYTES: u64 = 16 * 1024 * 1024;

// ============================================================================
// IO 路径沙箱（P1-4）
// ============================================================================
//
// 默认拒绝路径中包含 `..` 组件，防止路径穿越攻击
// （如 `read_file("../../../etc/passwd")` 读到沙箱外文件）。
//
// 绝对路径在沙箱模式下仍被允许：它们不含 `..`，且可访问根目录
// 控制应由上层应用配置（如 chroot / 工作目录绑定）承担。
//
// 向后兼容：调用方可通过 [`set_allow_unsafe_io_paths`] 关闭沙箱，
// 用于确实需要访问上级目录的合法场景（如配置加载器）。

thread_local! {
    /// 当前线程是否允许包含 `..` 的 IO 路径。
    ///
    /// - `false`（默认）：拒绝路径穿越（沙箱模式）
    /// - `true`：跳过路径校验（向后兼容模式）
    static ALLOW_UNSAFE_IO_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 设置当前线程的 IO 路径沙箱模式。
///
/// # 参数
/// * `allow` - `true` 关闭沙箱（允许 `..` 路径）；`false` 开启沙箱（拒绝 `..` 路径）
///
/// # 用法
/// ```rust,ignore
/// use nuzo_helpers::io::set_allow_unsafe_io_paths;
///
/// // 需要访问上级目录的合法场景
/// set_allow_unsafe_io_paths(true);
/// let _ = read_file("../config/app.toml");
/// set_allow_unsafe_io_paths(false); // 恢复沙箱
/// ```
pub fn set_allow_unsafe_io_paths(allow: bool) {
    ALLOW_UNSAFE_IO_PATHS.with(|f| f.set(allow));
}

/// 查询当前线程是否允许包含 `..` 的 IO 路径。
pub fn is_unsafe_io_paths_allowed() -> bool {
    ALLOW_UNSAFE_IO_PATHS.with(|f| f.get())
}

/// 校验文件路径是否安全（沙箱模式）。
///
/// 在沙箱模式（默认）下，拒绝路径中包含 `..` 组件的访问请求，
/// 防止路径穿越攻击。
///
/// # 错误
/// 当沙箱启用且路径含 `..` 组件时，返回 `InternalError::IoError`，
/// 错误消息包含函数名和路径，提示如何显式覆盖沙箱。
fn validate_io_path(path: &str, fn_name: &str) -> Result<(), NuzoError> {
    if is_unsafe_io_paths_allowed() {
        return Ok(());
    }
    // 用 Path::components 跨平台解析路径组件，检测 ParentDir (`..`)
    if Path::new(path).components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!(
                    "{}(\"{}\") rejected: path traversal ('..') blocked by IO sandbox; \
                     call set_allow_unsafe_io_paths(true) to override",
                    fn_name, path
                ),
            },
            None,
        ));
    }
    Ok(())
}

// ============================================================================
// 注册函数
// ============================================================================

/// 注册所有 IO 函数到 BuiltinRegistry
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "input" => builtin_input, arity = 0,
            signature = "input(prompt?) -> string",
            desc = "从标准输入读取一行。可选参数 prompt 作为提示信息。";
        "read_file" => builtin_read_file, arity = 1,
            signature = "read_file(path) -> string",
            desc = "读取文件的全部内容，返回字符串。";
        "write_file" => builtin_write_file, arity = 2,
            signature = "write_file(path, content) -> nil",
            desc = "将内容写入文件（覆盖已有内容）。";
        "append_file" => builtin_append_file, arity = 2,
            signature = "append_file(path, content) -> nil",
            desc = "将内容追加到文件末尾。";
    }
}

// ============================================================================
// 内置函数实现
// ============================================================================

/// **input(prompt?)** → string
///
/// 从标准输入读取一行。可选参数 prompt 作为提示信息。
fn builtin_input(args: &[Value]) -> Result<Value, NuzoError> {
    // 打印提示信息（如果有）
    if !args.is_empty() {
        if args[0].is_string() {
            let prompt = args[0].as_string_opt().unwrap_or_default();
            print!("{}", prompt);
        } else {
            print!("{}", args[0]);
        }
        io::stdout().flush().map_err(|e| {
            NuzoError::internal(
                nuzo_values::InternalError::IoError {
                    message: format!("flush stdout failed: {}", e),
                },
                None,
            )
        })?;
    }

    let mut line = String::new();
    io::stdin().read_line(&mut line).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError { message: format!("read stdin failed: {}", e) },
            None,
        )
    })?;

    // 去除末尾换行
    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
    Ok(Value::from_string(trimmed))
}

/// **read_file(path)** → string
///
/// 读取文件的全部内容，返回字符串。
fn builtin_read_file(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let path = require_string_arg(&args[0], "read_file")?;
    validate_io_path(&path, "read_file")?;

    let metadata = fs::metadata(&path).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!("read_file(\"{}\") failed: {}", path, e),
            },
            None,
        )
    })?;
    if metadata.len() > MAX_READ_FILE_BYTES {
        return Err(NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!(
                    "read_file(\"{}\") failed: file size {} bytes exceeds limit {} bytes",
                    path,
                    metadata.len(),
                    MAX_READ_FILE_BYTES
                ),
            },
            None,
        ));
    }

    let content = fs::read_to_string(&path).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!("read_file(\"{}\") failed: {}", path, e),
            },
            None,
        )
    })?;

    Ok(Value::from_string(&content))
}

/// **write_file(path, content)** → nil
///
/// 将内容写入文件（覆盖已有内容）。
fn builtin_write_file(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let path = require_string_arg(&args[0], "write_file")?;
    let content = require_string_arg(&args[1], "write_file")?;
    validate_io_path(&path, "write_file")?;

    fs::write(&path, &content).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!("write_file(\"{}\") failed: {}", path, e),
            },
            None,
        )
    })?;

    Ok(NIL)
}

/// **append_file(path, content)** → nil
///
/// 将内容追加到文件末尾。
fn builtin_append_file(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let path = require_string_arg(&args[0], "append_file")?;
    let content = require_string_arg(&args[1], "append_file")?;
    validate_io_path(&path, "append_file")?;

    // 确保父目录存在
    if let Some(parent) = Path::new(&path).parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }

    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!("append_file(\"{}\") failed: {}", path, e),
            },
            None,
        )
    })?;

    file.write_all(content.as_bytes()).map_err(|e| {
        NuzoError::internal(
            nuzo_values::InternalError::IoError {
                message: format!("append_file write failed: {}", e),
            },
            None,
        )
    })?;

    Ok(NIL)
}

// ============================================================================
// 辅助函数
// ============================================================================

fn require_string_arg(val: &Value, fn_name: &str) -> Result<String, NuzoError> {
    if !val.is_string() {
        return Err(NuzoError::type_mismatch(
            format!("string (arg of {})", fn_name),
            val.type_name(),
        ));
    }
    val.as_string_opt().ok_or_else(|| {
        NuzoError::type_mismatch(format!("string (arg of {})", fn_name), "invalid string")
    })
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// P1-4 回归测试：沙箱模式下拒绝路径穿越。
    #[test]
    fn test_io_sandbox_path_traversal() {
        // 保存原状态并在测试结束时恢复，避免污染其他测试
        let prev = is_unsafe_io_paths_allowed();
        // 确保沙箱启用
        set_allow_unsafe_io_paths(false);

        // 各种路径穿越形式都应被沙箱拒绝
        // Windows 路径（反斜杠）只在 Windows 上测试，因为 Unix 上反斜杠不是路径分隔符
        #[cfg(target_os = "windows")]
        let attack_paths = [
            "../../../etc/passwd",
            "..\\..\\windows\\system32",
            "data/../../secret.txt",
            "./../escape.txt",
            "a/b/../../../c",
        ];
        #[cfg(not(target_os = "windows"))]
        let attack_paths = [
            "../../../etc/passwd",
            "data/../../secret.txt",
            "./../escape.txt",
            "a/b/../../../c",
            "..//..//etc/shadow",
        ];
        for path in attack_paths {
            let val = Value::from_string(path);
            // read_file
            let r = builtin_read_file(&[val]);
            assert!(r.is_err(), "read_file({:?}) should be blocked by sandbox", path);
            let err_msg = format!("{}", r.unwrap_err());
            assert!(
                err_msg.contains("path traversal") && err_msg.contains(".."),
                "error should mention path traversal: got {}",
                err_msg
            );

            // write_file
            let r = builtin_write_file(&[val, Value::from_string("x")]);
            assert!(r.is_err(), "write_file({:?}) should be blocked by sandbox", path);

            // append_file
            let r = builtin_append_file(&[val, Value::from_string("x")]);
            assert!(r.is_err(), "append_file({:?}) should be blocked by sandbox", path);
        }

        // 恢复原状态
        set_allow_unsafe_io_paths(prev);
    }

    /// P1-4 回归测试：沙箱模式下允许安全路径（无 `..` 组件）。
    #[test]
    fn test_io_sandbox_allows_safe_paths() {
        let prev = is_unsafe_io_paths_allowed();
        set_allow_unsafe_io_paths(false);

        // 这些路径不含 `..`，沙箱不应拒绝（即使文件不存在，路径校验也应通过）
        // 注：校验通过后 fs::metadata 会因文件不存在而失败，这是预期的。
        let safe_paths = [
            "config.txt",
            "data/file.txt",
            "/home/user/file.txt", // Unix 绝对路径
            "C:\\Users\\file.txt", // Windows 绝对路径
            "./local.txt",         // 当前目录前缀（合法）
            "a/b/c/d.txt",         // 纯相对路径
        ];
        for path in safe_paths {
            let val = Value::from_string(path);
            let r = builtin_read_file(&[val]);
            // 校验通过 → 进入 fs::metadata，因文件不存在而失败（但错误信息不同）
            if let Err(e) = r {
                let msg = format!("{}", e);
                assert!(
                    !msg.contains("path traversal"),
                    "safe path {:?} should not be blocked by sandbox: got {}",
                    path,
                    msg
                );
            }
        }

        set_allow_unsafe_io_paths(prev);
    }

    /// P1-4 回归测试：allow_unsafe_paths 模式下允许路径穿越（向后兼容）。
    #[test]
    fn test_io_sandbox_unsafe_mode_allows_traversal() {
        let prev = is_unsafe_io_paths_allowed();
        set_allow_unsafe_io_paths(true);

        // 路径校验应直接通过，进入 fs::metadata（文件不存在则失败，但不是沙箱拒绝）
        let val = Value::from_string("../../../etc/nonexistent");
        let r = builtin_read_file(&[val]);
        if let Err(e) = r {
            let msg = format!("{}", e);
            assert!(
                !msg.contains("path traversal"),
                "unsafe mode should not block traversal paths: got {}",
                msg
            );
        }

        set_allow_unsafe_io_paths(prev);
    }
}
