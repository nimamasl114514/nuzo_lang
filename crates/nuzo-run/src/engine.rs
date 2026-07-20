//! Engine — long-lived runtime engine (Arc-shared, immutable after build).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use nuzo_bytecode::Chunk;
use nuzo_compiler::Compiler;
use nuzo_config::Config;
use nuzo_core::Value;
use nuzo_helpers::BuiltinRegistry;
use nuzo_ir::module_resolver::{ModuleResolver, ResolveError, StandardResolver};
use nuzo_signal::SignalBus;
use nuzo_vm::VM;
use nuzo_vm::gc::Gc;
use nuzo_vm::tracer_state::TraceConfig;

use crate::bench::BenchHarness;
use crate::config;
use crate::error::NuzoResult;
use crate::output::{Output, OutputSink};
use crate::plugin::NuzoPlugin;
use crate::session::Session;
use crate::test_harness::TestHarness;

/// Nuzo 语言的长生命周期执行引擎。
///
/// `Engine` 持有全局配置、插件列表和信号总线，是线程安全且可共享的。
/// 它不直接执行代码，而是通过 [`Engine::new_session`] 创建临时的 [`Session`]
/// 来运行脚本。一个 `Engine` 实例可以创建任意多个相互隔离的 `Session`。
pub struct Engine {
    inner: Arc<EngineInner>,
}

pub(crate) struct EngineInner {
    pub(crate) config: Config,
    plugins: Vec<Box<dyn NuzoPlugin>>,
    tracer_config: Option<TraceConfig>,
    pub(crate) bus: Arc<SignalBus>,
    /// 模块编译缓存：规范化绝对路径 → 已编译的字节码 [`Chunk`]。
    ///
    /// 用于 `import` 语句的依赖去重：同一模块被多次 import 时只编译一次，
    /// 后续 import 直接复用缓存的 Chunk。
    ///
    /// 使用 `RwLock` 允许多个 Session 并发读取；`Arc<Chunk>` 使返回值克隆开销极低。
    /// 因为 `EngineInner` 持有此字段，所以 `Session`（持有 `Arc<EngineInner>`）
    /// 可直接通过 `self.engine.module_cache` 访问。
    pub(crate) module_cache: RwLock<HashMap<PathBuf, Arc<Chunk>>>,
    /// 外部 builtin 注册表（如 GUI 库注入的 builtin 函数）。
    ///
    /// 通过 [`Engine::with_registry`] 设置，在创建每个新 Session 时
    /// 自动注册到 VM 的全局作用域。
    extra_builtins: Option<BuiltinRegistry>,
    /// 模块解析器：抽象 import 路径解析与源码加载。
    ///
    /// 默认为 [`StandardResolver`]（基于文件系统，原生行为）。
    /// wasm32 等无文件系统场景应通过 [`EngineBuilder::with_resolver`] 或
    /// [`Engine::with_resolver`] 注入 [`MemoryResolver`](nuzo_ir::module_resolver::MemoryResolver)
    /// 等无 fs 实现。
    ///
    /// 通过 `Arc<dyn ModuleResolver>` 持有，`impl ModuleResolver for EngineInner`
    /// 委托到此字段，保持向后兼容现有 `engine.inner.resolve(...)` 调用点。
    resolver: Arc<dyn ModuleResolver>,
}

/// `EngineBuilder` 状态标记：尚未提供配置。
///
/// 处于此状态时只能调用 `with_default_config`、`with_config` 等配置方法。
pub struct WantsConfig;
/// `EngineBuilder` 状态标记：配置已就绪，可以添加插件或构建。
pub struct Ready;

/// `Engine` 的构建器，使用类型状态模式保证编译期构建安全。
///
/// 构建流程：
/// 1. `Engine::builder()` → `WantsConfig`
/// 2. 调用 `with_default_config()` / `with_config(...)` / `with_config_file(...)` / `with_env_config()` → `Ready`
/// 3. （可选）调用 `trace()`、`plugin(...)`
/// 4. 调用 `build()` 得到 [`Engine`]
pub struct EngineBuilder<State = WantsConfig> {
    config: Option<Config>,
    tracer_config: Option<TraceConfig>,
    plugins: Vec<Box<dyn NuzoPlugin>>,
    resolver: Option<Arc<dyn ModuleResolver>>,
    _state: std::marker::PhantomData<State>,
}

