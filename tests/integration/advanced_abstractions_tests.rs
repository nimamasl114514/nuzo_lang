//! advanced_abstractions_tests — nuzo_values 高级泛型基础设施集成测试
//!
//! 覆盖 4 类工具：HList / Functor+Monad+Applicative / AnyMap / GenericArray
//! 同时展示 4 类工具的真实使用场景。
//!
//! 参考：nuzo_values 泛型边界用例。

use nuzo_values::{
    AnyMap, Applicative, Functor, GenericArray, HCons, HList, HListPrepend, HNil, Monad, Value,
    hlist,
};

// ============================================================================
// HList 测试组（覆盖 checklist 第 2 节 HList 全部用例）
// ============================================================================

#[test]
fn test_hlist_empty() {
    let h = HNil;
    assert_eq!(h.len(), 0);
    assert!(h.is_empty());
    assert_eq!(HNil::LEN, 0);
}

#[test]
fn test_hlist_single() {
    let h = hlist!(1);
    assert_eq!(h.len(), 1);
    assert!(!h.is_empty());
    assert_eq!(h.head(), &1);
}

#[test]
fn test_hlist_long() {
    // 4 元素异构列表：i32 / &str / bool / f64
    let h = hlist!(1, "a", true, 2.0_f64);
    assert_eq!(h.len(), 4);
    assert!(!h.is_empty());
    assert_eq!(h.head(), &1);
    assert_eq!(h.tail().head(), &"a");
    assert_eq!(h.tail().tail().head(), &true);
    assert_eq!(h.tail().tail().tail().head(), &2.0_f64);
}

#[test]
fn test_hlist_prepend() {
    // HNil.prepend(1).prepend("a") → HCons<&str, HCons<i32, HNil>>
    // 注意：prepend 是在前端加元素，所以最后一次 prepend 出现在最外层 head
    let h = HNil.prepend(1).prepend("a");
    assert_eq!(h.len(), 2);
    assert_eq!(h.head(), &"a");
    assert_eq!(h.tail().head(), &1);
}

#[test]
fn test_hlist_head_tail() {
    let h = hlist!(1, "a", true);
    assert_eq!(h.head(), &1);
    assert_eq!(h.tail().head(), &"a");
    assert_eq!(h.tail().tail().head(), &true);
    // 尾部应是 HNil
    assert_eq!(h.tail().tail().tail().len(), 0);
}

#[test]
fn test_hlist_into_head_tail() {
    let h = hlist!(1, "a");
    let (head, tail) = h.into_head_tail();
    assert_eq!(head, 1);
    assert_eq!(tail.head(), &"a");
    assert_eq!(tail.tail().len(), 0);
}

#[test]
fn test_hlist_value_elem() {
    // 含 nuzo_values::Value 元素的 HList 能编译并读写
    // Value 是 NaN-tagged（u64 表示），是 Copy，因此 HCons<Value, _>: Copy 也成立
    let h: HCons<Value, HCons<Value, HNil>> = hlist!(nuzo_values::NIL, nuzo_values::TRUE,);
    assert_eq!(h.len(), 2);
    assert_eq!(*h.head(), nuzo_values::NIL);
    assert_eq!(*h.tail().head(), nuzo_values::TRUE);
}

#[test]
fn test_hlist_construct_via_hcons_new() {
    // 不依赖宏，直接用 HCons::new 构造
    let h = HCons::new(1, HCons::new("a", HCons::new(true, HNil)));
    assert_eq!(h.len(), 3);
    assert_eq!(h.head(), &1);
    assert_eq!(h.tail().head(), &"a");
    assert_eq!(h.tail().tail().head(), &true);
}

// ============================================================================
// AnyMap 测试组
// ============================================================================

#[test]
fn test_anymap_empty() {
    let m = AnyMap::new();
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
    assert!(!m.contains::<u32>());
}

#[test]
fn test_anymap_insert_get() {
    let mut m = AnyMap::new();
    assert!(m.insert(42u32).is_none());
    assert_eq!(m.get::<u32>(), Some(&42u32));
    assert_eq!(m.len(), 1);
}

