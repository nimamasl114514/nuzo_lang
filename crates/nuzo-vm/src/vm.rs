//! Nuzo Virtual Machine (VM) - Bytecode Interpreter
//!
//! # Module Structure
//!
//! | Sub-module | Responsibility |
//! |------------|----------------|
//! | `builtin_registration` | Builtin function registration |
//! | `variable_ops` | Variable/register/global/diagnostic/GC-root ops |
//! | `call_dispatch` | Frame management, call dispatch, tracing, fetch |

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use crate::elastic_register_file::ElasticRegisterFile;
use crate::gc::Gc;
use crate::vm_hot_trace::HotTraceTable;
use crate::vm_lic::CallSites;
use nuzo_bytecode::scope::GlobalScope;
use nuzo_bytecode::{Chunk, Opcode};
use nuzo_config::Config;
use nuzo_core::Value;
use nuzo_core::{INITIAL_FRAME_CAPACITY, INITIAL_REGISTERS, InternalError};
use nuzo_error::ErrorCollector;
use nuzo_signal::VmErrorInfo;
use nuzo_values::{HeapObject, NIL, NuzoError, SourceLocation, ValueExt};

// ============================================================================
// Sub-module declarations
// ============================================================================

#[path = "dispatch.rs"]
pub(crate) mod dispatch;

#[path = "dispatch_table.rs"]
mod dispatch_table;

#[path = "builtin_registration.rs"]
mod builtin_registration;

#[path = "variable_ops.rs"]
pub(crate) mod variable_ops;

#[path = "call_dispatch.rs"]
pub(crate) mod call_dispatch;

#[path = "frame_v6.rs"]
pub(crate) mod frame_v6;

// ============================================================================
// Constants
// ============================================================================

const BUILTIN_ARGS_BUF_SIZE: usize = 16;
const CHUNK_CACHE_INITIAL_CAPACITY: usize = 64;
const MAX_BATCH_ITERATIONS: u32 = 100_000;

const UNINITIALIZED_TAG: u8 = 0xFF;
const PIC_EVICTION_THRESHOLD: u16 = 3;
const VALUE_TAG_SHIFT: u32 = 56;

#[inline(always)]
pub(super) fn extract_type_tag(val: Value) -> u8 {
    (val.into_raw_bits() >> VALUE_TAG_SHIFT) as u8
}

// ============================================================================
// InlineCacheEntry
// ============================================================================

#[derive(Debug, Clone)]
pub(super) struct InlineCacheEntry {
    cached_tag: u8,
    secondary_cached_tag: u8,
    consecutive_hit_count: u16,
}

impl Default for InlineCacheEntry {
    #[inline]
    fn default() -> Self {
        Self {
            cached_tag: UNINITIALIZED_TAG,
            secondary_cached_tag: UNINITIALIZED_TAG,
            consecutive_hit_count: 0,
        }
    }
}

impl InlineCacheEntry {
    #[inline]
    fn record(&mut self, tag: u8) {
        if self.cached_tag == tag {
            self.consecutive_hit_count = self.consecutive_hit_count.saturating_add(1);
        } else if self.secondary_cached_tag == tag {
            self.secondary_cached_tag = self.cached_tag;
            self.cached_tag = tag;
        } else if self.consecutive_hit_count < PIC_EVICTION_THRESHOLD {
            self.consecutive_hit_count += 1;
        } else {
            self.secondary_cached_tag = self.cached_tag;
            self.cached_tag = tag;
            self.consecutive_hit_count = 1;
        }
    }
}

// ============================================================================
// VM Observer (replaces VM_WILL_EXECUTE / VM_ERROR signals)
// ============================================================================

/// VM 执行观察者（替代 VM_WILL_EXECUTE / VM_ERROR 信号）
///
/// 单观察者回调模式，比信号总线开销更低：
/// - 无订阅者时 (observer = None) 零开销
/// - 有订阅者时仅一次虚调用，比信号总线的快照+遍历快
pub trait VmObserver: Send + Sync {
    /// 每条指令执行前调用
    fn on_will_execute(&self, _opcode: u8, _ip: usize) {}
    /// VM 运行时错误
    fn on_error(&self, _info: &VmErrorInfo) {}
}

/// 空实现（用于默认无观察者的场景）
pub struct NoopVmObserver;
impl VmObserver for NoopVmObserver {}

