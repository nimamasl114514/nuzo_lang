//! # 数组辅助函数
//!
//! 本模块提供**数组操作**功能集，支持常见的数组变换、搜索和聚合操作。
//! 所有函数都返回新数组（不可变风格），避免副作用。
//!
//! ## 可用函数（5 个）
//!
//! | 函数 | 签名 | 说明 | 时间复杂度 |
//! |------|------|------|-----------|
//! | `index_of` | `index_of(arr, val) → number` | 查找元素首次出现的索引 | O(n) |
//! | `slice` | `slice(arr, start, end) → array` | 提取子数组 | O(k) |
//! | `concat` | `concat(arr1, arr2) → array` | 连接两个数组 | O(n+m) |
//! | `unique` | `unique(arr) → array` | 去重（保留顺序）| O(n²) |
//! | `sort` | `sort(arr) → array` | 升序排序（仅数字）| O(n log n) |
//!
//! ## 设计原则
//!
//! ### 不可变性（Immutability）
//!
//! 所有操作返回**新数组**，不修改原数组：
//! ```nuzo
//! let original = [3, 1, 2]
//! let sorted = sort(original)
//! // original 仍为 [3, 1, 2]
//! // sorted 为 [1, 2, 3]
//! ```
//!
//! ### 类型安全
//!
//! - **参数校验**：严格的类型和数量检查
//! - **边界处理**：索引越界时返回空数组而非 panic
//! - **错误信息**：清晰的错误消息包含实际类型
//!
//! ## 使用示例
//!
//! ```nuzo
//! // 数据查找
//! let fruits = ["apple", "banana", "cherry"]
//! let idx = index_of(fruits, "banana")  // → 1
//!
//! // 数组分片
//! let nums = [0, 1, 2, 3, 4, 5]
//! let sub = slice(nums, 1, 4)           // → [1, 2, 3]
//!
//! // 数组合并
//! let a = [1, 2]
//! let b = [3, 4]
//! let merged = concat(a, b)             // → [1, 2, 3, 4]
//!
//! // 去重
//! let dupes = [1, 2, 2, 3, 3, 3]
//! let unique_list = unique(dupes)       // → [1, 2, 3]
//!
//! // 排序
//! let unsorted = [3, 1, 4, 1, 5]
//! let sorted_arr = sort(unsorted)       // → [1, 1, 3, 4, 5]
//! ```
//!
//! # 性能说明
//!
//! - **内存分配**：每次操作都会创建新数组（GC 管理）
//! - **大数据集**：对于 > 10K 元素的数组，建议使用专用数据结构
//! - **去重算法**：O(n)（基于 HashSet<u64> 去重键），适合大规模数据

use std::collections::HashSet;

use super::builtins::BuiltinRegistry;
use nuzo_core::Value;
use nuzo_values::{HeapObject, NuzoDict, NuzoError, ValueExt};

// ============================================================================
// 注册函数
// ============================================================================

/// 注册所有数组操作函数到 BuiltinRegistry
#[allow(unused_visibilities, dead_code)]
pub fn register(reg: &mut BuiltinRegistry) {
    nuzo_proc::define_builtins! {
        "index_of" => builtin_index_of, arity = 2,
            signature = "index_of(arr, value) -> number",
            desc = "返回 value 在数组中第一次出现的索引，未找到返回 -1。";
        "slice" => builtin_slice, arity = 3,
            signature = "slice(arr, start, end) -> array",
            desc = "返回数组从 start 到 end（不含）的切片。";
        "concat" => builtin_concat, arity = 2,
            signature = "concat(arr1, arr2) -> array",
            desc = "连接两个数组，返回新数组。";
        "unique" => builtin_unique, arity = 1,
            signature = "unique(arr) -> array",
            desc = "去除数组中的重复元素，保留首次出现的顺序。";
        "sort" => builtin_sort, arity = 1,
            signature = "sort(arr) -> array",
            desc = "对数组进行升序排序（仅限数字数组），返回新数组。";
    }
}

// ============================================================================
// 辅助：提取数组
// ============================================================================