#[test]
fn test_anymap_override() {
    let mut m = AnyMap::new();
    m.insert(1u32);
    // 同类型二次 insert 应返回旧值
    let old = m.insert(2u32);
    assert_eq!(old, Some(1u32));
    // 当前值是新值
    assert_eq!(m.get::<u32>(), Some(&2u32));
    assert_eq!(m.len(), 1);
}

#[test]
fn test_anymap_get_missing() {
    let m = AnyMap::new();
    assert_eq!(m.get::<u32>(), None);
    assert_eq!(m.get::<String>(), None);
}

#[test]
fn test_anymap_remove() {
    let mut m = AnyMap::new();
    m.insert(42u32);
    let removed = m.remove::<u32>();
    assert_eq!(removed, Some(42u32));
    assert_eq!(m.get::<u32>(), None);
    assert!(!m.contains::<u32>());
    assert_eq!(m.len(), 0);
    // 再次 remove 应返回 None
    assert_eq!(m.remove::<u32>(), None);
}

#[test]
fn test_anymap_contains() {
    let mut m = AnyMap::new();
    m.insert(42u32);
    assert!(m.contains::<u32>());
    assert!(!m.contains::<u64>());
    assert!(!m.contains::<String>());
}

#[test]
fn test_anymap_multi_type() {
    let mut m = AnyMap::new();
    m.insert(1u32);
    m.insert("hello".to_string());
    m.insert(vec![1u8, 2, 3]);
    assert_eq!(m.len(), 3);
    assert_eq!(m.get::<u32>(), Some(&1u32));
    assert_eq!(m.get::<String>(), Some(&"hello".to_string()));
    assert_eq!(m.get::<Vec<u8>>(), Some(&vec![1u8, 2, 3]));
    // 各类型互不干扰：移除一个不影响其他
    m.remove::<String>();
    assert_eq!(m.get::<u32>(), Some(&1u32));
    assert_eq!(m.get::<Vec<u8>>(), Some(&vec![1u8, 2, 3]));
    assert_eq!(m.len(), 2);
}

#[test]
fn test_anymap_clear() {
    let mut m = AnyMap::new();
    m.insert(1u32);
    m.insert("x".to_string());
    m.insert(vec![1u8]);
    assert_eq!(m.len(), 3);
    m.clear();
    assert_eq!(m.len(), 0);
    assert!(m.is_empty());
    assert!(!m.contains::<u32>());
}

#[test]
fn test_anymap_get_mut() {
    let mut m = AnyMap::new();
    m.insert(42u32);
    {
        let v = m.get_mut::<u32>().expect("u32 must exist");
        *v = 100;
    }
    assert_eq!(m.get::<u32>(), Some(&100u32));
    // 修改 String
    m.insert("hello".to_string());
    {
        let s = m.get_mut::<String>().expect("String must exist");
        s.push_str(" world");
    }
    assert_eq!(m.get::<String>(), Some(&"hello world".to_string()));
}

#[test]
fn test_anymap_default() {
    let m: AnyMap = AnyMap::default();
    assert!(m.is_empty());
}

// ============================================================================
// GenericArray 测试组
// ============================================================================

#[test]
fn test_garray_empty() {
    let arr: GenericArray<u32, 0> = GenericArray::new([]);
    assert_eq!(arr.len(), 0);
    assert!(arr.is_empty());
    assert!(arr.as_slice().is_empty());
}

#[test]
fn test_garray_single() {
    let arr: GenericArray<u32, 1> = GenericArray::new([7]);
    assert_eq!(arr.len(), 1);
    assert!(!arr.is_empty());
    assert_eq!(arr[0], 7);
    assert_eq!(arr.as_slice(), &[7]);
}

