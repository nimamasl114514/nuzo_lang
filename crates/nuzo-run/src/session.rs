//! Session — single execution context owning a VM instance.

use std::path::{Path, PathBuf};
use std::sync::Arc;
// 使用 web_time::Instant 替代 std::time::Instant：
// wasm32-unknown-unknown 上 std::time::Instant::now() 会 panic
// ("time not implemented on this platform")，web-time 在 wasm32 上使用 performance.now() 实现。
use web_time::Instant;

use nuzo_bytecode::Chunk;
use nuzo_compiler::Compiler;
use nuzo_core::Value;
// 引入 ModuleResolver trait 到作用域：Session::compile 中将 `&EngineInner` 隐式 coerce
// 为 `&dyn ModuleResolver`，trait 必须在作用域内才能触发 unsized coercion。
#[allow(unused_imports)]
use nuzo_ir::module_resolver::ModuleResolver;
use nuzo_vm::VM;

use crate::engine::EngineInner;
use crate::error::NuzoResult;
use crate::output::{Output, OutputSink};

/// 单次脚本执行的上下文，持有独立的 VM 实例和输出捕获目标。
///
/// `Session` 由 [`Engine::new_session`] 或 [`Engine::new_session_with`] 创建。
/// 不同 `Session` 之间的寄存器、堆和输出捕获相互隔离。
pub struct Session {
    pub(crate) vm: VM,
    pub(crate) output: OutputSink,
    pub(crate) engine: Arc<EngineInner>,
    pub(crate) _trace_buf: Option<Arc<std::sync::Mutex<Vec<String>>>>,
    /// 当前模块的源文件路径。
    ///
    /// - `Some(path)`: 当前脚本来自 `path` 文件（通常由 [`Engine::run_file`] 设置）。
    ///   此时编译会通过 [`Compiler::compile_with_bus_and_resolver`] 注入
    ///   [`EngineInner`]（实现 [`ModuleResolver`]）以支持 `import` 语句。
    ///   编译结果会被缓存到 `engine.module_cache`，避免重复编译。
    /// - `None`: 当前脚本是 REPL/字符串入口，无文件路径。
    ///   此时使用旧版 `compile_with_bus` 路径（无 import 解析能力，保持向后兼容）。
    pub(crate) current_module_path: Option<PathBuf>,
}

impl Session {
    /// 将当前 Session 的输出捕获目标压入线程局部栈，并返回 RAII guard，
    /// 在作用域结束时自动弹出。
    fn push_output_capture(&self) -> OutputCaptureGuard {
        nuzo_helpers::builtins::push_output_capture(self.output.capture_buffer());
        OutputCaptureGuard
    }

    /// 设置当前模块的源文件路径。
    ///
    /// 由 [`Engine::run_file`] 在创建 Session 后调用，将文件路径注入编译上下文。
    /// 后续 [`Session::compile`] / [`Session::run`] / [`Session::eval`] 调用会：
    /// - 使用 [`EngineInner`] 作为 [`ModuleResolver`]（解析相对路径 import）
    /// - 以 `path` 为 cache key 查询/写入 `module_cache`
    ///
    /// 路径规范化：调用 `std::fs::canonicalize` 将传入路径转换为绝对规范化路径。
    /// 这确保 [`EngineInner::resolve`]（import 路径解析）返回的规范化路径与
    /// `module_cache` 的 key 一致 —— 后者是 `OP_INIT_MODULE` 运行期在
    /// `VM.module_cache`（key 为 `String`）查找模块 chunk 时的目标字符串。
    /// 若规范化失败（文件不存在等），保留原始路径以保留错误上下文。
    pub fn set_module_path(&mut self, path: &Path) {
        // wasm32 目标无文件系统，直接保留原始路径
        #[cfg(not(target_arch = "wasm32"))]
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        #[cfg(target_arch = "wasm32")]
        let canonical = path.to_path_buf();
        self.current_module_path = Some(canonical);
    }