pub(super) fn gc_roots_trampoline(gc: &mut Gc, userdata: *mut c_void) {
    let vm_ptr = userdata as *const VM;
    if !vm_ptr.is_null() {
        // SAFETY:
        // 1. `userdata` is registered by VM::install_gc_roots (or equivalent) as
        //    `self as *const VM as *mut c_void`. The VM owns the Gc instance and
        //    outlives every GC cycle: GC only runs synchronously inside VM
        //    methods (collect/collect_with_roots), so the borrow is confined to
        //    the call stack of those methods.
        // 2. The VM is single-threaded by design — Gc does not implement Send
        //    for its roots callback. Therefore there is no concurrent access to
        //    the VM during GC: the GC mutator is paused while roots are scanned.
        // 3. `vm_ptr` is checked for null above (defensive against malformed
        //    userdata). The shared reference `&*vm_ptr` is sound because the VM
        //    is not mutably borrowed elsewhere during the GC callback (the
        //    `&mut Gc` passed in is the only active borrow; VM and Gc are
        //    separate allocations).
        // 4. `collect_gc_roots(&self, gc)` takes a shared reference to the VM
        //    and a mutable reference to the Gc — these do not alias because the
        //    Gc is a field of the VM but the &mut Gc was already projected out
        //    before invoking the trampoline, and the Gc is not accessed through
        //    the VM reference during root scanning.
        unsafe { &*vm_ptr }.collect_gc_roots(gc);
    }
}

// ============================================================================
// Frame Kind
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FrameKind {
    Normal,
    Trampoline,
}

// ============================================================================
// TcoRecord
// ============================================================================

#[derive(Clone, Debug)]
#[allow(dead_code)] // TCO 调试记录，保留供 TCO 优化诊断使用
pub(super) struct TcoRecord {
    pub replaced_closure: Option<Arc<HeapObject>>,
    pub replaced_ip: usize,
    pub replaced_return_address: usize,
    pub replaced_chunk: Option<Arc<Chunk>>,
}

// ============================================================================
// CallFrame — SCHF v6 Phase 4 已移除
// ============================================================================
//
// 历史上 ExecutionContext 持有 `frames: VecDeque<CallFrame>` 作为帧栈。
// SCHF v6 用 `frame_data: FrameData` + `frame_ring: FrameRing` +
// `frame_metas: Vec<FrameMeta>` + `frame_overflow: OverflowStack` 替代。
//
// 帧的控制字段（return_address, base）现存储在 FrameInfo（16B 紧凑环形槽）；
// 帧的冷路径字段（closure, caller_chunk, call_site, arena, tco_*）现存储在
// FrameMeta（Vec<FrameMeta>）。
//
// 详见 `frame_v6.rs` 和 `SCHF_V6_SPEC.md`。

// ============================================================================
// Exception Frame
// ============================================================================

#[derive(Debug, Clone)]
pub(super) struct ExceptionFrame {
    catch_ip: usize,
    exc_reg: u16,
    #[allow(dead_code)] // 异常帧恢复时需要 base_stack_size，保留供异常展开使用
    base_stack_size: usize,
}

// ============================================================================
// ExecutionContext
// ============================================================================

/// VM 的执行上下文，包含寄存器文件、调用栈、全局作用域等运行时状态。
///
/// 高级用户可通过此结构体检查或重置 VM 的运行时状态；
/// 通常由 [`VM::new`] 系列构造器自动创建。
pub struct ExecutionContext {
    pub(super) registers: ElasticRegisterFile,
    pub(super) register_write_ptr: usize,
    pub(super) running: bool,
    pub(super) global_scope: GlobalScope,
    pub(super) chunk_cache: HashMap<usize, Arc<Chunk>>,
    pub(super) hot_trace_table: HotTraceTable,
    pub(super) call_sites: CallSites,
    pub(super) inline_cache: Vec<InlineCacheEntry>,
    pub(super) last_call_stack: Vec<nuzo_error::StackFrameInfo>,
    pub(crate) prop_ic: Vec<dispatch::PropICSlot>,
    pub(super) global_cache: Vec<dispatch::GlobalCacheEntry>,
    pub(super) global_versions: Vec<u32>,
    pub(super) region: crate::arena::RegionAllocator,
    /// 已编译的模块字节码缓存 (path_string → Arc<Chunk>)。
    ///
    /// 用于 lazy import 的 `OP_INIT_MODULE`：避免在 VM 内重新编译。
    /// key 使用 `String`（而非 `PathBuf`），与 `InternalError::ModuleNotLoaded { path: String }`
    /// 对齐；由 Engine 在 `VM::run` 调用前通过 `VM::register_module` 注入。
    pub(super) module_cache: HashMap<String, Arc<Chunk>>,
    // === SCHF v6 影子帧栈（Phase 2 双写，Phase 3 切换读取路径） ===
    /// 连续值栈：帧 = data[base..base+n_cip]，bump pointer 推进 top。
    pub(super) frame_data: frame_v6::FrameData,
    /// 64 槽环形 FrameInfo 缓冲（热路径：return_address + base）。
    pub(super) frame_ring: frame_v6::FrameRing,
    /// 帧元数据（与 frames 一一对应，含 spill 时插入的 trampoline meta）。
    pub(super) frame_metas: Vec<frame_v6::FrameMeta>,
    /// ring 溢出降级存储（>64 层递归时使用）。
    pub(super) frame_overflow: frame_v6::OverflowStack,
}