#[test]
fn test_garray_len() {
    let arr: GenericArray<u32, 8> = GenericArray::new([1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(arr.len(), 8);
    assert!(!arr.is_empty());
}

#[test]
fn test_garray_index() {
    let arr: GenericArray<u32, 4> = GenericArray::new([10, 20, 30, 40]);
    assert_eq!(arr[0], 10);
    assert_eq!(arr[3], 40);
    assert_eq!(arr.as_slice()[2], 30);
}

#[test]
fn test_garray_index_mut() {
    let mut arr: GenericArray<u32, 4> = GenericArray::new([10, 20, 30, 40]);
    arr[3] = 99;
    assert_eq!(arr[3], 99);
    arr[0] = arr[1] + 5;
    assert_eq!(arr[0], 25);
    // as_mut_slice 写入
    let slice = arr.as_mut_slice();
    slice[2] = 333;
    assert_eq!(arr[2], 333);
}

#[test]
fn test_garray_map() {
    let arr: GenericArray<i32, 3> = GenericArray::new([1, 2, 3]);
    let doubled: GenericArray<i32, 3> = arr.map(|x| x * 2);
    assert_eq!(doubled.as_slice(), &[2, 4, 6]);
    // 类型转换：i32 → String
    let arr: GenericArray<i32, 3> = GenericArray::new([1, 2, 3]);
    let strs: GenericArray<String, 3> = arr.map(|x| x.to_string());
    assert_eq!(strs.as_slice(), &["1".to_string(), "2".to_string(), "3".to_string()]);
}

#[test]
fn test_garray_from_fn() {
    let arr: GenericArray<u32, 4> = GenericArray::from_fn(|i| i as u32);
    assert_eq!(arr.as_slice(), &[0, 1, 2, 3]);
    // 用 i 计算平方
    let squares: GenericArray<u32, 5> = GenericArray::from_fn(|i| (i * i) as u32);
    assert_eq!(squares.as_slice(), &[0, 1, 4, 9, 16]);
}

#[test]
fn test_garray_into_iter() {
    // for x in &arr —— 借用迭代
    let arr: GenericArray<u32, 4> = GenericArray::new([1, 2, 3, 4]);
    let mut sum = 0u32;
    for x in &arr {
        sum += *x;
    }
    assert_eq!(sum, 10);
    // arr 仍然可用（借用）
    assert_eq!(arr.len(), 4);
}

#[test]
fn test_garray_into_iter_owned() {
    // into_iter() 消费数组
    let arr: GenericArray<u32, 4> = GenericArray::new([1, 2, 3, 4]);
    let collected: Vec<u32> = arr.into_iter().collect();
    assert_eq!(collected, vec![1, 2, 3, 4]);
}

#[test]
fn test_garray_default() {
    let arr: GenericArray<u32, 3> = GenericArray::default();
    assert_eq!(arr.as_slice(), &[0, 0, 0]);
    let arr: GenericArray<String, 2> = GenericArray::default();
    assert_eq!(arr.as_slice(), &["".to_string(), "".to_string()]);
}

#[test]
fn test_garray_into_inner() {
    let arr: GenericArray<u32, 3> = GenericArray::new([1, 2, 3]);
    let inner: [u32; 3] = arr.into_inner();
    assert_eq!(inner, [1, 2, 3]);
}

#[test]
fn test_garray_clone_copy() {
    let arr: GenericArray<u32, 3> = GenericArray::new([1, 2, 3]);
    // Copy 语义：赋值不移动
    let arr2 = arr;
    let arr3 = arr;
    assert_eq!(arr.as_slice(), &[1, 2, 3]);
    assert_eq!(arr2.as_slice(), &[1, 2, 3]);
    assert_eq!(arr3.as_slice(), &[1, 2, 3]);
    // PartialEq
    assert_eq!(arr, arr2);
}

// ============================================================================
// Functor / Applicative / Monad 测试组
// ============================================================================

// ---- Functor ----

#[test]
fn test_functor_option_some() {
    let x: Option<i32> = Some(1);
    let r = Functor::map(x, |v| v + 1);
    assert_eq!(r, Some(2));
}

#[test]
fn test_functor_option_none() {
    let x: Option<i32> = None;
    let r = Functor::map(x, |v| v + 1);
    assert_eq!(r, None);
}

#[test]
fn test_functor_vec() {
    let v = vec![1, 2, 3];
    let r: Vec<i32> = Functor::map(v, |x| x * 2);
    assert_eq!(r, vec![2, 4, 6]);
}

#[test]
fn test_functor_vec_empty() {
    let v: Vec<i32> = Vec::new();
    let r: Vec<i32> = Functor::map(v, |x| x * 2);
    assert!(r.is_empty());
}

#[test]
fn test_functor_result_ok() {
    let r: Result<i32, &str> = Ok(1);
    let mapped: Result<i32, &str> = Functor::map(r, |x| x + 1);
    assert_eq!(mapped, Ok(2));
}

#[test]
fn test_functor_result_err() {
    let r: Result<i32, &str> = Err("e");
    let mapped: Result<i32, &str> = Functor::map(r, |x| x + 1);
    assert_eq!(mapped, Err("e"));
}

// ---- Applicative ----

#[test]
fn test_applicative_pure() {
    let x: Option<i32> = Applicative::pure(42);
    assert_eq!(x, Some(42));
    let v: Vec<i32> = Applicative::pure(7);
    assert_eq!(v, vec![7]);
    let r: Result<i32, &str> = Applicative::pure(7);
    assert_eq!(r, Ok(7));
}

#[test]
fn test_applicative_apply() {
    // Some(1).apply(Some(|x| x+1)) == Some(2)
    let r: Option<i32> = Applicative::apply(Some(1), Some(|x: i32| x + 1));
    assert_eq!(r, Some(2));
    // None 应用 Some 函数 → None
    let r: Option<i32> = Applicative::apply(None::<i32>, Some(|x: i32| x + 1));
    assert_eq!(r, None);
    // Some 应用 None 函数 → None
    let r: Option<i32> = Applicative::apply(Some(1), None::<fn(i32) -> i32>);
    assert_eq!(r, None);
}

#[test]
fn test_applicative_vec_apply() {
    // Vec 笛卡尔积语义：[1,2] apply [(+10), (+100)] → [11, 101, 12, 102]
    let xs = vec![1, 2];
    let fs: Vec<Box<dyn FnMut(i32) -> i32>> = vec![Box::new(|x| x + 10), Box::new(|x| x + 100)];
    let r: Vec<i32> = Applicative::apply(xs, fs);
    assert_eq!(r, vec![11, 101, 12, 102]);
}

#[test]
fn test_applicative_result_apply() {
    let r: Result<i32, &str> = Applicative::apply(Ok(1), Ok(|x: i32| x + 1));
    assert_eq!(r, Ok(2));
    let r: Result<i32, &str> = Applicative::apply(Err::<i32, &str>("e"), Ok(|x: i32| x + 1));
    assert_eq!(r, Err("e"));
}

// ---- Monad ----

#[test]
fn test_monad_bind_option() {
    let r: Option<i32> = Monad::bind(Some(1), |x| Some(x * 2));
    assert_eq!(r, Some(2));
}

#[test]
fn test_monad_bind_none() {
    let r: Option<i32> = Monad::bind(None::<i32>, |x| Some(x * 2));
    assert_eq!(r, None);
    // bind 返回 None
    let r: Option<i32> = Monad::bind(Some(1), |x| if x > 0 { None } else { Some(x) });
    assert_eq!(r, None);
}

#[test]
fn test_monad_chain() {
    // Some(1).bind(...).bind(...) 链式
    let r: Option<i32> = Monad::bind(Monad::bind(Some(1), |x| Some(x + 1)), |x| Some(x * 10));
    assert_eq!(r, Some(20));
    // 链中任一环节返回 None 都会短路
    let r: Option<i32> =
        Monad::bind(Monad::bind(Some(1i32), |x| if x > 0 { None } else { Some(x) }), |x| {
            Some(x * 10)
        });
    assert_eq!(r, None);
}

#[test]
fn test_monad_result_bind() {
    let r: Result<i32, &str> = Monad::bind(Ok(1), |x| if x > 0 { Ok(x + 1) } else { Err("neg") });
    assert_eq!(r, Ok(2));
    let r: Result<i32, &str> =
        Monad::bind(Ok(-1i32), |x| if x > 0 { Ok(x + 1) } else { Err("neg") });
    assert_eq!(r, Err("neg"));
    // 错误短路
    let r: Result<i32, &str> =
        Monad::bind(Err::<i32, &str>("init"), |x| if x > 0 { Ok(x + 1) } else { Err("neg") });
    assert_eq!(r, Err("init"));
}

#[test]
fn test_monad_vec_bind() {
    // flatMap 语义：[1,2,3].bind(x => [x, x*10]) == [1,10,2,20,3,30]
    let v = vec![1, 2, 3];
    let r: Vec<i32> = Monad::bind(v, |x| vec![x, x * 10]);
    assert_eq!(r, vec![1, 10, 2, 20, 3, 30]);
    // 空 Vec
    let v: Vec<i32> = Vec::new();
    let r: Vec<i32> = Monad::bind(v, |x| vec![x, x * 10]);
    assert!(r.is_empty());
}

// ============================================================================
// 真实使用场景示例
// ============================================================================

/// 场景 1：AnyMap 模拟编译器 pass 间元数据传递。
///
/// 编译器各 pass（parser / resolver / typechecker / optimizer / codegen）
/// 产出不同类型的元数据，下游 pass 按类型按需取用。AnyMap 提供类型安全
/// 的"按 TypeId 索引"语义，避免到处写 trait object 或 enum 包装。
#[test]
fn test_realworld_anymap_metadata_store() {
    let mut metadata = AnyMap::new();

    // pass 1：parser 收集 warning 列表
    metadata.insert(vec!["unused variable `x`".to_string(), "shadowed `y`".to_string()]);
    // pass 2：optimizer 累计优化次数
    metadata.insert(42u32);
    // pass 3：peephole 写入二进制记录（u8 序列）
    metadata.insert(vec![0u8, 1, 2, 3, 4]);

    // 下游 pass 按类型取用
    let warnings: &Vec<String> = metadata.get::<Vec<String>>().expect("warnings must exist");
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].contains("unused"));

    let opt_count: &u32 = metadata.get::<u32>().expect("opt count must exist");
    assert_eq!(*opt_count, 42);

    let peephole: &Vec<u8> = metadata.get::<Vec<u8>>().expect("peephole records must exist");
    assert_eq!(peephole.len(), 5);

    // codegen pass 增量更新优化次数
    if let Some(count) = metadata.get_mut::<u32>() {
        *count += 10;
    }
    assert_eq!(metadata.get::<u32>(), Some(&52));

    assert_eq!(metadata.len(), 3);
}