    /// 将 Engine 的 `module_cache`（`PathBuf → Arc<Chunk>`）注入到 VM 的
    /// `module_cache`（`String → Arc<Chunk>`）。
    ///
    /// # 背景（Wave 4 → Wave 5 集成缺口）
    /// - Engine 在编译期通过 [`ModuleResolver`] 递归解析 import，将主模块的 chunk
    ///   缓存到 `engine.module_cache`（key 为规范化 [`PathBuf`]）。
    /// - VM 的 `OP_INIT_MODULE`（lazy import 运行期触发）从 `vm.module_cache`
    ///   按 `String` key 查找模块 chunk —— 该 cache 默认为空。
    ///
    /// 本方法在 `VM::run` 调用前执行注入，桥接两端：
    /// - key 转换：`PathBuf.to_string_lossy()` → `String`
    /// - value 共享：`Arc<Chunk>` 直接克隆（零拷贝引用计数）
    ///
    /// # 性能
    /// 每次 `eval`/`run` 都会重新注入（包括重复 key 覆盖）。
    /// 因为 `module_cache` 通常较小（一个程序的依赖数量有限），
    /// 且 `Arc::clone` 是原子操作，开销可忽略。
    ///
    /// # 路径一致性
    /// `Engine.module_cache` 的 key 在 [`Session::set_module_path`] 中已规范化，
    /// 与 `IrBuilder::resolve_imports` 中 `resolver.resolve()` 返回的规范化路径一致。
    /// 因此 `to_string_lossy()` 后的字符串与 codegen 写入常量池的路径字符串可对齐。
    fn inject_engine_modules_into_vm(&mut self) {
        // 读取 Engine 缓存（持有读锁期间收集，避免与 VM 调用交叠）
        // 采用 poison recovery 模式：若其它线程 panic 导致锁中毒，
        // 仍取出内部数据继续使用，避免单次 panic 让整个 Session 不可用。
        let modules: Vec<(String, Arc<Chunk>)> = {
            let cache = self.engine.module_cache.read().unwrap_or_else(|e| e.into_inner());
            cache
                .iter()
                .map(|(path, chunk)| (path.to_string_lossy().into_owned(), Arc::clone(chunk)))
                .collect()
        };
        self.vm.register_modules(modules);
    }

    /// 清除当前模块路径，恢复到无 import 解析的默认模式。
    ///
    /// 主要用于测试场景：在多次复用同一 Session 时切换上下文。
    #[allow(dead_code)] // 测试辅助 API，保留供多 Session 复用场景使用
    pub(crate) fn clear_module_path(&mut self) {
        self.current_module_path = None;
    }

    /// 编译并执行源码字符串，返回脚本最终值。
    ///
    /// 输出按本 Session 创建时指定的 [`OutputSink`] 处理。
    ///
    /// # import 集成
    /// 在 `VM::run` 调用前，通过 [`inject_engine_modules_into_vm`](Self::inject_engine_modules_into_vm)
    /// 将 Engine 缓存的模块 chunk 注入到 VM 的 `module_cache`，使
    /// `OP_INIT_MODULE`（lazy import）能在运行期按路径字符串查找到模块。
    pub fn run(&mut self, source: &str) -> NuzoResult<Value> {
        let _guard = self.push_output_capture();
        let chunk = self.compile(source)?;
        self.inject_engine_modules_into_vm();
        self.vm.run(chunk)
    }