impl ExecutionContext {
    /// 使用默认容量创建新的执行上下文。
    pub fn new() -> Self {
        Self::with_capacity(INITIAL_REGISTERS, INITIAL_FRAME_CAPACITY)
    }

    /// 使用指定的初始寄存器容量和帧容量创建执行上下文。
    pub fn with_capacity(initial_registers: usize, initial_frame_capacity: usize) -> Self {
        let _ = initial_frame_capacity; // SCHF v6: 帧容量由 frame_data 预分配管理
        Self {
            registers: ElasticRegisterFile::with_capacity(initial_registers),
            register_write_ptr: 0,
            running: false,
            global_scope: GlobalScope::new(),
            chunk_cache: HashMap::with_capacity(CHUNK_CACHE_INITIAL_CAPACITY),
            hot_trace_table: HotTraceTable::new(),
            call_sites: CallSites::new(),
            inline_cache: Vec::new(),
            last_call_stack: Vec::new(),
            prop_ic: vec![dispatch::PropICSlot::default(); 64],
            global_cache: Vec::new(),
            global_versions: Vec::new(),
            region: crate::arena::RegionAllocator::with_default(),
            module_cache: HashMap::new(),
            frame_data: frame_v6::FrameData::default(),
            frame_ring: frame_v6::FrameRing::default(),
            frame_metas: Vec::new(),
            frame_overflow: frame_v6::OverflowStack::default(),
        }
    }

    pub fn snapshot_for_chunk_switch(&mut self) {
        self.running = true;
        self.hot_trace_table = HotTraceTable::new();
        self.call_sites = CallSites::new();
        self.inline_cache.clear();
        self.last_call_stack.clear();
    }