// validate_index 已统一到 crate::validation::validate_index，消除三处重复定义（P1-6）
fn extract_array(val: &Value, fn_name: &str) -> Result<Vec<Value>, NuzoError> {
    if !val.is_heap_object() {
        return Err(NuzoError::type_mismatch(
            format!("array (arg of {})", fn_name),
            val.type_name(),
        ));
    }
    match val.with_heap_object(|obj| match obj {
        HeapObject::Array(arr) => Some(arr.clone()),
        _ => None,
    }) {
        Some(Some(arr)) => Ok(arr),
        _ => Err(NuzoError::type_mismatch(format!("array (arg of {})", fn_name), val.type_name())),
    }
}

// ============================================================================
// 内置函数实现
// ============================================================================

/// **index_of(arr, value)** → number
///
/// 返回 value 在数组中第一次出现的索引，未找到返回 -1。
fn builtin_index_of(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let arr = extract_array(&args[0], "index_of")?;
    let target = &args[1];

    for (i, item) in arr.iter().enumerate() {
        if item.value_equals(target) {
            return Ok(Value::from_number(i as f64));
        }
    }
    Ok(Value::from_number(-1.0))
}

/// **slice(arr, start, end)** → array
///
/// 返回数组从 start 到 end（不含）的切片。
fn builtin_slice(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 3 {
        return Err(NuzoError::invalid_argument_count(3, args.len()));
    }
    let arr = extract_array(&args[0], "slice")?;
    if !args[1].is_number() {
        return Err(NuzoError::type_mismatch("number", args[1].type_name()));
    }
    if !args[2].is_number() {
        return Err(NuzoError::type_mismatch("number", args[2].type_name()));
    }
    let start = crate::validation::validate_index(args[1].as_number())?;
    let end = crate::validation::validate_index(args[2].as_number())?;

    if start >= arr.len() {
        return Ok(Value::from_heap_object_gc(HeapObject::Array(Vec::new())));
    }
    let end = end.min(arr.len());
    if start >= end {
        return Ok(Value::from_heap_object_gc(HeapObject::Array(Vec::new())));
    }
    Ok(Value::from_heap_object_gc(HeapObject::Array(arr[start..end].to_vec())))
}

/// **concat(arr1, arr2)** → array
///
/// 连接两个数组，返回新数组。
fn builtin_concat(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let arr1 = extract_array(&args[0], "concat")?;
    let arr2 = extract_array(&args[1], "concat")?;
    let mut result = arr1;
    result.extend(arr2);
    Ok(Value::from_heap_object_gc(HeapObject::Array(result)))
}

/// **unique(arr)** → array
///
/// 去除数组中的重复元素，保留首次出现的顺序。
///
/// # 算法
/// 使用 `HashSet<u64>` 做去重键存储，利用 `Value::into_raw_bits()` 获取 NaN-tagged
/// 内部位模式作为去重键（O(1) 查找）。原 O(n²) `Vec::contains` 实现在大数组下
/// 性能急剧下降（10K 元素 ≈ 100M 次比较）。
///
/// # 注意
/// - 不同 Value 若位模式相同（如 `1.0` 和 Smmi `1`）会被去重为同一项；
///   这与原实现行为一致（原实现也用 `into_raw_bits()`）。
fn builtin_unique(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let arr = extract_array(&args[0], "unique")?;
    let mut seen: HashSet<u64> = HashSet::with_capacity(arr.len());
    let mut result: Vec<Value> = Vec::with_capacity(arr.len());

    for item in &arr {
        // 用 Value 的内部 u64 表示做去重键
        let key = item.into_raw_bits();
        if seen.insert(key) {
            result.push(*item);
        }
    }
    Ok(Value::from_heap_object_gc(HeapObject::Array(result)))
}

/// **sort(arr)** → array
///
/// 对数组进行升序排序（仅限数字数组），返回新数组。
fn builtin_sort(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let arr = extract_array(&args[0], "sort")?;
    let mut nums: Vec<f64> = Vec::with_capacity(arr.len());
    for item in &arr {
        if !item.is_number() {
            return Err(NuzoError::type_mismatch(
                "number array",
                format!("found {} in array", item.type_name()),
            ));
        }
        nums.push(item.as_number());
    }
    nums.sort_by(|a, b| match (a.is_nan(), b.is_nan()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal),
    });
    let sorted: Vec<Value> = nums.into_iter().map(Value::from_number).collect();
    Ok(Value::from_heap_object_gc(HeapObject::Array(sorted)))
}