    /// 编译并执行源码字符串，返回包含最终值、捕获输出和执行耗时的 [`Output`]。
    ///
    /// 如果当前输出目标为捕获模式，执行前会清空已有捕获内容。
    ///
    /// # import 集成
    /// 与 [`run`](Self::run) 一致，在 `VM::run` 前注入 Engine 的 module_cache
    /// 到 VM（[`inject_engine_modules_into_vm`](Self::inject_engine_modules_into_vm)）。
    pub fn eval(&mut self, source: &str) -> NuzoResult<Output> {
        let _guard = self.push_output_capture();
        let start = Instant::now();
        if let OutputSink::Capture(ref buf) = self.output
            && let Ok(mut g) = buf.lock()
        {
            g.clear();
        }
        let chunk = self.compile(source)?;
        self.inject_engine_modules_into_vm();
        let value = self.vm.run(chunk)?;
        let duration = start.elapsed();
        let stdout = match &self.output {
            OutputSink::Capture(buf) => buf.lock().map(|g| g.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        Ok(Output { value, stdout, duration })
    }

    /// 将源码字符串编译为字节码 [`Chunk`]，不执行。
    ///
    /// # 编译路径选择
    /// - 若 `current_module_path` 为 `Some`：使用 [`Compiler::compile_with_bus_and_resolver`]
    ///   注入 [`EngineInner`] 作为 [`ModuleResolver`]，支持 `import` 语句。
    ///   编译结果按路径缓存到 `engine.module_cache`，重复 import 同一文件只编译一次。
    /// - 若 `current_module_path` 为 `None`：使用旧版 [`Compiler::compile_with_bus`]，
    ///   无 import 解析能力，保持向后兼容。
    pub fn compile(&self, source: &str) -> NuzoResult<Chunk> {
        if let Some(path) = &self.current_module_path {
            if let Some(cached) =
                self.engine.module_cache.read().ok().and_then(|g| g.get(path).cloned())
            {
                return Ok((*cached).clone());
            }

            // 2. 调用带 resolver 的编译入口
            //    EngineInner 实现 ModuleResolver trait，提供路径解析/源码加载/循环检测
            //    返回 (主模块 Chunk, 子模块 Chunks)：子模块 Chunks 由 CodeGenerator
            //    从 ImportRecord.functions 生成，需注册到 module_cache 供 OP_INIT_MODULE 使用
            let (chunk, sub_chunks) = Compiler::compile_with_bus_and_resolver(
                source,
                Arc::clone(&self.engine.bus),
                self.engine.as_ref(), // &EngineInner: &dyn ModuleResolver
                Some(path.as_path()),
            )?;

            // 3. 写入缓存（子模块 + 主模块，按规范化路径去重）
            //    子模块 Chunk 的 key 是 ImportRecord.path.to_string_lossy() 转回 PathBuf，
            //    与 inject_engine_modules_into_vm 中 PathBuf → to_string_lossy() 的转换对齐，
            //    确保 VM module_cache 的 key 与 OP_INIT_MODULE 常量池中的路径字符串一致。
            if let Ok(mut g) = self.engine.module_cache.write() {
                for (path_str, sub_chunk) in &sub_chunks {
                    g.insert(PathBuf::from(path_str), Arc::new(sub_chunk.clone()));
                }
                g.insert(path.clone(), Arc::new(chunk.clone()));
            }

            return Ok(chunk);
        }

        // 无路径 → 旧路径（NullResolver 等价，无 import 解析）
        Compiler::compile_with_bus(source, Arc::clone(&self.engine.bus)).map_err(Into::into)
    }

    /// 直接执行一个已编译的字节码 [`Chunk`]，返回最终值。
    pub fn execute(&mut self, chunk: Chunk) -> NuzoResult<Value> {
        self.vm.run(chunk)
    }

    pub fn reset(&mut self) {
        self.vm = VM::with_config(self.engine.config.clone());
    }

    pub fn vm_mut(&mut self) -> &mut VM {
        &mut self.vm
    }

    /// 消费 Session，返回内部的 VM 实例。
    ///
    /// 用于需要将 VM 从 Session 中取出以便外部控制执行循环的场景
    /// （如 GUI 每帧调用 `call_global_function`）。
    ///
    /// 注意：调用后 Session 将被消费，其输出捕获和模块路径等信息将丢失。
    pub fn into_vm(self) -> VM {
        self.vm
    }

    /// 返回当前已捕获的标准输出内容副本。
    ///
    /// 如果输出目标不是捕获模式，返回空向量。
    pub fn stdout(&self) -> Vec<String> {
        match &self.output {
            OutputSink::Capture(buf) => buf.lock().map(|g| g.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn flush_stdout(&self) {
        if let OutputSink::Capture(ref buf) = self.output
            && let Ok(g) = buf.lock()
        {
            for line in g.iter() {
                print!("{}", line);
            }
        }
    }
}

/// 输出捕获栈帧的 RAII guard，确保 `push_output_capture` 之后一定调用
/// `pop_output_capture`，即使执行过程中发生 panic。
struct OutputCaptureGuard;

impl Drop for OutputCaptureGuard {
    fn drop(&mut self) {
        nuzo_helpers::builtins::pop_output_capture();
    }
}