    pub fn reset_registers_and_frames(&mut self, locals_count: u16) {
        self.registers.clear();
        self.registers.resize(locals_count as usize, NIL);
        self.register_write_ptr = self.registers.len();
        self.region.reset();
        // SCHF v6 Phase 4：重置 v6 帧栈（VecDeque 已移除）
        self.frame_data.clear();
        self.frame_ring.clear();
        self.frame_metas.clear();
        self.frame_overflow.clear();
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// VM Struct Definition (field order MUST NOT change)
// ============================================================================

/// Nuzo 虚拟机，负责解释执行字节码 [`Chunk`]。
///
/// `VM` 持有垃圾回收器、寄存器文件、调用栈和全局作用域。
/// 通过 [`VM::run`] 传入字节码并执行，返回脚本最终值或运行时错误。
pub struct VM {
    gc: Box<Gc>,
    pub(super) chunk: Option<Arc<Chunk>>,
    chunk_ptr: *const Chunk,
    pub(super) ip: usize,
    current_base: usize,
    error_collector: ErrorCollector,
    pub(super) max_stack_size: usize,
    pub(super) output_capture: Option<Arc<Mutex<Vec<String>>>>,
    tracer: Option<crate::tracer_state::TracerState>,
    frame_pager: crate::frame_paging::FramePager,
    builtin_args_buf: [Value; BUILTIN_ARGS_BUF_SIZE],
    closure_invokers: std::collections::HashMap<usize, dispatch::ClosureInvoker>,
    pub(super) exception_stack: Vec<ExceptionFrame>,
    pub(super) pending_exception: Option<Value>,
    pub cx: ExecutionContext,
    execution_timeout_ms: Option<u64>,
    execution_start: web_time::Instant,
    observer: Option<Box<dyn VmObserver>>,
}

impl Drop for VM {
    fn drop(&mut self) {
        nuzo_values::unregister_gc_heap_alloc();
        crate::gc::heap::clear_gc_heap_gc_ptr();
        self.gc.register_roots_fn(None, std::ptr::null_mut());
    }
}

// ============================================================================
// Internal Initialization + Public Constructors
// ============================================================================

struct VmInitConfig {
    gc: Option<Gc>,
    output_capture: Option<Arc<Mutex<Vec<String>>>>,
    tracer: Option<crate::tracer_state::TracerState>,
    config: Option<Config>,
}

impl VM {
    #[inline]
    fn init_with(init_config: VmInitConfig) -> Self {
        let cfg = init_config.config.unwrap_or_default();
        let max_stack_size = cfg.vm.max_stack_size;
        let initial_registers = cfg.vm.initial_registers;
        let initial_frame_capacity = cfg.vm.initial_frame_capacity;

        let gc = Box::new(match init_config.gc {
            Some(gc) => gc,
            None => Gc::with_config(cfg.gc.clone()),
        });

        let mut vm = VM {
            gc,
            chunk: None,
            chunk_ptr: std::ptr::null(),
            ip: 0,
            current_base: 0,
            error_collector: ErrorCollector::new(),
            max_stack_size,
            output_capture: init_config.output_capture,
            tracer: init_config.tracer,
            frame_pager: crate::frame_paging::FramePager::with_config(cfg.frame_paging),
            builtin_args_buf: [NIL; BUILTIN_ARGS_BUF_SIZE],
            closure_invokers: std::collections::HashMap::new(),
            exception_stack: Vec::new(),
            pending_exception: None,
            cx: ExecutionContext::with_capacity(initial_registers, initial_frame_capacity),
            execution_timeout_ms: cfg.vm.execution_timeout_ms,
            execution_start: web_time::Instant::now(),
            observer: None,
        };
        vm.gc.register_roots_fn(Some(gc_roots_trampoline), std::ptr::null_mut());
        // `vm` is about to be moved to the caller, so we store a pointer to the
        // heap-allocated Gc (which has a stable address) rather than to the VM.
        crate::gc::heap::set_gc_heap_gc_ptr(&mut *vm.gc);
        crate::gc::install_scratch_aware_accessors(vm.gc.scratch_data_ptr(), &mut vm.cx.region);
        crate::gc::update_gc_chunks_ptr(&vm.gc);
        vm.register_builtins();
        vm
    }

    /// 使用默认配置创建新的 VM。
    pub fn new() -> Self {
        Self::init_with(VmInitConfig { gc: None, output_capture: None, tracer: None, config: None })
    }

    /// 使用外部 [`Gc`] 实例创建 VM。
    ///
    /// 适用于需要共享 GC 或自定义 GC 配置的场景。
    pub fn init_gc(gc: Gc) -> Self {
        Self::init_with(VmInitConfig {
            gc: Some(gc),
            output_capture: None,
            tracer: None,
            config: None,
        })
    }

    /// 使用外部 [`Gc`] 和 [`Config`] 创建 VM。
    ///
    /// 适用于需要共享 GC 并指定 VM/编译器/垃圾回收配置的场景。
    pub fn init_gc_with_config(gc: Gc, config: Config) -> Self {
        Self::init_with(VmInitConfig {
            gc: Some(gc),
            output_capture: None,
            tracer: None,
            config: Some(config),
        })
    }

    /// 使用外部 [`Gc`]、[`Config`] 和追踪器配置创建 VM。
    ///
    /// 返回 VM 实例以及用于收集追踪输出的共享缓冲区。
    pub fn init_gc_with_config_and_tracer(
        gc: Gc,
        config: Config,
        trace_config: crate::tracer_state::TraceConfig,
    ) -> (Self, Arc<Mutex<Vec<String>>>) {
        use crate::tracer_state::TracerState;
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        nuzo_helpers::configure_output_capture(Some(buf.clone()));
        let vm = Self::init_with(VmInitConfig {
            gc: Some(gc),
            output_capture: Some(buf.clone()),
            tracer: Some(TracerState::new(trace_config)),
            config: Some(config),
        });
        (vm, buf)
    }

    /// 创建 VM 并启用输出捕获，返回 VM 和共享捕获缓冲区。
    pub fn new_with_output_capture() -> (Self, Arc<Mutex<Vec<String>>>) {
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        nuzo_helpers::configure_output_capture(Some(buf.clone()));
        let vm = Self::init_with(VmInitConfig {
            gc: None,
            output_capture: Some(buf.clone()),
            tracer: None,
            config: None,
        });
        (vm, buf)
    }

    /// 创建 VM 并同时启用输出捕获和指令追踪器。
    pub fn new_with_output_capture_and_tracer(
        config: crate::tracer_state::TraceConfig,
    ) -> (Self, Arc<Mutex<Vec<String>>>) {
        use crate::tracer_state::TracerState;
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        nuzo_helpers::configure_output_capture(Some(buf.clone()));
        let vm = Self::init_with(VmInitConfig {
            gc: None,
            output_capture: Some(buf.clone()),
            tracer: Some(TracerState::new(config)),
            config: None,
        });
        (vm, buf)
    }

    /// 使用指定的 [`Config`] 创建 VM。
    pub fn with_config(config: Config) -> Self {
        Self::init_with(VmInitConfig {
            gc: None,
            output_capture: None,
            tracer: None,
            config: Some(config),
        })
    }

    /// 使用指定的 [`Config`] 和追踪器配置创建 VM，并启用输出捕获。
    pub fn with_config_and_tracer(
        config: Config,
        trace_config: crate::tracer_state::TraceConfig,
    ) -> (Self, Arc<Mutex<Vec<String>>>) {
        use crate::tracer_state::TracerState;
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        nuzo_helpers::configure_output_capture(Some(buf.clone()));
        let vm = Self::init_with(VmInitConfig {
            gc: None,
            output_capture: Some(buf.clone()),
            tracer: Some(TracerState::new(trace_config)),
            config: Some(config),
        });
        (vm, buf)
    }

    /// 设置 VM 的 Print opcode 输出捕获缓冲区。
    ///
    /// 与线程局部的 builtin 捕获栈分离，应由 Session 在创建时按自己的
    /// 输出目标设置。
    pub fn set_output_capture(&mut self, capture: Option<Arc<Mutex<Vec<String>>>>) {
        self.output_capture = capture;
    }

    // ========================================================================
    // Module Cache (Lazy Import Support)
    // ========================================================================

    /// 注入预编译的模块字节码（用于 lazy import 支持）。
    ///
    /// 在 [`VM::run`] 调用前由 Engine 调用：将已编译的模块 [`Chunk`] 以
    /// `path` 字符串为 key 注入 `module_cache`，供 `OP_INIT_MODULE` 在运行期
    /// 首次访问 lazy import 模块时直接取出执行，避免在 VM 内重新编译。
    ///
    /// 重复注入同一 `path` 会覆盖旧值（Engine 应保证规范化路径一致）。
    pub fn register_module(&mut self, path: &str, chunk: Arc<Chunk>) {
        self.cx.module_cache.insert(path.to_string(), chunk);
    }

    /// 批量注入预编译模块字节码。
    ///
    /// 等价于多次调用 [`VM::register_module`](Self::register_module)，但
    /// 仅一次获取 HashMap 写锁，性能更优。Engine 在 Session 创建时使用。
    pub fn register_modules<I>(&mut self, modules: I)
    where
        I: IntoIterator<Item = (String, Arc<Chunk>)>,
    {
        self.cx.module_cache.extend(modules);
    }

    /// 查询当前 VM 中已注册的模块数量（主要用于测试与诊断）。
    #[inline]
    pub fn registered_module_count(&self) -> usize {
        self.cx.module_cache.len()
    }

    /// 查询某个模块路径是否已注册（主要用于测试与诊断）。
    #[inline]
    pub fn is_module_registered(&self, path: &str) -> bool {
        self.cx.module_cache.contains_key(path)
    }

    // ========================================================================
    // Observer Builder Methods
    // ========================================================================

    /// Builder 链式方法：为 VM 设置执行观察者
    ///
    /// 单观察者回调模式，比信号总线开销更低：
    /// - 无观察者时 (observer = None) 零开销
    /// - 有观察者时仅一次虚调用
    ///
    /// # Example
    ///
    /// ```ignore
    /// use nuzo_vm::{VM, VmObserver, NoopVmObserver};
    /// let vm = VM::new().with_observer(Box::new(NoopVmObserver));
    /// ```
    pub fn with_observer(mut self, observer: Box<dyn VmObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// 原地设置观察者（适用于已构造的 VM 实例）
    pub fn set_observer(&mut self, observer: Box<dyn VmObserver>) {
        self.observer = Some(observer);
    }

    /// 获取当前观察者的引用（如果已设置）
    pub fn observer(&self) -> Option<&dyn VmObserver> {
        self.observer.as_deref()
    }

    // ========================================================================
    // Source Location Helpers
    // ========================================================================

    /// Get the source location of the currently executing instruction.
    ///
    /// Uses `ip - 1` because `fetch_opcode` has already advanced `self.ip`
    /// past the opcode byte, while debug info is keyed by the opcode's
    /// start address.
    #[inline]
    fn current_source_location(&self) -> Option<SourceLocation> {
        if self.chunk_ptr.is_null() {
            return None;
        }
        let chunk = unsafe { &*self.chunk_ptr };
        chunk.get_source_location(self.ip.saturating_sub(1))
    }

    /// Attach the current source location to a runtime error if available.
    #[cold]
    #[inline(never)]
    fn error_with_source_location(&self, error: NuzoError) -> NuzoError {
        match self.current_source_location() {
            Some(loc) => error.with_source_location(loc),
            None => error,
        }
    }

    // ========================================================================
    // Main Execution Loop
    // ========================================================================

    /// 加载并执行一个字节码 [`Chunk`]，返回脚本最终值。
    ///
    /// 执行前会重置 VM 状态并将 `chunk` 加载为当前执行块。
    /// 执行过程中发生运行时错误时返回 [`NuzoError`]。
    pub fn run(&mut self, chunk: Chunk) -> Result<Value, NuzoError> {
        self.reset_and_load_chunk(chunk);
        self.cx.registers.activate();
        if self.execution_timeout_ms.is_some() {
            self.execution_start = web_time::Instant::now();
        }
        let result = self.run_inner();
        self.cx.registers.deactivate();
        result
    }

    /// 按名称调用全局作用域中的函数，不重置 VM 状态。
    ///
    /// 用于 GUI 等需要反复调用同一函数（如 `render()`）的场景：
    /// 1. 先调用 `run(chunk)` 执行脚本（定义函数 + 初始化状态）
    /// 2. 之后每帧调用 `call_global_function("render", &[])` 执行渲染
    ///
    /// 与 `run()` 的区别：**不重置寄存器/帧栈**，保留全局变量状态。
    ///
    /// # 参数
    /// - `name`: 全局变量名（如 "render"）
    /// - `args`: 传递给函数的参数
    ///
    /// # 返回值
    /// 函数的返回值，或错误。
    ///
    /// # 错误
    /// - 全局变量不存在 → `InternalError::CompilerBug`
    /// - 全局变量不是闭包 → `NuzoError::TypeMismatch`
    pub fn call_global_function(&mut self, name: &str, args: &[Value]) -> Result<Value, NuzoError> {
        let func_val = self.get_global_by_name(name).ok_or_else(|| {
            NuzoError::internal(
                InternalError::CompilerBug {
                    message: format!("global function '{}' not found", name),
                },
                None,
            )
        })?;

        if !func_val.is_closure() {
            return Err(NuzoError::type_mismatch("function (closure)", func_val.type_name()));
        }

        // Extract closure data
        let closure_heap_obj = func_val.as_closure_heap_object_opt().ok_or_else(|| {
            NuzoError::internal(
                InternalError::CompilerBug {
                    message: format!("closure heap object not found for '{}'", name),
                },
                None,
            )
        })?;

        let (prototype, arity) = match &*closure_heap_obj {
            HeapObject::Closure { prototype, .. } => {
                (Arc::clone(prototype), prototype.arity as usize)
            }
            _ => {
                return Err(NuzoError::type_mismatch("closure", "non-closure heap object"));
            }
        };

        if args.len() != arity {
            return Err(NuzoError::invalid_argument_count(arity, args.len()));
        }

        // Save current state for restoration after call
        let saved_ip = self.ip;
        let saved_base = self.current_base;
        let saved_chunk = self.chunk.clone();
        let saved_register_len = self.cx.registers.len();

        // Set up call frame (similar to execute_normal_call)
        let new_base = self.cx.registers.len();

        // Push arguments onto register stack
        for arg in args {
            self.cx.registers.push(*arg);
        }

        // Pad registers for function locals
        let needed = new_base + args.len() + prototype.locals_count as usize;
        if needed > self.max_stack_size {
            return Err(NuzoError::internal(
                InternalError::StackOverflow { depth: needed, max_depth: self.max_stack_size },
                None,
            ));
        }
        self.cx.registers.resize(needed, Value::default());

        // Push call frame
        let return_address = saved_ip;
        self.push_frame_with_base(
            return_address,
            new_base,
            Some(closure_heap_obj),
            0, // caller_func_reg: not meaningful for external calls
            saved_chunk.clone(),
        )?;

        // Load function chunk and start execution
        let func_chunk = self.get_or_create_chunk(&prototype);
        self.chunk = Some(func_chunk.clone());
        self.chunk_ptr = Arc::as_ptr(&func_chunk);
        self.invalidate_cigc_cache();
        self.ip = 0;
        self.current_base = new_base;

        // Run until Return
        self.cx.running = true;
        self.cx.registers.activate();
        let result = self.run_inner();
        self.cx.registers.deactivate();
        self.cx.running = false;

        // Restore state
        self.ip = saved_ip;
        self.current_base = saved_base;
        self.chunk = saved_chunk;
        if let Some(ref c) = self.chunk {
            self.chunk_ptr = Arc::as_ptr(c);
        }
        self.invalidate_cigc_cache();
        self.cx.registers.truncate(saved_register_len);

        result
    }

    /// 设置单次执行的超时时间（毫秒）。
    ///
    /// 传入 `None` 表示不限制执行时间。
    pub fn set_execution_timeout(&mut self, ms: Option<u64>) {
        self.execution_timeout_ms = ms;
    }

    fn run_inner(&mut self) -> Result<Value, NuzoError> {
        let mut timeout_check_counter: u64 = 0;
        const TIMEOUT_CHECK_INTERVAL: u64 = 1024;
        let observer_enabled = self.observer.is_some();
        let error_collector_enabled = self.error_collector.is_enabled();
        let trace_enabled = self.tracer.is_some();

        while self.cx.running {
            if let Some(limit_ms) = self.execution_timeout_ms {
                timeout_check_counter = timeout_check_counter.wrapping_add(1);
                if timeout_check_counter >= TIMEOUT_CHECK_INTERVAL {
                    timeout_check_counter = 0;
                    if self.execution_start.elapsed().as_millis() as u64 > limit_ms {
                        return Err(NuzoError::execution_timeout(limit_ms));
                    }
                }
            }

            if self.chunk_ptr.is_null() {
                break;
            }
            let chunk = unsafe { &*self.chunk_ptr };
            if self.ip >= chunk.code().len() {
                break;
            }

            if self.cx.hot_trace_table.is_hot_trace(self.ip) {
                let end_ip = self.cx.hot_trace_table.hot_trace_end(self.ip);
                self.execute_hot_trace_batch(end_ip)?;
                continue;
            }

            if error_collector_enabled {
                self.error_collector.record_instruction();
            }

            let opcode = match self.fetch_opcode() {
                Ok(op) => op,
                Err(e) => {
                    if let Some(ref obs) = self.observer {
                        obs.on_error(&VmErrorInfo {
                            error_message: format!("{:?}", e),
                            opcode: None,
                            ip: self.ip.saturating_sub(1),
                            call_depth: self.frame_depth(),
                        });
                    }
                    let instr_ip = self.ip.saturating_sub(1);
                    let e = self.with_current_source_location(e, instr_ip);
                    self.cx.last_call_stack = self.build_call_stack(instr_ip);
                    if self.handle_error_in_diagnostic_mode(e.clone(), None, Some(instr_ip)) {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            };

            if observer_enabled {
                let obs = self.observer.as_deref().expect("observer_enabled must imply observer");
                obs.on_will_execute(opcode as u8, self.ip.saturating_sub(1));
            }

            let is_loop_back_edge =
                matches!(opcode, Opcode::Jmp | Opcode::Test) && self.is_backward_jump(opcode);
            let instr_ip = self.ip.saturating_sub(1);

            let execute_result =
                if trace_enabled { self.execute(opcode) } else { self.execute_inner(opcode) };

            if let Err(e) = execute_result {
                if observer_enabled {
                    let obs =
                        self.observer.as_deref().expect("observer_enabled must imply observer");
                    obs.on_error(&VmErrorInfo {
                        error_message: format!("{:?}", e),
                        opcode: Some(opcode as u8),
                        ip: instr_ip,
                        call_depth: self.call_depth(),
                    });
                }
                let e = self.with_current_source_location(e, instr_ip);
                self.cx.last_call_stack = self.build_call_stack(instr_ip);
                if self.handle_error_in_diagnostic_mode(e.clone(), Some(opcode), Some(instr_ip)) {
                    continue;
                } else {
                    return Err(e);
                }
            }

            if is_loop_back_edge {
                self.cx.hot_trace_table.profile(self.ip, opcode);
                self.try_register_hot_trace();
                self.gc_safe_point()?;
            }
        }

        Ok(self.cx.registers.first().unwrap_or(NIL))
    }

    // ========================================================================
    // Hot Trace JIT
    // ========================================================================

    fn execute_hot_trace_batch(&mut self, end_ip: usize) -> Result<(), NuzoError> {
        let mut iterations = 0u32;
        let original_chunk_ptr = self.chunk_ptr;
        let start_ip = self.ip;

        if self.tracer.is_some() {
            self.cx
                .hot_trace_table
                .events
                .push(crate::vm_hot_trace::HotTraceEvent::Hit { start_ip, end_ip });
        }

        while self.ip < end_ip && self.cx.running {
            iterations += 1;
            if iterations > MAX_BATCH_ITERATIONS {
                if self.tracer.is_some() {
                    self.cx.hot_trace_table.events.push(
                        crate::vm_hot_trace::HotTraceEvent::Abort {
                            ip: self.ip,
                            reason: "iteration_limit",
                        },
                    );
                }
                break;
            }
            if self.chunk_ptr != original_chunk_ptr || self.chunk_ptr.is_null() {
                if self.tracer.is_some() {
                    self.cx.hot_trace_table.events.push(
                        crate::vm_hot_trace::HotTraceEvent::Abort {
                            ip: self.ip,
                            reason: "chunk_switch",
                        },
                    );
                }
                break;
            }
            let chunk = unsafe { &*self.chunk_ptr };
            if self.ip >= chunk.code().len() {
                if self.tracer.is_some() {
                    self.cx.hot_trace_table.events.push(
                        crate::vm_hot_trace::HotTraceEvent::Abort {
                            ip: self.ip,
                            reason: "ip_out_of_bounds",
                        },
                    );
                }
                break;
            }
            let opcode = self.fetch_opcode()?;
            // S1 修复：fetch_opcode() 已通过 read_byte() 把 self.ip 前进 1 字节，
            // 故 self.ip 现在指向当前指令的首个操作数字节，而非 opcode。
            // instruction_size() 返回完整指令长度（opcode 1 字节 + 操作数 N 字节）。
            // 下一条 opcode 的位置 = (self.ip - 1) + instruction_size()
            //                         = self.ip + instruction_size() - 1。
            // 修复前 `self.ip + instruction_size()` 多偏移 1 字节，读到下一条指令的
            // 首个操作数字节而非 opcode 字节 → Hot Trace 融合几乎永远无法匹配，
            // 偶发误融合（操作数字节恰好落在合法 opcode 编码区间时）。
            let next_ip = self.ip + opcode.instruction_size() - 1;
            if next_ip < end_ip
                && let Some(next_byte) = chunk.code().get(next_ip)
                && let Some(next_opcode) = Chunk::decode_opcode(*next_byte)
            {
                if matches!(
                    (opcode, next_opcode),
                    (Opcode::LoadK, Opcode::Add) | (Opcode::LoadK, Opcode::Mul)
                ) {
                    use crate::vm::dispatch_table;
                    use crate::zero_unbox;
                    match next_opcode {
                        Opcode::Add => dispatch_table::_op_loadk_arith(
                            self,
                            |a, b| a + b,
                            zero_unbox::smi_add,
                            zero_unbox::generic_add_slow,
                        )?,
                        Opcode::Mul => dispatch_table::_op_loadk_arith(
                            self,
                            |a, b| a * b,
                            zero_unbox::smi_mul,
                            zero_unbox::generic_mul_slow,
                        )?,
                        _ => unreachable!(),
                    }
                    continue;
                }
                if matches!((opcode, next_opcode), (Opcode::Mov, Opcode::Add)) {
                    dispatch_table::_op_getlocal_add(self)?;
                    continue;
                }
                if opcode == Opcode::Mov
                    && matches!(next_opcode, Opcode::Sub | Opcode::Mul | Opcode::Div | Opcode::Pow)
                {
                    use crate::zero_unbox;
                    match next_opcode {
                        Opcode::Sub => dispatch_table::_op_mov_binaryop(
                            self,
                            |a, b| a - b,
                            zero_unbox::smi_sub,
                            zero_unbox::generic_sub_slow,
                            false,
                        )?,
                        Opcode::Mul => dispatch_table::_op_mov_binaryop(
                            self,
                            |a, b| a * b,
                            zero_unbox::smi_mul,
                            zero_unbox::generic_mul_slow,
                            false,
                        )?,
                        Opcode::Div => dispatch_table::_op_mov_binaryop(
                            self,
                            |a, b| a / b,
                            zero_unbox::smi_div,
                            zero_unbox::generic_div_slow,
                            true,
                        )?,
                        Opcode::Pow => dispatch_table::_op_mov_binaryop(
                            self,
                            |a, b| a.powf(b),
                            zero_unbox::smi_pow,
                            zero_unbox::generic_pow_slow,
                            true,
                        )?,
                        _ => unreachable!(),
                    }
                    continue;
                }
            }
            self.execute(opcode)?;
        }
        Ok(())
    }
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "vm_tests.rs"]
mod tests;
