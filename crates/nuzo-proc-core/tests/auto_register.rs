//! 自动注册机制集成测试
//!
//! 验证 `define_constants!` 宏生成的 `#[ctor::ctor]` 函数能在程序启动时
//! 自动调用 `__register_constants()`，将常量注册到全局注册表。

use nuzo_proc_core::define_constants;
use nuzo_proc_core::hardcode::registry;

// 定义测试常量
define_constants! {
    /// 测试常量：栈大小
    pub TEST_AUTO_REGISTER_STACK_SIZE: usize = 65536;

    /// 测试常量：GC 阈值
    pub TEST_AUTO_REGISTER_GC_THRESHOLD: usize = 10 * 1024 * 1024;

    /// 测试常量：浮点值
    pub TEST_AUTO_REGISTER_RATIO: f64 = 0.5;

    /// 测试常量：字符串
    pub TEST_AUTO_REGISTER_NAME: &str = "<test>";
}

#[test]
fn test_auto_registered_constants_exist() {
    // 由于 #[ctor::ctor] 在程序启动时自动执行，
    // 测试启动时注册表应已包含所有 define_constants! 定义的常量
    assert!(
        registry::exists("TEST_AUTO_REGISTER_STACK_SIZE"),
        "TEST_AUTO_REGISTER_STACK_SIZE 应已自动注册"
    );
    assert!(
        registry::exists("TEST_AUTO_REGISTER_GC_THRESHOLD"),
        "TEST_AUTO_REGISTER_GC_THRESHOLD 应已自动注册"
    );
    assert!(registry::exists("TEST_AUTO_REGISTER_RATIO"), "TEST_AUTO_REGISTER_RATIO 应已自动注册");
    assert!(registry::exists("TEST_AUTO_REGISTER_NAME"), "TEST_AUTO_REGISTER_NAME 应已自动注册");
}

#[test]
fn test_auto_registered_constant_values() {
    let info = registry::get("TEST_AUTO_REGISTER_STACK_SIZE")
        .expect("TEST_AUTO_REGISTER_STACK_SIZE 应已注册");
    assert_eq!(info.name, "TEST_AUTO_REGISTER_STACK_SIZE");
    assert_eq!(info.type_name, "usize");
    assert_eq!(info.value_str, "65536");
    assert!(info.is_integer());
    assert_eq!(info.parse_as_i64(), Some(65536));
}

#[test]
fn test_auto_registered_float_constant() {
    let info =
        registry::get("TEST_AUTO_REGISTER_RATIO").expect("TEST_AUTO_REGISTER_RATIO 应已注册");
    assert_eq!(info.type_name, "f64");
    assert_eq!(info.value_str, "0.5");
    assert!(info.is_float());
    assert!(!info.is_integer());
    assert_eq!(info.parse_as_f64(), Some(0.5));
}

#[test]
fn test_auto_registered_string_constant() {
    let info = registry::get("TEST_AUTO_REGISTER_NAME").expect("TEST_AUTO_REGISTER_NAME 应已注册");
    assert_eq!(info.type_name, "&str");
    assert_eq!(info.value_str, "\"<test>\"");
    assert!(info.is_string());
    assert!(!info.is_integer());
}

#[test]
fn test_const_values_accessible() {
    // 验证 const 常量本身可正常访问
    assert_eq!(TEST_AUTO_REGISTER_STACK_SIZE, 65536);
    assert_eq!(TEST_AUTO_REGISTER_GC_THRESHOLD, 10 * 1024 * 1024);
    assert_eq!(TEST_AUTO_REGISTER_RATIO, 0.5);
    assert_eq!(TEST_AUTO_REGISTER_NAME, "<test>");
}

#[test]
fn test_by_module_filter() {
    // 验证按模块路径过滤
    let module_constants = registry::by_module(module_path!());
    assert!(
        module_constants.iter().any(|c| c.name == "TEST_AUTO_REGISTER_STACK_SIZE"),
        "by_module 应返回当前模块的常量"
    );
}

#[test]
fn test_by_type_filter() {
    // 验证按类型过滤
    let usize_constants = registry::by_type("usize");
    assert!(
        usize_constants.iter().any(|c| c.name == "TEST_AUTO_REGISTER_STACK_SIZE"),
        "by_type(\"usize\") 应返回 usize 类型的常量"
    );

    let f64_constants = registry::by_type("f64");
    assert!(
        f64_constants.iter().any(|c| c.name == "TEST_AUTO_REGISTER_RATIO"),
        "by_type(\"f64\") 应返回 f64 类型的常量"
    );
}