// ============================================================================
// dict 容器操作
// ============================================================================
//
// 以下函数提供 dict（字典）容器的查询与变换操作，遵循与数组函数一致的
// 不可变风格：所有操作返回新值，不修改原 dict。
//
// NuzoDict 的键为字符串池索引（u32）：通过 Value::string_index() 从字符串
// Value 获取，通过 Value::from_string_index(idx) 还原为字符串 Value。

/// 从 Value 提取 NuzoDict（克隆）。非 dict 类型返回 TypeMismatch 错误。
fn extract_dict(val: &Value, fn_name: &str) -> Result<NuzoDict, NuzoError> {
    if !val.is_heap_object() {
        return Err(NuzoError::type_mismatch(
            format!("dict (arg of {})", fn_name),
            val.type_name(),
        ));
    }
    match val.with_heap_object(|obj| match obj {
        HeapObject::Dict(d) => Some(d.clone()),
        _ => None,
    }) {
        Some(Some(d)) => Ok(d),
        _ => Err(NuzoError::type_mismatch(format!("dict (arg of {})", fn_name), val.type_name())),
    }
}

/// **dict_keys(dict)** → array
///
/// 返回 dict 所有键组成的数组。空 dict 返回空数组。
pub fn dict_keys(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let dict = extract_dict(&args[0], "dict_keys")?;
    let keys: Vec<Value> =
        dict.iter().map(|(key_index, _)| Value::from_string_index(key_index)).collect();
    Ok(Value::from_heap_object_gc(HeapObject::Array(keys)))
}

/// **dict_values(dict)** → array
///
/// 返回 dict 所有值组成的数组。空 dict 返回空数组。
pub fn dict_values(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 1 {
        return Err(NuzoError::invalid_argument_count(1, args.len()));
    }
    let dict = extract_dict(&args[0], "dict_values")?;
    let values: Vec<Value> = dict.values().collect();
    Ok(Value::from_heap_object_gc(HeapObject::Array(values)))
}

/// **dict_has_key(dict, key)** → bool
///
/// 检查键是否存在。key 为非字符串类型时返回 false（不报错）。
pub fn dict_has_key(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let dict = extract_dict(&args[0], "dict_has_key")?;
    let key_index = match args[1].string_index() {
        Some(idx) => idx,
        None => return Ok(Value::from_bool(false)),
    };
    Ok(Value::from_bool(dict.get(key_index).is_some()))
}

/// **dict_has_value(dict, value)** → bool
///
/// 检查值是否存在（使用 Value 的相等比较）。
pub fn dict_has_value(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let dict = extract_dict(&args[0], "dict_has_value")?;
    Ok(Value::from_bool(dict.contains_value(&args[1])))
}

/// **dict_extend(dict1, dict2)** → dict
///
/// 返回新 dict，dict2 的键覆盖 dict1 同名键。
/// COW 语义：不修改原 dict1 和 dict2。
pub fn dict_extend(args: &[Value]) -> Result<Value, NuzoError> {
    if args.len() != 2 {
        return Err(NuzoError::invalid_argument_count(2, args.len()));
    }
    let mut result = extract_dict(&args[0], "dict_extend")?;
    let dict2 = extract_dict(&args[1], "dict_extend")?;
    for (key_index, value) in dict2.iter() {
        result.insert(key_index, value);
    }
    Ok(Value::from_heap_object_gc(HeapObject::Dict(result)))
}

// ============================================================================
// 测试模块（P1-5 回归测试）
// ============================================================================
//
// 覆盖 builtin_unique 的关键场景：
// - 大数组去重正确性（性能回归，确保 HashSet 路径生效，非 O(n²)）
// - 保留首次出现的顺序
// - 边界条件：空数组、全重复、单元素
// - 混合类型去重