/// 场景 2：HList 模拟 codegen 多类型参数列表。
///
/// 参考 `nuzo_compiler/src/codegen.rs:81` 的 context 标签场景：
/// codegen 阶段需要传递 (函数名, 行号, 优化标志, 调试注释) 4 种异构参数，
/// 用 HList 而非 tuple 的好处是 head/tail 可被链式解构、长度编译期已知、
/// 且扩展为 5/6 个参数时不破坏现有 head/tail 调用模式。
#[test]
fn test_realworld_hlist_heterogeneous_params() {
    // 模拟 codegen 上下文：(函数名, 行号, 优化标志, 调试注释)
    let ctx = hlist!("main", 42u32, true, "entry point");

    // 长度编译期已知
    assert_eq!(ctx.len(), 4);

    // head/tail 逐个取出，类型安全
    let fn_name: &&str = ctx.head();
    assert_eq!(*fn_name, "main");

    let line_no: &u32 = ctx.tail().head();
    assert_eq!(*line_no, 42);

    let opt_flag: &bool = ctx.tail().tail().head();
    assert!(*opt_flag);

    let debug_comment: &&str = ctx.tail().tail().tail().head();
    assert_eq!(*debug_comment, "entry point");

    // 尾部应是 HNil
    assert_eq!(ctx.tail().tail().tail().tail().len(), 0);

    // 拆解为 (head, tail) 用于"消费"上下文
    let (name, rest) = ctx.into_head_tail();
    assert_eq!(name, "main");
    assert_eq!(rest.len(), 3);
}

