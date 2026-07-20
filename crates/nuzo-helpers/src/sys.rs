//! # 系统调用模块
//!
//! 本模块提供**进程环境和标准 IO** 相关的系统调用函数。
//!
//! ## 可用函数
//!
//! | 函数 | 签名 | 说明 |
//! |------|------|------|
//! | `sys_args` | `args() -> array` | 返回命令行参数数组 |
//! | `sys_env` | `env() -> dict` | 返回所有环境变量 |
//! | `sys_getenv` | `getenv(name) -> string \| nil` | 获取指定环境变量 |
//! | `sys_exit` | `exit(code) -> !` | 终止进程 |
//! | `sys_print` | `print(args...) -> nil` | 输出到 stdout（无换行）|
//! | `sys_println` | `println(args...) -> nil` | 输出到 stdout（带换行）|
//! | `sys_eprintln` | `eprintln(args...) -> nil` | 输出到 stderr（带换行）|
//! | `sys_list_dir` | `list_dir(path) -> array` | 列出目录条目 |
//! | `sys_mkdir` | `mkdir(path) -> bool` | 创建目录 |
//! | `sys_exists` | `exists(path) -> bool` | 检查路径是否存在 |
//! | `sys_remove` | `remove(path) -> bool` | 删除文件或空目录 |
//! | `sys_rename` | `rename(old, new) -> bool` | 重命名/移动 |
//!
//! ## 注册迁移说明
//!
//! - 模块导出（`pub mod sys`）由 T8 处理
//! - `print`/`println` 注册迁移（含输出捕获集成）由 T9 统一处理
//! - 当前 `sys_print`/`sys_println`/`sys_eprintln` 直接写入 stdout/stderr，
//!   输出捕获（output capture）集成在 T9 注册迁移时完成

use std::fs;
use std::path::Path;

use nuzo_core::Value;
use nuzo_values::{HeapObject, InternalError, NIL, NuzoDict, NuzoError, ValueExt};

// ============================================================================
// 进程环境
// ============================================================================

/// **args()** -> array
///
/// 返回命令行参数数组，第 0 个元素为脚本路径。
pub fn sys_args(_args: &[Value]) -> Result<Value, NuzoError> {
    let items: Vec<Value> = std::env::args().map(|s| Value::from_string(&s)).collect();
    Ok(Value::from_heap_object_gc(HeapObject::Array(items)))
}

/// **env()** -> dict
///
/// 返回包含所有环境变量的字典。
pub fn sys_env(_args: &[Value]) -> Result<Value, NuzoError> {
    let mut dict = NuzoDict::new();
    for (key, value) in std::env::vars() {
        let key_index = Value::from_string(&key)
            .string_index()
            .expect("interned string must have a valid pool index");
        dict.insert(key_index, Value::from_string(&value));
    }
    Ok(Value::from_heap_object_gc(HeapObject::Dict(dict)))
}

/// **getenv(name)** -> string | nil
///
/// 返回指定环境变量的值。变量不存在时返回 nil。
pub fn sys_getenv(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    if !args[0].is_string() {
        return Err(NuzoError::type_mismatch("string", args[0].type_name()));
    }
    let name = args[0]
        .as_string_opt()
        .ok_or_else(|| NuzoError::type_mismatch("string (arg of getenv)", "invalid string"))?;
    match std::env::var(&name) {
        Ok(value) => Ok(Value::from_string(&value)),
        Err(_) => Ok(NIL),
    }
}

/// **exit(code)** -> !
///
/// 以指定退出码终止进程。code 必须是整数（Smi）。
pub fn sys_exit(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    if !args[0].is_smi() {
        return Err(NuzoError::type_mismatch("integer (smi)", args[0].type_name()));
    }
    let code = args[0].as_smi() as i32;
    std::process::exit(code)
}

// ============================================================================
// IO 迁移（从 builtins.rs 迁移）
// ============================================================================
//
// 注意：输出捕获（output capture）集成由 T9 在注册迁移时统一处理。
// 当前实现直接写入 stdout/stderr，与 builtins.rs 的 builtin_print 行为一致
// （未启用捕获时也是直接写入）。