impl EngineBuilder<WantsConfig> {
    pub(crate) fn new() -> Self {
        Self {
            config: None,
            tracer_config: None,
            plugins: Vec::new(),
            resolver: None,
            _state: std::marker::PhantomData,
        }
    }

    /// 使用默认配置继续构建。
    pub fn with_default_config(self) -> EngineBuilder<Ready> {
        EngineBuilder {
            config: Some(Config::default()),
            tracer_config: self.tracer_config,
            plugins: self.plugins,
            resolver: self.resolver,
            _state: std::marker::PhantomData,
        }
    }

    /// 使用提供的 [`Config`] 继续构建。
    pub fn with_config(self, config: Config) -> EngineBuilder<Ready> {
        EngineBuilder {
            config: Some(config),
            tracer_config: self.tracer_config,
            plugins: self.plugins,
            resolver: self.resolver,
            _state: std::marker::PhantomData,
        }
    }

    /// 从指定路径加载 TOML 配置文件并继续构建。
    ///
    /// # 错误
    /// 文件读取或解析失败时返回 [`NuzoError`]。
    pub fn with_config_file(self, path: impl AsRef<Path>) -> NuzoResult<EngineBuilder<Ready>> {
        let cfg = config::load_config_file(path)?;
        Ok(self.with_config(cfg))
    }

    /// 从环境变量加载配置并继续构建。
    ///
    /// # 错误
    /// 环境变量解析失败时返回 [`NuzoError`]。
    pub fn with_env_config(self) -> NuzoResult<EngineBuilder<Ready>> {
        let cfg = config::load_env_config()?;
        Ok(self.with_config(cfg))
    }
}

impl EngineBuilder<Ready> {
    /// 启用默认的指令级执行追踪器。
    pub fn trace(mut self) -> Self {
        self.tracer_config = Some(TraceConfig::default());
        self
    }

    /// 设置标准库搜索路径。当 `import` 路径以 `std/` 开头时，
    /// 模块解析器将以此路径为基准查找 `.nuzo` 文件。
    pub fn with_std_path(mut self, std_path: PathBuf) -> Self {
        if let Some(ref mut cfg) = self.config {
            cfg.std_path = Some(std_path);
        }
        self
    }

    /// 启用执行追踪器，并捕获最近 `register_window` 条寄存器状态。
    pub fn trace_registers(mut self, register_window: usize) -> Self {
        self.tracer_config = Some(TraceConfig {
            capture_registers: true,
            register_window: Some(register_window),
            ..TraceConfig::default()
        });
        self
    }