/// 场景 3：GenericArray 模拟 VM 定长寄存器窗口。
///
/// 参考 `nuzo_vm/src/elastic_register_file.rs`：VM 在调用约定中常需要
/// "N 个寄存器组成的定长窗口"。GenericArray<T, N> 用 const generics 表达
/// 定长，比 Vec 更省一次堆分配、比 [T; N] 更易实现 map/from_fn。
#[test]
fn test_realworld_garray_fixed_register_window() {
    // 8 寄存器窗口，初始全 0
    let mut window: GenericArray<u64, 8> = GenericArray::from_fn(|_| 0u64);
    assert_eq!(window.len(), 8);

    // 写入：模拟指令 mov r1, 0xABCD
    window[1] = 0xABCD_u64;
    // 写入：mov r2, r1 + 1
    window[2] = window[1] + 1;
    assert_eq!(window[1], 0xABCD);
    assert_eq!(window[2], 0xABCE);

    // 读取：验证未写入的寄存器仍为 0
    for i in (3..8).chain(0..1) {
        assert_eq!(window[i], 0);
    }

    // map 转换：所有寄存器值翻倍（模拟某种 pass）
    let doubled: GenericArray<u64, 8> = window.map(|v| v * 2);
    assert_eq!(doubled[1], 0xABCD * 2);
    assert_eq!(doubled[2], 0xABCE * 2);

    // 遍历求和（借用迭代）
    let total: u64 = (&window).into_iter().sum();
    assert_eq!(total, 0xABCD + 0xABCE);
}

