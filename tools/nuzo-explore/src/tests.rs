//! Nuzo 探索性测试 — 系统性地测试各类语法特性
//! 验证编译+执行是否成功（不挂起、不崩溃）

use nuzo_compiler::Compiler;
use nuzo_vm::VM;

pub struct TestResult {
    pub name: &'static str,
    pub category: &'static str,
    pub passed: bool,
    pub detail: String,
}

/// 编译并执行 Nuzo 源码，返回 Ok(()) 或 Err(错误信息)
fn run_nuzo(source: &str) -> Result<(), String> {
    let chunk = Compiler::compile(source).map_err(|e| format!("编译失败: {}", e))?;
    let mut vm = VM::new();
    vm.run(chunk).map_err(|e| format!("执行失败: {}", e))?;
    Ok(())
}

pub fn run_all() {
    println!("正在运行探索性测试...\n");
    let mut results: Vec<TestResult> = Vec::new();

    macro_rules! t {
        ($name:expr, $cat:expr, $src:expr) => {
            let r = run_nuzo($src);
            results.push(TestResult {
                name: $name,
                category: $cat,
                passed: r.is_ok(),
                detail: format!("{:?}", r),
            });
        };
    }

    // ========== 1. 字符串与拼接 ==========
    t!("字符串拼接(3段)", "字符串", r#"println("hello" + " " + "world")"#);
    t!("字符串拼接(变量+字面量)", "字符串", r#"s = "abc"; println(s + "def" + s)"#);
    t!("空字符串拼接", "字符串", r#"println("" + "" + "")"#);
    t!("字符串+数字拼接", "字符串", r#"println("abc" + 123)"#);
    t!(
        "字符串拼接(8段)",
        "字符串",
        r#"s = ""; s = s + "a" + "b" + "c" + "d" + "e" + "f" + "g" + "h"; println(s)"#
    );

    // ========== 2. 递归与尾递归 ==========
    t!(
        "递归斐波那契(fib(10)=55)",
        "递归",
        r#"fn fib(n) { if n <= 1 { return n } return fib(n - 1) + fib(n - 2) } println(fib(10))"#
    );
    t!(
        "尾递归阶乘(20!)",
        "递归",
        r#"fn fact(n, acc) { if n <= 1 { return acc } return fact(n - 1, acc * n) } println(fact(20, 1))"#
    );
    t!(
        "尾递归深度(5000层)",
        "递归",
        r#"fn countdown(n, acc) { if n <= 0 { return acc } return countdown(n - 1, acc + 1) } println(countdown(5000, 0))"#
    );
    t!(
        "尾递归深度(20000层)",
        "递归",
        r#"fn deep(n, acc) { if n <= 0 { return acc } return deep(n - 1, n + acc) } println(deep(20000, 0))"#
    );

    // ========== 3. 数组操作 ==========
    t!(
        "数组基本操作",
        "数组",
        r#"arr = [1, 2, 3, 4, 5]; println(len(arr)); println(arr[0]); println(arr[4]); push(arr, 6); println(len(arr)); println(pop(arr))"#
    );
    t!(
        "数组填充(1000元素)",
        "数组",
        r#"arr = []; i = 0; while i < 1000 { push(arr, i); i = i + 1 } println(len(arr)); println(arr[0]); println(arr[999])"#
    );
    t!(
        "嵌套数组访问",
        "数组",
        r#"arr = [1, [2, 3], [4, [5, 6]]]; println(arr[0]); println(arr[1][0]); println(arr[2][1][0])"#
    );

    // ========== 4. 字典操作 ==========
    t!(
        "字典基本操作",
        "字典",
        r#"d = {name: "Nuzo", version: 1}; println(d["name"]); println(d["version"]); d["version"] = 2; println(d["version"]); println(len(d))"#
    );
    t!(
        "字典填充(100键)",
        "字典",
        r#"d = {}; i = 0; while i < 100 { d[i] = i * i; i = i + 1 } println(d[0]); println(d[99]); println(len(d))"#
    );

    // ========== 5. 闭包与高阶函数 ==========
    t!(
        "闭包-加法器工厂",
        "闭包",
        r#"fn make_adder(x) { return fn(y) { x + y } } add5 = make_adder(5); println(add5(10)); println(add5(100))"#
    );
    t!(
        "嵌套闭包",
        "闭包",
        r#"fn outer(x) { fn inner(y) { return x * y } return inner } f = outer(10); println(f(5))"#
    );
    t!("Lambda箭头函数", "闭包", r#"double = x => x * 2; println(double(7))"#);
    t!(
        "闭包-计数器(可变状态)",
        "闭包",
        r#"fn make_counter() { count = 0; return fn() { count = count + 1; return count } } counter = make_counter(); println(counter()); println(counter()); println(counter())"#
    );

    // ========== 6. 控制流 ==========
    t!(
        "while循环累加(0..99)",
        "控制流",
        r#"x = 0; i = 0; while i < 100 { x = x + i; i = i + 1 } println(x)"#
    );
    t!(
        "for-in遍历数组",
        "控制流",
        r#"sum = 0; for i in [1, 2, 3, 4, 5] { sum = sum + i } println(sum)"#
    );
    t!(
        "loop+break",
        "控制流",
        r#"sum = 0; i = 0; loop { if i >= 50 { break }; sum = sum + i; i = i + 1 } println(sum)"#
    );
    t!(
        "if/else表达式返回值",
        "控制流",
        r#"x = 42; result = if x > 10 { "big" } else { "small" }; println(result)"#
    );
    t!(
        "多级if-return(成绩等级)",
        "控制流",
        r#"fn grade(score) { if score >= 90 { return "A" } if score >= 80 { return "B" } if score >= 70 { return "C" } if score >= 60 { return "D" } return "F" } println(grade(95)); println(grade(85)); println(grade(55))"#
    );

    // ========== 7. 异常处理 ==========
    t!(
        "try-catch除零",
        "异常",
        r#"try { x = 1 / 0; println("never") } catch (e) { println("caught") }"#
    );
    t!(
        "try-catch-keep",
        "异常",
        r##"try { println("try") } catch (e) { println("catch") } keep { println("keep") }"##
    );
    t!(
        "手动out+catch",
        "异常",
        r#"fn risky(x) { if x < 0 { out "negative" } return x * 2 } try { println(risky(-5)) } catch (e) { println("error") } println(risky(10))"#
    );

    // ========== 8. 复杂表达式 ==========
    t!("幂运算结合性(2**3**2)", "表达式", r#"x = 2 ** 3 ** 2; println(x)"#);
    t!("取模运算(含负数)", "表达式", r#"println(10 % 3); println(10 mod 3); println(-10 % 3)"#);
    t!(
        "布尔运算",
        "表达式",
        r#"a = true && false; b = true || false; c = !true; println(a); println(b); println(c)"#
    );
    t!("链式比较(1<2<3)", "表达式", r#"println(1 < 2 < 3)"#);

    // ========== 9. 浮点数 ==========
    t!("浮点运算", "数值", r#"println(0.1 + 0.2); println(1.5 * 2.0); println(10.0 / 3.0)"#);
    t!("大整数乘法", "数值", r#"println(9999999999 * 9999999999)"#);

    // ========== 10. 内置函数 ==========
    t!(
        "typeof类型检测",
        "内置函数",
        r#"println(typeof(42)); println(typeof("hello")); println(typeof(true)); println(typeof(nil)); println(typeof([1,2])); println(typeof({a:1}))"#
    );
    t!(
        "assert断言",
        "内置函数",
        r#"assert(true); println("pass1"); assert(1 + 1 == 2); println("pass2"); assert(2 * 3 == 6); println("pass3")"#
    );

    // ========== 11. 中英文关键字 ==========
    t!(
        "中文关键字(如果/否则/真/打印)",
        "国际化",
        r#"如果 真 { 打印("中文") } 否则 { 打印("english") }"#
    );
    t!("中文关键字(函数/返回/打印)", "国际化", r#"函数 加(a, b) { 返回 a + b } 打印(加(3, 7))"#);

    // ========== 12. 边界/压力场景 ==========
    t!(
        "大量局部变量(20个)",
        "边界",
        r#"a=1; b=2; c=3; d=4; e=5; f=6; g=7; h=8; i=9; j=10; k=11; l=12; m=13; n=14; o=15; p=16; q=17; r=18; s=19; t=20; println(a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t)"#
    );
    t!("复合赋值(+= *= -= /=)", "边界", r#"x = 10; x += 5; x *= 2; x -= 3; x /= 3; println(x)"#);
    t!(
        "嵌套函数中递归",
        "边界",
        r#"fn outer(x) { fn inner(y) { if y <= 1 { return y } return inner(y - 1) + inner(y - 2) } return inner(x) } println(outer(10))"#
    );
    t!(
        "匿名函数递归(变量自引用)",
        "边界",
        r#"fib = fn(n) { if n <= 1 { return n } return fib(n - 1) + fib(n - 2) } println(fib(10))"#
    );
    t!("空函数体", "边界", r#"fn empty() {}; println(empty())"#);
    t!("无参函数", "边界", r#"fn meaning() { return 42 }; println(meaning())"#);

    // ========== 13. 错误路径测试 ==========
    t!("未定义变量(应报错)", "错误路径", r#"println(undefined_var)"#);
    t!("除零(运行时错误)", "错误路径", r#"println(1/0)"#);
    t!("空数组索引(越界)", "错误路径", r#"arr = []; println(arr[0])"#);
    t!("数字+字符串类型错误", "错误路径", r##"1 + "hello""##);
    t!("未闭合字符串(编译错误)", "错误路径", r#"println("unclosed string)"#);

    // ========== 14. 互递归 ==========
    t!(
        "互递归(even/odd)",
        "递归",
        r#"fn is_even(n) { if n == 0 { return true } return is_odd(n - 1) } fn is_odd(n) { if n == 0 { return false } return is_even(n - 1) } println(is_even(10)); println(is_odd(10)); println(is_even(11))"#
    );

    // ========== 打印结果 ==========
    println!("{:=^70}", " 测试结果汇总 ");
    println!("{:>4} | {:^40} | {:^8} | {:^10}", "#", "测试名称", "类别", "结果");
    println!("{:-<70}", "");

    let mut pass_count = 0;
    let mut fail_count = 0;
    for (i, r) in results.iter().enumerate() {
        let status = if r.passed { "✅ PASS" } else { "❌ FAIL" };
        println!("{:>4} | {:40} | {:^8} | {}", i + 1, r.name, r.category, status);
        if r.passed {
            pass_count += 1;
        } else {
            fail_count += 1;
        }
    }
    println!("{:-<70}", "");
    println!(" 总计: {} 通过, {} 失败, {} 总计\n", pass_count, fail_count, results.len());

    // 打印失败详情
    if fail_count > 0 {
        println!("{:=^70}", " 失败详情 ");
        for r in &results {
            if !r.passed {
                println!("  ❌ {}: {}", r.name, r.detail);
            }
        }
        println!();
    }
}