/// **print(args...)** -> nil
///
/// 将参数输出到 stdout，无换行。参数间以空格分隔。
pub fn sys_print(args: &[Value]) -> Result<Value, NuzoError> {
    let output = args_join(args);
    print!("{}", output);
    Ok(NIL)
}

/// **println(args...)** -> nil
///
/// 将参数输出到 stdout，带换行。参数间以空格分隔。
pub fn sys_println(args: &[Value]) -> Result<Value, NuzoError> {
    let output = args_join(args);
    println!("{}", output);
    Ok(NIL)
}

/// **eprintln(args...)** -> nil
///
/// 将参数输出到 stderr，带换行。参数间以空格分隔。
pub fn sys_eprintln(args: &[Value]) -> Result<Value, NuzoError> {
    let output = args_join(args);
    eprintln!("{}", output);
    Ok(NIL)
}

// ============================================================================
// 文件系统
// ============================================================================

/// **list_dir(path)** -> array
///
/// 返回目录条目数组（文件名列表）。
/// path 不存在或指向文件时返回错误；空目录返回空数组。
pub fn sys_list_dir(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let path = require_string_arg(&args[0], "list_dir")?;

    let entries: Vec<Value> = fs::read_dir(&path)
        .map_err(|e| io_error("list_dir", &path, &e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| Value::from_string(&entry.file_name().to_string_lossy()))
        .collect();

    Ok(Value::from_heap_object_gc(HeapObject::Array(entries)))
}

/// **mkdir(path)** -> bool
///
/// 创建目录。path 已存在时返回 false（不报错）；创建成功返回 true。
pub fn sys_mkdir(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let path = require_string_arg(&args[0], "mkdir")?;

    match fs::create_dir(&path) {
        Ok(()) => Ok(Value::from_bool(true)),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Value::from_bool(false)),
        Err(e) => Err(io_error("mkdir", &path, &e)),
    }
}

/// **exists(path)** -> bool
///
/// 检查路径是否存在。空字符串返回 false。
pub fn sys_exists(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let path = require_string_arg(&args[0], "exists")?;

    if path.is_empty() {
        return Ok(Value::from_bool(false));
    }
    Ok(Value::from_bool(Path::new(&path).exists()))
}