/// 场景 4：Monad 链式模拟编译器多 pass 流水线。
///
/// 模拟 parse → resolve → typecheck → codegen 四个 pass，每个 pass 可能失败。
/// 用 `Result<T, &str>` 表达，`Monad::bind` 把每步串起来，与传统 `?` 操作符
/// 等价但显式展示"短路"语义。
#[test]
fn test_realworld_monad_result_pipeline() {
    // 用 Monad::bind 串联多 pass
    let pipeline: Result<String, &str> = Monad::bind(
        Monad::bind(
            Monad::bind(
                Ok::<&str, &str>("let x = 1;"), // pass 1: parse → AST
                |_ast| Ok("AST".to_string()),   // pass 2: resolve
            ),
            |_resolved| Ok("Typed".to_string()), // pass 3: typecheck
        ),
        |_typed| Ok("Bytecode".to_string()), // pass 4: codegen
    );
    assert_eq!(pipeline, Ok("Bytecode".to_string()));

    // 任意一步失败都短路：typecheck 失败
    let failing: Result<String, &str> = Monad::bind(
        Monad::bind(
            Monad::bind(Ok::<&str, &str>("let x = 1;"), |_ast| Ok("AST".to_string())),
            |_resolved| Err::<String, &str>("type error: undefined symbol"),
        ),
        |_typed| Ok("Bytecode".to_string()),
    );
    assert_eq!(failing, Err("type error: undefined symbol"));

    // 与传统 `?` 操作符的等价性验证
    fn pipeline_with_qmark(input: &str) -> Result<String, &'static str> {
        let ast = if input.is_empty() {
            return Err("empty input");
        } else {
            "AST"
        };
        let resolved = if ast == "AST" {
            "Resolved"
        } else {
            return Err("resolve failed");
        };
        let typed = if resolved == "Resolved" {
            "Typed"
        } else {
            return Err("typecheck failed");
        };
        Ok(format!("{} → Bytecode", typed))
    }

    // 同样输入下两种写法都成功
    assert!(pipeline_with_qmark("let x = 1;").is_ok());
    assert_eq!(pipeline_with_qmark(""), Err("empty input"));
}
