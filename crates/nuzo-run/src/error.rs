//! Unified error types for nuzo_run.

pub use nuzo_core::NuzoError;
pub use nuzo_core::NuzoErrorKind;

pub type NuzoResult<T> = Result<T, NuzoError>;

/// Create an internal error with a message string.
pub fn internal_err(msg: impl Into<String>) -> NuzoError {
    NuzoError::internal(nuzo_core::InternalError::CompilerBug { message: msg.into() }, None)
}

/// Wrap an IO error as NuzoError.
///
/// IO 错误属于"运行时环境故障"（文件读写、stdin、网络等），
/// 不应归类为 `CompilerBug`（那是编译器内部不变量破坏）。
/// 使用 `InternalError::IoError` 变体保留错误类别语义，
/// 让上层能根据错误类型选择合适的处理策略（重试/降级/报错退出）。
#[cfg(not(target_arch = "wasm32"))]
pub fn io_err(err: std::io::Error) -> NuzoError {
    NuzoError::internal(nuzo_core::InternalError::IoError { message: err.to_string() }, None)
}

/// 将 [`ResolveError`](nuzo_ir::module_resolver::ResolveError) 转换为 [`NuzoError`]。
///
/// 供 `Engine::run_file` / `Engine::compile_file` 等直接调用
/// `ModuleResolver::load_source` 的入口使用 `.map_err(resolve_err)?` 传播错误。
///
/// 语义上归类为 IO 错误（模块加载失败属运行时环境故障），
/// 保留可读消息便于诊断；位置信息丢失（如需精确位置应通过 IR 构建路径）。
///
/// 注意：不使用 `From` impl 是因为 orphan rule —— `NuzoError` 与 `ResolveError`
/// 均非本 crate 定义，无法在 `nuzo_run` 中为它们实现 `From`。
pub fn resolve_err(e: nuzo_ir::module_resolver::ResolveError) -> NuzoError {
    use nuzo_ir::module_resolver::ResolveError;
    let message = match &e {
        ResolveError::ModuleNotFound { path, .. } => {
            format!("Module not found: {}", path)
        }
        ResolveError::CircularImport { chain, .. } => {
            let chain_str =
                chain.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" -> ");
            format!("Circular import detected: {}", chain_str)
        }
        ResolveError::DuplicateSymbol { name, .. } => {
            format!("Duplicate symbol: {}", name)
        }
        ResolveError::IoError { path, message, .. } => {
            format!("IO error loading module {}: {}", path, message)
        }
        ResolveError::DepthExceeded { depth, max_depth, .. } => {
            format!("Import depth exceeded: {}/{}", depth, max_depth)
        }
    };
    NuzoError::internal(nuzo_core::InternalError::IoError { message }, None)
}