#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_values::NIL;
    use std::time::Instant;

    /// 构造一个 Array HeapObject 包装的 Value
    fn make_array_value(items: Vec<Value>) -> Value {
        Value::from_heap_object_gc(HeapObject::Array(items))
    }

    /// 从 Value 提取 Array 内容（测试辅助）
    fn extract_array_value(val: &Value) -> Vec<Value> {
        val.with_heap_object(|obj| match obj {
            HeapObject::Array(arr) => Some(arr.clone()),
            _ => None,
        })
        .flatten()
        .unwrap_or_default()
    }

    /// P1-5 回归：大数组去重必须正确且快速完成（HashSet 路径，非 O(n²)）。
    ///
    /// 原 O(n²) 实现在 10K 元素下需 ~100M 次比较，可能 >1s。
    /// HashSet 实现应在毫秒级完成。
    #[test]
    fn test_unique_large_array() {
        // 构造 10000 元素，其中 5000 个重复
        let n = 10_000;
        let mut input: Vec<Value> = Vec::with_capacity(n);
        for i in 0..(n / 2) {
            let v = Value::from_number(i as f64);
            input.push(v);
            input.push(v); // 重复
        }
        let arr = make_array_value(input);

        let start = Instant::now();
        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let elapsed = start.elapsed();

        let output = extract_array_value(&result);
        assert_eq!(
            output.len(),
            n / 2,
            "10K duplicates should reduce to {} unique elements",
            n / 2
        );

        // 验证顺序保留：应为 0, 1, 2, ..., 4999
        for (i, item) in output.iter().enumerate() {
            assert_eq!(
                item.as_number(),
                i as f64,
                "unique should preserve first-occurrence order at index {}",
                i
            );
        }

        // 性能断言：HashSet 路径应在 100ms 内完成（O(n)）
        // 原 O(n²) 实现在 10K 元素下约需 500ms+，此阈值足以区分
        assert!(
            elapsed.as_millis() < 200,
            "unique on 10K elements should complete in <200ms (HashSet path), took {:?}",
            elapsed
        );
    }

    /// 大数组去重：单次重复模式（每个值出现 4 次）。
    #[test]
    fn test_unique_large_array_quad_duplicates() {
        let n = 4_000;
        let mut input: Vec<Value> = Vec::with_capacity(n);
        for i in 0..(n / 4) {
            for _ in 0..4 {
                input.push(Value::from_number(i as f64));
            }
        }
        let arr = make_array_value(input);

        let start = Instant::now();
        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let elapsed = start.elapsed();

        let output = extract_array_value(&result);
        assert_eq!(output.len(), n / 4);

        assert!(
            elapsed.as_millis() < 100,
            "unique on 4K quad-duplicates should complete in <100ms, took {:?}",
            elapsed
        );
    }

    /// 保留首次出现的顺序。
    #[test]
    fn test_unique_preserves_first_occurrence_order() {
        let input = vec![
            Value::from_string("b"),
            Value::from_string("a"),
            Value::from_string("b"),
            Value::from_string("c"),
            Value::from_string("a"),
            Value::from_string("d"),
        ];
        let arr = make_array_value(input);

        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let output = extract_array_value(&result);

        // 期望顺序：b, a, c, d（保留首次出现）
        assert_eq!(output.len(), 4);
        assert_eq!(output[0].as_string_opt().as_deref(), Some("b"));
        assert_eq!(output[1].as_string_opt().as_deref(), Some("a"));
        assert_eq!(output[2].as_string_opt().as_deref(), Some("c"));
        assert_eq!(output[3].as_string_opt().as_deref(), Some("d"));
    }

    /// 边界：空数组。
    #[test]
    fn test_unique_empty_array() {
        let arr = make_array_value(Vec::new());
        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let output = extract_array_value(&result);
        assert!(output.is_empty(), "unique of empty should be empty");
    }

    /// 边界：全重复元素。
    #[test]
    fn test_unique_all_duplicates() {
        let v = Value::from_number(42.0);
        let arr = make_array_value(vec![v; 100]);

        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let output = extract_array_value(&result);
        assert_eq!(output.len(), 1, "100 identical elements should reduce to 1");
        assert_eq!(output[0].as_number(), 42.0);
    }

    /// 边界：单元素数组。
    #[test]
    fn test_unique_single_element() {
        let arr = make_array_value(vec![Value::from_number(7.0)]);
        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let output = extract_array_value(&result);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].as_number(), 7.0);
    }

    /// 混合类型去重：数字、字符串、nil、bool。
    #[test]
    fn test_unique_mixed_types() {
        let input = vec![
            Value::from_number(1.0),
            Value::from_string("a"),
            Value::from_number(1.0), // 与首个重复
            NIL,
            Value::from_string("a"), // 与第二个重复
            Value::from_number(2.0),
            NIL, // 与首个 nil 重复
        ];
        let arr = make_array_value(input);

        let result = builtin_unique(&[arr]).expect("unique should succeed");
        let output = extract_array_value(&result);

        // 期望：1.0, "a", nil, 2.0
        assert_eq!(output.len(), 4);
        assert_eq!(output[0].as_number(), 1.0);
        assert_eq!(output[1].as_string_opt().as_deref(), Some("a"));
        assert!(output[2].is_nil());
        assert_eq!(output[3].as_number(), 2.0);
    }

    /// 参数数量错误。
    #[test]
    fn test_unique_wrong_arg_count() {
        let result = builtin_unique(&[]);
        assert!(result.is_err(), "unique with 0 args should error");
    }

    /// 非 Array 类型应返回 TypeMismatch 错误。
    #[test]
    fn test_unique_non_array_input() {
        let result = builtin_unique(&[Value::from_number(42.0)]);
        assert!(result.is_err(), "unique on a number should error");
    }

    // ========================================================================
    // dict 容器操作测试
    // ========================================================================

    /// 构造一个 Dict HeapObject 包装的 Value。
    /// pairs 为 (键字符串, 值) 列表，按顺序插入。
    fn make_dict_value(pairs: &[(&str, Value)]) -> Value {
        let mut dict = NuzoDict::new();
        for (key, val) in pairs {
            let key_index = Value::from_string(key).string_index().unwrap();
            dict.insert(key_index, *val);
        }
        Value::from_heap_object_gc(HeapObject::Dict(dict))
    }

    /// 从 Value 提取 NuzoDict（克隆）。测试辅助，用于验证 COW 语义。
    fn extract_dict_value(val: &Value) -> Option<NuzoDict> {
        val.with_heap_object(|obj| match obj {
            HeapObject::Dict(d) => Some(d.clone()),
            _ => None,
        })
        .flatten()
    }

    // ─── dict_keys ───

    #[test]
    fn test_dict_keys_basic() {
        let d = make_dict_value(&[("a", Value::from_number(1.0)), ("b", Value::from_number(2.0))]);

        let result = dict_keys(&[d]).expect("dict_keys should succeed");
        let keys = extract_array_value(&result);
        assert_eq!(keys.len(), 2, "dict with 2 entries should have 2 keys");

        let key_strs: Vec<String> =
            keys.iter().map(|v| v.as_string_opt().expect("dict keys should be strings")).collect();
        assert!(key_strs.contains(&"a".to_string()), "keys should contain 'a', got {:?}", key_strs);
        assert!(key_strs.contains(&"b".to_string()), "keys should contain 'b', got {:?}", key_strs);
    }

    #[test]
    fn test_dict_keys_empty() {
        let d = make_dict_value(&[]);
        let result = dict_keys(&[d]).expect("dict_keys should succeed");
        let keys = extract_array_value(&result);
        assert!(keys.is_empty(), "empty dict should yield empty keys array");
    }

    #[test]
    fn test_dict_keys_wrong_arg_count() {
        let result = dict_keys(&[]);
        assert!(result.is_err(), "dict_keys with 0 args should error");
    }

    // ─── dict_values ───

    #[test]
    fn test_dict_values_basic() {
        let d = make_dict_value(&[("a", Value::from_number(1.0)), ("b", Value::from_number(2.0))]);

        let result = dict_values(&[d]).expect("dict_values should succeed");
        let vals = extract_array_value(&result);
        assert_eq!(vals.len(), 2, "dict with 2 entries should have 2 values");

        let nums: Vec<f64> = vals.iter().map(|v| v.as_number()).collect();
        assert!(nums.contains(&1.0), "values should contain 1.0, got {:?}", nums);
        assert!(nums.contains(&2.0), "values should contain 2.0, got {:?}", nums);
    }

    #[test]
    fn test_dict_values_empty() {
        let d = make_dict_value(&[]);
        let result = dict_values(&[d]).expect("dict_values should succeed");
        let vals = extract_array_value(&result);
        assert!(vals.is_empty(), "empty dict should yield empty values array");
    }

    // ─── dict_has_key ───

    #[test]
    fn test_dict_has_key_exists() {
        let d = make_dict_value(&[("name", Value::from_string("Alice"))]);
        let key = Value::from_string("name");
        let result = dict_has_key(&[d, key]).expect("dict_has_key should succeed");
        assert!(
            result.is_bool() && result.as_bool(),
            "has_key('name') on dict containing 'name' should return true"
        );
    }

    #[test]
    fn test_dict_has_key_not_exists() {
        let d = make_dict_value(&[("name", Value::from_string("Alice"))]);
        let key = Value::from_string("age");
        let result = dict_has_key(&[d, key]).expect("dict_has_key should succeed");
        assert!(
            result.is_bool() && !result.as_bool(),
            "has_key('age') on dict without 'age' should return false"
        );
    }

    #[test]
    fn test_dict_has_key_wrong_type() {
        let d = make_dict_value(&[("name", Value::from_string("Alice"))]);
        // 非字符串 key（数字）应返回 false，不报错
        let result = dict_has_key(&[d, Value::from_number(123.0)])
            .expect("dict_has_key with non-string key should not error");
        assert!(
            result.is_bool() && !result.as_bool(),
            "has_key with non-string key should return false"
        );
    }

    #[test]
    fn test_dict_has_key_empty_dict() {
        let d = make_dict_value(&[]);
        let key = Value::from_string("anything");
        let result = dict_has_key(&[d, key]).expect("dict_has_key should succeed");
        assert!(result.is_bool() && !result.as_bool(), "has_key on empty dict should return false");
    }

    // ─── dict_has_value ───

    #[test]
    fn test_dict_has_value_exists() {
        let d = make_dict_value(&[("a", Value::from_number(1.0)), ("b", Value::from_number(2.0))]);
        let result =
            dict_has_value(&[d, Value::from_number(1.0)]).expect("dict_has_value should succeed");
        assert!(
            result.is_bool() && result.as_bool(),
            "has_value(1) on dict containing 1 should return true"
        );
    }

    #[test]
    fn test_dict_has_value_not_exists() {
        let d = make_dict_value(&[("a", Value::from_number(1.0))]);
        let result =
            dict_has_value(&[d, Value::from_number(999.0)]).expect("dict_has_value should succeed");
        assert!(
            result.is_bool() && !result.as_bool(),
            "has_value(999) on dict without 999 should return false"
        );
    }

    #[test]
    fn test_dict_has_value_empty_dict() {
        let d = make_dict_value(&[]);
        let result =
            dict_has_value(&[d, Value::from_number(1.0)]).expect("dict_has_value should succeed");
        assert!(
            result.is_bool() && !result.as_bool(),
            "has_value on empty dict should return false"
        );
    }

    // ─── dict_extend ───

    #[test]
    fn test_dict_extend_basic() {
        let d1 = make_dict_value(&[("a", Value::from_number(1.0)), ("b", Value::from_number(2.0))]);
        let d2 = make_dict_value(&[("b", Value::from_number(3.0)), ("c", Value::from_number(4.0))]);

        let result = dict_extend(&[d1, d2]).expect("dict_extend should succeed");
        let result_dict = extract_dict_value(&result).expect("result should be a dict");

        // 期望：{"a": 1, "b": 3, "c": 4}，d2 的 "b" 覆盖 d1 的 "b"
        assert_eq!(result_dict.len(), 3, "merged dict should have 3 entries");

        let key_a = Value::from_string("a").string_index().unwrap();
        let key_b = Value::from_string("b").string_index().unwrap();
        let key_c = Value::from_string("c").string_index().unwrap();

        assert_eq!(
            result_dict.get(key_a),
            Some(Value::from_number(1.0)),
            "a should be 1 (from d1)"
        );
        assert_eq!(
            result_dict.get(key_b),
            Some(Value::from_number(3.0)),
            "b should be 3 (overridden by d2)"
        );
        assert_eq!(
            result_dict.get(key_c),
            Some(Value::from_number(4.0)),
            "c should be 4 (from d2)"
        );
    }

    #[test]
    fn test_dict_extend_cow_semantics() {
        let d1 = make_dict_value(&[("a", Value::from_number(1.0))]);
        let d2 = make_dict_value(&[("b", Value::from_number(2.0))]);

        let result = dict_extend(&[d1, d2]).expect("dict_extend should succeed");

        // 验证 result 是 {"a": 1, "b": 2}
        let result_dict = extract_dict_value(&result).expect("result should be a dict");
        assert_eq!(result_dict.len(), 2, "merged dict should have 2 entries");

        let key_a = Value::from_string("a").string_index().unwrap();
        let key_b = Value::from_string("b").string_index().unwrap();
        assert_eq!(result_dict.get(key_a), Some(Value::from_number(1.0)));
        assert_eq!(result_dict.get(key_b), Some(Value::from_number(2.0)));

        // COW：验证 d1 未被修改，仍只有 {"a": 1}
        let d1_dict = extract_dict_value(&d1).expect("d1 should still be a dict");
        assert_eq!(d1_dict.len(), 1, "d1 should not be modified (COW semantics)");
        assert_eq!(d1_dict.get(key_a), Some(Value::from_number(1.0)));
        assert_eq!(d1_dict.get(key_b), None, "d1 should not contain key from d2");

        // COW：验证 d2 未被修改，仍只有 {"b": 2}
        let d2_dict = extract_dict_value(&d2).expect("d2 should still be a dict");
        assert_eq!(d2_dict.len(), 1, "d2 should not be modified (COW semantics)");
        assert_eq!(d2_dict.get(key_b), Some(Value::from_number(2.0)));
        assert_eq!(d2_dict.get(key_a), None, "d2 should not contain key from d1");
    }

    #[test]
    fn test_dict_extend_type_mismatch() {
        let d = make_dict_value(&[("a", Value::from_number(1.0))]);
        let non_dict = Value::from_number(42.0);

        // extend(非dict, dict) 应返回错误
        let result1 = dict_extend(&[non_dict, d]);
        assert!(result1.is_err(), "extend(non-dict, dict) should return error");

        // extend(dict, 非dict) 应返回错误
        let result2 = dict_extend(&[d, non_dict]);
        assert!(result2.is_err(), "extend(dict, non-dict) should return error");
    }

    #[test]
    fn test_dict_extend_empty_dicts() {
        let empty = make_dict_value(&[]);
        let d = make_dict_value(&[("a", Value::from_number(1.0)), ("b", Value::from_number(2.0))]);

        // extend(empty, d) 应返回 d 的副本
        let result1 = dict_extend(&[empty, d]).expect("dict_extend should succeed");
        let r1_dict = extract_dict_value(&result1).expect("result should be a dict");
        assert_eq!(r1_dict.len(), 2, "extend(empty, d) should yield d's entries");

        // extend(d, empty) 应返回 d 的副本
        let result2 = dict_extend(&[d, empty]).expect("dict_extend should succeed");
        let r2_dict = extract_dict_value(&result2).expect("result should be a dict");
        assert_eq!(r2_dict.len(), 2, "extend(d, empty) should yield d's entries");

        // extend(empty, empty) 应返回空 dict
        let result3 = dict_extend(&[empty, empty]).expect("dict_extend should succeed");
        let r3_dict = extract_dict_value(&result3).expect("result should be a dict");
        assert_eq!(r3_dict.len(), 0, "extend(empty, empty) should yield empty dict");
    }
}