/// **remove(path)** -> bool
///
/// 删除文件或空目录。文件不存在返回 false；删除成功返回 true；
/// 非空目录或其他错误返回错误。
pub fn sys_remove(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let path = require_string_arg(&args[0], "remove")?;

    // First try removing as a file
    match fs::remove_file(&path) {
        Ok(()) => return Ok(Value::from_bool(true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Value::from_bool(false)),
        Err(_) => {}
    }
    // Fall through: try removing as a (empty) directory
    match fs::remove_dir(&path) {
        Ok(()) => Ok(Value::from_bool(true)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::from_bool(false)),
        Err(e) => Err(io_error("remove", &path, &e)),
    }
}

/// **rename(old, new)** -> bool
///
/// 重命名或移动文件/目录。成功返回 true；失败返回错误
/// （Windows 上目标已存在也会返回错误）。
pub fn sys_rename(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let old = require_string_arg(&args[0], "rename")?;
    let new = require_string_arg(&args[1], "rename")?;

    match fs::rename(&old, &new) {
        Ok(()) => Ok(Value::from_bool(true)),
        Err(e) => Err(NuzoError::internal(
            InternalError::IoError {
                message: format!("rename(\"{}\", \"{}\") failed: {}", old, new, e),
            },
            None,
        )),
    }
}

// ============================================================================
// 辅助函数
// ============================================================================

/// 将参数列表以空格连接为字符串，使用 concat_repr 进行格式化。
fn args_join(args: &[Value]) -> String {
    args.iter().map(|v| v.concat_repr()).collect::<Vec<_>>().join(" ")
}

/// 校验参数为字符串并返回其值。用于文件系统函数的参数校验。
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

/// 构造 IO 错误，统一文件系统函数的错误消息格式。
fn io_error(fn_name: &str, path: &str, e: &std::io::Error) -> NuzoError {
    NuzoError::internal(
        InternalError::IoError { message: format!("{}(\"{}\") failed: {}", fn_name, path, e) },
        None,
    )
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // 辅助：提取返回值中的堆对象
    // =========================================================================

    /// 从 Value 中提取 Array 引用，失败则 panic。
    fn expect_array(val: Value) -> Vec<Value> {
        let obj = val.as_heap_object_opt().expect("expected heap object (array)");
        match obj.as_ref() {
            HeapObject::Array(arr) => arr.clone(),
            other => panic!("expected Array, got {:?}", other.type_name()),
        }
    }

    /// 从 Value 中提取 Dict 引用，失败则 panic。
    fn expect_dict(val: Value) -> NuzoDict {
        let obj = val.as_heap_object_opt().expect("expected heap object (dict)");
        match obj.as_ref() {
            HeapObject::Dict(d) => d.clone(),
            other => panic!("expected Dict, got {:?}", other.type_name()),
        }
    }

    // =========================================================================
    // sys_getenv 测试
    // =========================================================================

    #[test]
    fn test_sys_getenv_exists() {
        let var_name = format!("NUZO_TEST_GETENV_{}", std::process::id());
        // SAFETY: 测试使用唯一的变量名，不会与其他测试冲突。
        unsafe {
            std::env::set_var(&var_name, "hello");
        }
        let key = Value::from_string(&var_name);
        let result = sys_getenv(&[key]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_string());
        assert_eq!(val.as_string_opt().unwrap(), "hello");
        // SAFETY: 清理测试环境变量。
        unsafe {
            std::env::remove_var(&var_name);
        }
    }

    #[test]
    fn test_sys_getenv_not_exists() {
        let key = Value::from_string("NUZO_NONEXISTENT_VAR_XYZ_999");
        let result = sys_getenv(&[key]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_nil(), "nonexistent env var should return nil");
    }

    #[test]
    fn test_sys_getenv_wrong_type() {
        // 传入非字符串参数（Smi）
        let result = sys_getenv(&[Value::from_smi(42)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_getenv_wrong_arg_count() {
        // 0 个参数
        let result = sys_getenv(&[]);
        assert!(result.is_err());
        // 2 个参数
        let result = sys_getenv(&[Value::from_string("A"), Value::from_string("B")]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_exists 测试
    // =========================================================================

    #[test]
    fn test_sys_exists_current_dir() {
        let path = Value::from_string(".");
        let result = sys_exists(&[path]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_bool());
        assert!(val.as_bool(), "current dir should exist");
    }

    #[test]
    fn test_sys_exists_nonexistent() {
        let path = Value::from_string("/nonexistent/path/xyz_nuzo_999");
        let result = sys_exists(&[path]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_bool());
        assert!(!val.as_bool(), "nonexistent path should return false");
    }

    #[test]
    fn test_sys_exists_empty_string() {
        let path = Value::from_string("");
        let result = sys_exists(&[path]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_bool());
        assert!(!val.as_bool(), "empty string should return false");
    }

    #[test]
    fn test_sys_exists_wrong_type() {
        let result = sys_exists(&[Value::from_smi(42)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_exists_wrong_arg_count() {
        let result = sys_exists(&[]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_mkdir + sys_remove 测试（配合使用）
    // =========================================================================

    #[test]
    fn test_sys_mkdir_and_remove_lifecycle() {
        let test_dir = format!("nuzo_test_mkdir_{}", std::process::id());
        // 确保测试前目录不存在
        let _ = fs::remove_dir(&test_dir);

        // mkdir 首次 → true
        let path = Value::from_string(&test_dir);
        let result = sys_mkdir(&[path]);
        assert!(result.is_ok());
        assert!(result.unwrap().as_bool(), "mkdir should succeed on new dir");

        // exists 确认
        let result = sys_exists(&[path]);
        assert!(result.unwrap().as_bool(), "dir should exist after mkdir");

        // mkdir 再次 → false（已存在）
        let result = sys_mkdir(&[path]);
        assert!(result.is_ok());
        assert!(!result.unwrap().as_bool(), "mkdir on existing dir should return false");

        // remove → true
        let result = sys_remove(&[path]);
        assert!(result.is_ok());
        assert!(result.unwrap().as_bool(), "remove should succeed");

        // exists 确认已删除
        let result = sys_exists(&[path]);
        assert!(!result.unwrap().as_bool(), "dir should not exist after remove");
    }

    #[test]
    fn test_sys_mkdir_wrong_type() {
        let result = sys_mkdir(&[Value::from_smi(42)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_mkdir_wrong_arg_count() {
        let result = sys_mkdir(&[]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_remove 测试
    // =========================================================================

    #[test]
    fn test_sys_remove_nonexistent() {
        let path = Value::from_string("nuzo_test_nonexistent_remove_999");
        let result = sys_remove(&[path]);
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(val.is_bool());
        assert!(!val.as_bool(), "removing nonexistent should return false");
    }

    #[test]
    fn test_sys_remove_file() {
        let test_file = format!("nuzo_test_remove_file_{}", std::process::id());
        fs::write(&test_file, "test").expect("failed to create test file");

        let path = Value::from_string(&test_file);
        let result = sys_remove(&[path]);
        assert!(result.is_ok());
        assert!(result.unwrap().as_bool(), "remove should succeed on existing file");

        // 确认文件已删除
        assert!(!Path::new(&test_file).exists());
    }

    #[test]
    fn test_sys_remove_wrong_type() {
        let result = sys_remove(&[Value::from_bool(true)]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_list_dir 测试
    // =========================================================================

    #[test]
    fn test_sys_list_dir_current() {
        let path = Value::from_string(".");
        let result = sys_list_dir(&[path]);
        assert!(result.is_ok());
        let arr = expect_array(result.unwrap());
        // 当前目录应该有文件（至少 Cargo.toml 或 src/ 等）
        assert!(!arr.is_empty(), "current dir should have entries (got empty array)");
        // 每个元素都应该是字符串
        for entry in &arr {
            assert!(entry.is_string(), "dir entries should be strings");
        }
    }

    #[test]
    fn test_sys_list_dir_nonexistent() {
        let path = Value::from_string("/nonexistent/path/xyz_nuzo_999");
        let result = sys_list_dir(&[path]);
        assert!(result.is_err(), "list_dir on nonexistent should error");
    }

    #[test]
    fn test_sys_list_dir_wrong_type() {
        let result = sys_list_dir(&[Value::from_smi(0)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_list_dir_wrong_arg_count() {
        let result = sys_list_dir(&[]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_rename 测试
    // =========================================================================

    #[test]
    fn test_sys_rename_file() {
        let old_name = format!("nuzo_test_rename_old_{}", std::process::id());
        let new_name = format!("nuzo_test_rename_new_{}", std::process::id());

        // 清理可能残留的文件
        let _ = fs::remove_file(&old_name);
        let _ = fs::remove_file(&new_name);

        // 创建测试文件
        fs::write(&old_name, "rename_test_content").expect("failed to create test file");

        let old_val = Value::from_string(&old_name);
        let new_val = Value::from_string(&new_name);
        let result = sys_rename(&[old_val, new_val]);
        assert!(result.is_ok());
        assert!(result.unwrap().as_bool(), "rename should succeed");

        // 验证新文件存在且旧文件不存在
        assert!(Path::new(&new_name).exists(), "new file should exist");
        assert!(!Path::new(&old_name).exists(), "old file should not exist");

        // 读取内容验证
        let content = fs::read_to_string(&new_name).expect("failed to read renamed file");
        assert_eq!(content, "rename_test_content");

        // 清理
        let _ = fs::remove_file(&new_name);
    }

    #[test]
    fn test_sys_rename_nonexistent() {
        let old_val = Value::from_string("nuzo_test_rename_nonexistent_old_999");
        let new_val = Value::from_string("nuzo_test_rename_nonexistent_new_999");
        let result = sys_rename(&[old_val, new_val]);
        assert!(result.is_err(), "rename nonexistent should error");
    }

    #[test]
    fn test_sys_rename_wrong_arg_count() {
        // 1 个参数
        let result = sys_rename(&[Value::from_string("a")]);
        assert!(result.is_err());
        // 3 个参数
        let result = sys_rename(&[
            Value::from_string("a"),
            Value::from_string("b"),
            Value::from_string("c"),
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_rename_wrong_type() {
        let result = sys_rename(&[Value::from_smi(1), Value::from_smi(2)]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_args 测试
    // =========================================================================

    #[test]
    fn test_sys_args_returns_array() {
        let result = sys_args(&[]);
        assert!(result.is_ok());
        let arr = expect_array(result.unwrap());
        // 至少包含程序名（测试二进制路径）
        assert!(!arr.is_empty(), "args should have at least the program name");
        // 第 0 个元素应该是字符串（程序路径）
        assert!(arr[0].is_string(), "args[0] should be a string (program path)");
    }

    // =========================================================================
    // sys_env 测试
    // =========================================================================

    #[test]
    fn test_sys_env_returns_dict() {
        // 使用唯一变量名作为锚点，避免依赖环境变量总数（并发测试下
        // std::env::vars() 会被其他测试的 set_var/remove_var 修改，
        // 导致 dict.len() 与 std::env::vars().count() 不一致）。
        let var_name = format!("NUZO_TEST_SYS_ENV_DICT_{}", std::process::id());
        // SAFETY: 变量名包含 PID 且前缀唯一，不会与其他测试冲突。
        unsafe {
            std::env::set_var(&var_name, "env_test_value");
        }

        let result = sys_env(&[]);

        // SAFETY: 测试结束前清理环境变量。
        unsafe {
            std::env::remove_var(&var_name);
        }

        assert!(result.is_ok());
        let dict = expect_dict(result.unwrap());
        // 环境变量字典不应为空（进程总会有 PATH 等变量）
        assert!(!dict.is_empty(), "env dict should not be empty");

        // 验证刚设置的变量在字典中且值正确（确定性的单变量检查，
        // 不受并发测试影响）
        let key_index = Value::from_string(&var_name)
            .string_index()
            .expect("interned string must have a valid pool index");
        let val = dict
            .get(key_index)
            .expect("env dict should contain the test var that was set before sys_env");
        assert!(val.is_string(), "env var value should be a string");
        assert_eq!(
            val.as_string_opt().unwrap(),
            "env_test_value",
            "env var value should match what was set before sys_env"
        );
    }

    // =========================================================================
    // sys_exit 测试（仅验证参数校验，不实际退出）
    // =========================================================================

    #[test]
    fn test_sys_exit_wrong_type() {
        // 传入非 Smi 参数应返回错误（而不是退出进程）
        let result = sys_exit(&[Value::from_string("not_a_number")]);
        assert!(result.is_err());
    }

    #[test]
    fn test_sys_exit_wrong_arg_count() {
        let result = sys_exit(&[]);
        assert!(result.is_err());
        let result = sys_exit(&[Value::from_smi(0), Value::from_smi(1)]);
        assert!(result.is_err());
    }

    // =========================================================================
    // sys_print / sys_println / sys_eprintln 测试
    // =========================================================================

    #[test]
    fn test_sys_print_returns_nil() {
        let result = sys_print(&[Value::from_string("test")]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_nil(), "print should return nil");
    }

    #[test]
    fn test_sys_println_returns_nil() {
        let result = sys_println(&[Value::from_string("test")]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_nil(), "println should return nil");
    }

    #[test]
    fn test_sys_eprintln_returns_nil() {
        let result = sys_eprintln(&[Value::from_string("test")]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_nil(), "eprintln should return nil");
    }

    #[test]
    fn test_sys_print_multi_args() {
        // 多参数以空格分隔
        let result =
            sys_print(&[Value::from_string("hello"), Value::from_smi(42), Value::from_bool(true)]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sys_print_no_args() {
        // 无参数也应正常返回 nil
        let result = sys_print(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_nil());
    }
}