    pub fn plugin(mut self, plugin: impl NuzoPlugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// 注入自定义模块解析器。
    ///
    /// 默认使用 [`StandardResolver`]（基于文件系统，以 `config.std_path` 为基准）。
    /// wasm32 等无文件系统场景应注入 [`MemoryResolver`](nuzo_ir::module_resolver::MemoryResolver)
    /// 等无 fs 实现。
    ///
    /// 若未调用本方法，[`build`](Self::build) 时会以 `config.std_path` 构造默认
    /// [`StandardResolver`]，与历史行为一致。
    pub fn with_resolver(mut self, resolver: Arc<dyn ModuleResolver>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// 构建 [`Engine`] 实例。
    ///
    /// 触发所有已注册插件的 `on_start` 回调，并初始化引擎级信号总线。
    pub fn build(mut self) -> NuzoResult<Engine> {
        nuzo_values::register_value_hooks();
        let config = self.config.take().unwrap_or_default();
        // 默认解析器：StandardResolver + config.std_path（与历史行为一致）。
        // 用户可通过 `with_resolver` 注入 MemoryResolver 等无 fs 实现以支持 wasm32。
        let resolver = self
            .resolver
            .take()
            .unwrap_or_else(|| Arc::new(StandardResolver::new(config.std_path.clone())));
        let mut plugins = std::mem::take(&mut self.plugins);

        let bus = Arc::new(nuzo_signal::SignalBus::scoped(nuzo_signal::BusScope::Custom("engine")));
        for plugin in &plugins {
            plugin.register_signals(&bus);
        }
        for plugin in &mut plugins {
            plugin.on_start();
        }

        #[allow(clippy::arc_with_non_send_sync)]
        // EngineInner 含 Plugin trait object（非 Send/Sync），单线程使用安全
        let inner = Arc::new(EngineInner {
            config,
            plugins,
            tracer_config: self.tracer_config,
            bus,
            module_cache: RwLock::new(HashMap::new()),
            extra_builtins: None,
            resolver,
        });

        Ok(Engine { inner })
    }
}

impl Engine {
    /// 创建一个新的 [`EngineBuilder`]，进入 `WantsConfig` 状态。
    pub fn builder() -> EngineBuilder<WantsConfig> {
        EngineBuilder::new()
    }

    /// 使用默认配置快速创建一个 [`Engine`]。
    ///
    /// 等价于 `Engine::builder().with_default_config().build()`。
    pub fn quick() -> NuzoResult<Self> {
        Self::builder().with_default_config().build()
    }

    /// 获取 Engine 持有的信号总线引用
    ///
    /// 外部代码可通过此方法订阅 Engine 级别的信号，
    /// 或将 bus 注入到其他组件（如 VM 的 GC）。
    pub fn bus(&self) -> &Arc<SignalBus> {
        &self.inner.bus
    }

    /// 创建一个使用默认标准输出捕获的新 [`Session`]。
    pub fn new_session(&self) -> Session {
        self.new_session_with(OutputSink::Stdout)
    }

    /// 创建一个指定输出目标的新 [`Session`]。
    ///
    /// `sink` 决定脚本中 `println` 等输出被捕获、忽略还是直接打印到 stdout。
    pub fn new_session_with(&self, sink: OutputSink) -> Session {
        let bus = Arc::clone(&self.inner.bus);
        let gc = Gc::with_default_threshold().with_bus(bus);
        let (mut vm, trace_buf) = if let Some(ref tc) = self.inner.tracer_config {
            let (v, buf) =
                VM::init_gc_with_config_and_tracer(gc, self.inner.config.clone(), tc.clone());
            (v, Some(buf))
        } else {
            let v = VM::init_gc_with_config(gc, self.inner.config.clone());
            (v, None)
        };
        vm.set_output_capture(sink.capture_buffer());
        // 注册外部 builtin（如 GUI 库注入的函数）
        if let Some(ref extra) = self.inner.extra_builtins {
            vm.register_builtins_from(extra);
        }
        Session {
            vm,
            output: sink,
            engine: Arc::clone(&self.inner),
            _trace_buf: trace_buf,
            current_module_path: None,
        }
    }

    /// 编译并执行源码字符串，返回最终值。
    ///
    /// 每次调用都会创建一个新的临时 [`Session`]，因此多次调用的输出不会混合。
    pub fn run(&self, source: &str) -> NuzoResult<Value> {
        self.new_session().run(source)
    }

    /// 编译并执行源码字符串，返回包含最终值和捕获输出的 [`Output`]。
    pub fn eval(&self, source: &str) -> NuzoResult<Output> {
        let (sink, _buf) = OutputSink::new_capture();
        let mut session = self.new_session_with(sink);
        session.eval(source)
    }

    /// 读取文件内容并作为 Nuzo 脚本执行，返回执行结果。
    ///
    /// 与 [`Engine::eval`](Self::eval) 的区别：本方法将文件路径注入编译上下文，
    /// 使脚本中的 `import "..."` 语句能以当前文件目录为基准解析相对路径。
    ///
    /// 实现细节：创建一个临时 [`Session`]，调用 [`Session::set_module_path`]
    /// 注入文件路径（同时作为相对路径基准与 module_cache 的 key），
    /// 然后通过 [`Session::eval`] 触发带 [`ModuleResolver`] 的编译路径。
    ///
    /// # 错误
    /// 文件读取失败或执行过程中出错时返回 [`NuzoError`]。
    pub fn run_file(&self, path: &Path) -> NuzoResult<Output> {
        let source = self.inner.load_source(path).map_err(crate::error::resolve_err)?;
        let (sink, _buf) = OutputSink::new_capture();
        let mut session = self.new_session_with(sink);
        // 注入当前模块路径，使后续编译能解析相对路径 import 与缓存命中。
        session.set_module_path(path);
        session.eval(&source)
    }

    /// 注册外部 builtin 函数，使其在后续创建的所有 Session 中可用。
    ///
    /// 通过传入闭包，外部 crate 可以将自己的 builtin 注册到 [`BuiltinRegistry`] 中，
    /// 这些 builtin 会随默认 builtin 一起注册到每个新创建的 Session 的 VM 中。
    ///
    /// # 前置条件
    ///
    /// 必须在共享 `Engine` 引用（即创建任何 Session）之前调用，
    /// 否则会 panic。典型用法是在 `Engine::build()` 之后立即调用。
    ///
    /// # 示例
    ///
    /// ```rust,ignore
    /// let mut engine = Engine::builder()
    ///     .with_default_config()
    ///     .build()?;
    /// engine.with_registry(|registry| {
    ///     nuzo_gui::register_all(registry);
    /// });
    /// ```
    pub fn with_registry<F>(&mut self, f: F)
    where
        F: FnOnce(&mut BuiltinRegistry),
    {
        let mut registry = BuiltinRegistry::new();
        f(&mut registry);
        // SAFETY: Engine is single-threaded; extra_builtins is only accessed
        // when creating new sessions (also single-threaded).
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::get_mut(&mut self.inner)
            .expect("with_registry must be called before sharing the Engine");
        inner.extra_builtins = Some(registry);
    }

    /// 注入自定义模块解析器（构建后修改）。
    ///
    /// 与 [`EngineBuilder::with_resolver`] 等价，但允许在 `Engine::quick()` 等
    /// 便捷构造之后覆盖默认的 [`StandardResolver`]。
    ///
    /// # 前置条件
    ///
    /// 必须在共享 `Engine` 引用（即创建任何 Session）之前调用，否则 panic。
    pub fn with_resolver(&mut self, resolver: Arc<dyn ModuleResolver>) {
        let inner = Arc::get_mut(&mut self.inner)
            .expect("with_resolver must be called before sharing the Engine");
        inner.resolver = resolver;
    }

    /// 读取文件并编译为字节码 [`Chunk`]，不执行。
    ///
    /// 与 [`Engine::compile`](Self::compile) 的区别：本方法将文件路径注入编译上下文，
    /// 使脚本中的 `import "..."` 语句能以当前文件目录为基准解析相对路径。
    pub fn compile_file(&self, path: &Path) -> NuzoResult<Chunk> {
        let source = self.inner.load_source(path).map_err(crate::error::resolve_err)?;
        let mut session = self.new_session();
        session.set_module_path(path);
        session.compile(&source)
    }

    /// 将源码字符串编译为字节码 [`Chunk`]，不执行。
    pub fn compile(&self, source: &str) -> NuzoResult<Chunk> {
        Compiler::compile_with_bus(source, Arc::clone(&self.inner.bus)).map_err(Into::into)
    }

    /// 返回一个绑定到本引擎的 [`BenchHarness`]，用于运行基准测试。
    pub fn bench(&self) -> BenchHarness<'_> {
        BenchHarness::new(self)
    }

    /// 返回一个绑定到本引擎的 [`TestHarness`]，用于运行测试套件。
    pub fn test(&self) -> TestHarness<'_> {
        TestHarness::new(self)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let inner = match Arc::get_mut(&mut self.inner) {
            Some(inner) => inner,
            None => return,
        };
        for plugin in &mut inner.plugins {
            plugin.on_stop();
        }
    }
}

// ============================================================================
// ModuleResolver 实现 — 委托给 self.resolver（默认 StandardResolver）
// ============================================================================
//
// 实现放在 `EngineInner` 而非 `Engine` 上：因为 `Session` 持有 `Arc<EngineInner>`
// 而非 `Arc<Engine>`，必须让 Session 能直接通过 `&self.engine` 调用 trait 方法。
//
// T3 重构后，所有 fs/env 调用从 nuzo_run 移至 nuzo_ir 的 StandardResolver。
// 本 impl 仅作委托，便于：
// 1. 保持向后兼容（现有 `engine.inner.resolve(...)` 调用点仍可用）
// 2. wasm32 场景通过 `Engine::with_resolver(MemoryResolver)` 注入无 fs 实现
// 3. 解析逻辑集中在 nuzo_ir::module_resolver，避免双份实现

impl ModuleResolver for EngineInner {
    fn resolve(&self, current: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError> {
        self.resolver.resolve(current, import_path)
    }

    fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
        self.resolver.load_source(path)
    }

    fn check_circular(&self, path: &Path, stack: &[PathBuf]) -> Result<(), ResolveError> {
        self.resolver.check_circular(path, stack)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_two_sessions_captures_do_not_mix() {
        let engine = Engine::quick().unwrap();

        let (sink_a, buf_a) = OutputSink::new_capture();
        let (sink_b, buf_b) = OutputSink::new_capture();

        let mut session_a = engine.new_session_with(sink_a);
        let mut session_b = engine.new_session_with(sink_b);

        session_a.eval(r#"println("A1"); println("A2");"#).unwrap();
        session_b.eval(r#"println("B1"); println("B2");"#).unwrap();

        assert_eq!(buf_a.lock().unwrap().as_slice(), &["A1", "A2"]);
        assert_eq!(buf_b.lock().unwrap().as_slice(), &["B1", "B2"]);
    }

    #[test]
    fn test_null_sink_suppresses_output() {
        let engine = Engine::quick().unwrap();
        let mut session = engine.new_session_with(OutputSink::Null);
        let output = session.eval(r#"println("hidden")"#).unwrap();
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn test_stdout_sink_runs_without_capture() {
        let engine = Engine::quick().unwrap();
        let mut session = engine.new_session_with(OutputSink::Stdout);
        let output = session.eval(r#"println("to stdout")"#).unwrap();
        assert!(output.stdout.is_empty());
    }

    /// RAII 守卫：drop 时递归删除临时目录，避免测试残留文件。
    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_resolve_bare_module_name() {
        // 准备临时 std 目录，写入 math.nuzo
        let std_dir =
            std::env::temp_dir().join(format!("nuzo_test_resolve_bare_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&std_dir);
        std::fs::create_dir_all(&std_dir).unwrap();
        let _guard = TempDirGuard(std_dir.clone());

        let math_file = std_dir.join("math.nuzo");
        std::fs::write(&math_file, "// math module").unwrap();

        // 构造带 std_path 的引擎
        let engine =
            Engine::builder().with_default_config().with_std_path(std_dir.clone()).build().unwrap();

        // 裸模块名 "math"（不含路径分隔符或扩展名）→ 解析为 std_path/math.nuzo
        let resolved = engine.inner.resolve(None, "math").unwrap();
        let expected = std::fs::canonicalize(&math_file).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn test_resolve_bare_module_not_found() {
        // std 目录存在，但不含 nonexistent.nuzo
        let std_dir =
            std::env::temp_dir().join(format!("nuzo_test_resolve_bare_nf_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&std_dir);
        std::fs::create_dir_all(&std_dir).unwrap();
        let _guard = TempDirGuard(std_dir.clone());

        let engine =
            Engine::builder().with_default_config().with_std_path(std_dir.clone()).build().unwrap();

        // 裸模块名 "nonexistent" → 文件不存在 → ModuleNotFound
        let err = engine.inner.resolve(None, "nonexistent").unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { .. }),
            "expected ModuleNotFound, got {:?}",
            err
        );
    }
}
