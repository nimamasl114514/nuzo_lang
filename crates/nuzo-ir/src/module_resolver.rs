//! # 模块路径解析器 trait
//!
//! 本模块定义 [`ModuleResolver`] trait，用于抽象 import 路径解析与模块源码加载。
//!
//! ## 设计动机
//!
//! [`crate::builder::IrBuilder`] 在处理 import 语句时需要解析模块路径并加载源码。
//! 然而真正的解析逻辑（文件系统访问、搜索路径、缓存等）属于运行时职责，
//! 由 `nuzo_run::Engine` 实现，再注入到 IrBuilder。
//!
//! 这样可以避免**反向依赖**：`nuzo_ir` 不能依赖 `nuzo_run`（位于更高层级）。
//!
//! ## 错误模型
//!
//! 所有解析失败统一返回 [`ResolveError`]，携带 [`SourceLocation`] 用于错误报告。
//! - [`ResolveError::ModuleNotFound`][]: 模块路径不存在
//! - [`ResolveError::CircularImport`][]: 检测到循环依赖
//! - [`ResolveError::DuplicateSymbol`][]: 同名符号重复定义
//! - [`ResolveError::IoError`]: 文件 I/O 错误
//! - [`ResolveError::DepthExceeded`]: import 嵌套深度超限

use std::path::{Path, PathBuf};

use nuzo_core::SourceLocation;

/// 模块路径解析器 trait
///
/// 由 `nuzo_run::Engine` 实现，注入到 [`crate::builder::IrBuilder`]，
/// 避免 `nuzo_ir` 反向依赖 `nuzo_run`。
///
/// # 实现约定
/// - 实现应为**幂等**：相同输入应产生相同输出
/// - 实现应**线程安全**（`Send + Sync`），以便在并发构建场景下使用
/// - `check_circular` 应使用 DFS 灰白标记法检测环
pub trait ModuleResolver {
    /// 解析 import 路径 → 规范化绝对路径
    ///
    /// # 参数
    /// - `current`: 当前模块路径（用于相对路径解析基准；`None` 表示顶层入口模块）
    /// - `import_path`: import 语句中的字面量路径
    ///
    /// # 返回
    /// 成功时返回规范化后的绝对路径；失败返回 [`ResolveError::ModuleNotFound`]。
    fn resolve(&self, current: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError>;

    /// 加载已解析的模块源码
    ///
    /// # 参数
    /// - `path`: 已通过 [`resolve`](Self::resolve) 得到的绝对路径
    ///
    /// # 返回
    /// 成功时返回模块源码文本；失败返回 [`ResolveError::IoError`] 或 [`ResolveError::ModuleNotFound`]。
    fn load_source(&self, path: &Path) -> Result<String, ResolveError>;

    /// 检查循环依赖（DFS 灰白标记）
    ///
    /// # 参数
    /// - `path`: 即将加载的模块路径
    /// - `stack`: 当前正在加载的模块栈（从根到当前模块的路径序列）
    ///
    /// # 返回
    /// 检测到环时返回 [`ResolveError::CircularImport`]，否则返回 `Ok(())`。
    fn check_circular(&self, path: &Path, stack: &[PathBuf]) -> Result<(), ResolveError>;
}

/// 模块解析错误
///
/// 所有变体均携带 [`SourceLocation`] 以便在错误报告中精确定位。
#[derive(Debug, Clone)]
pub enum ResolveError {
    /// 模块未找到
    ModuleNotFound {
        /// 未找到的模块路径（用户书写的形式）
        path: String,
        /// 触发错误的源码位置
        location: SourceLocation,
    },
    /// 循环 import 检测到
    CircularImport {
        /// 形成环的路径链（按 import 顺序，最后一项指向链中已存在的项）
        chain: Vec<PathBuf>,
        /// 触发错误的源码位置
        location: SourceLocation,
    },
    /// 同名符号重复定义
    DuplicateSymbol {
        /// 重复的符号名
        name: String,
        /// 第一次定义的位置
        first_location: SourceLocation,
        /// 第二次定义的位置
        second_location: SourceLocation,
    },
    /// 文件 I/O 错误
    IoError {
        /// 出错的文件路径
        path: String,
        /// I/O 错误信息
        message: String,
        /// 触发错误的源码位置
        location: SourceLocation,
    },
    /// import 嵌套深度超限
    DepthExceeded {
        /// 当前嵌套深度
        depth: usize,
        /// 允许的最大深度
        max_depth: usize,
        /// 触发错误的源码位置
        location: SourceLocation,
    },
}

/// 默认 Null resolver，用于不使用 import 的场景
///
/// 所有解析方法均返回 [`ResolveError::ModuleNotFound`]，
/// `check_circular` 始终返回 `Ok(())`。
///
/// 适用于：
/// - 单文件脚本（无 import）
/// - 测试场景
/// - REPL 交互模式
pub struct NullResolver;

impl ModuleResolver for NullResolver {
    fn resolve(&self, _: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError> {
        Err(ResolveError::ModuleNotFound {
            path: import_path.to_string(),
            location: SourceLocation::default(),
        })
    }

    fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
        Err(ResolveError::ModuleNotFound {
            path: path.display().to_string(),
            location: SourceLocation::default(),
        })
    }

    fn check_circular(&self, _: &Path, _: &[PathBuf]) -> Result<(), ResolveError> {
        Ok(())
    }
}

// ============================================================================
// StandardResolver — 基于文件系统的标准模块解析器
// ============================================================================

/// 标准模块解析器
///
/// 基于文件系统的 import 路径解析器，支持两种语法：
///
/// 1. **模块名语法**：import_path 不含路径分隔符（`/`、`\`）且不含 `.`，
///    如 `math`。在 [`std_path`](Self::std_path) 下查找 `<name>.nuzo`。
///    若 `std_path` 未配置或文件不存在，返回 [`ResolveError::ModuleNotFound`]。
///
/// 2. **路径语法**：import_path 含分隔符或 `.`，如 `utils.nuzo`、`sub/mod.nuzo`
///    或绝对路径。相对于「当前模块所在目录」解析（`current` 为 `None` 时使用
///    进程工作目录），无扩展名时自动追加 `.nuzo`，最后 `canonicalize` 规范化。
///
/// 与 [`NullResolver`] 的区别：`StandardResolver` 真正执行文件系统访问，
/// 适用于测试及不需要完整运行时引擎的场景。
///
/// # 解析算法
///
/// 1. import_path 为空 → [`ResolveError::ModuleNotFound`]
/// 2. 裸模块名（无 `/`、`\`、`.`）→ `std_path/<name>.nuzo`（未配置 std_path → NotFound）
/// 3. 绝对路径 → 直接使用
/// 4. 相对路径 → `base_dir.join(import_path)`，base_dir = `current.parent()` 或 cwd
/// 5. 候选无扩展名 → 追加 `.nuzo`
/// 6. `canonicalize`（要求文件存在）→ 成功返回绝对路径；失败 → [`ResolveError::ModuleNotFound`]
pub struct StandardResolver {
    /// 标准库根目录；模块名语法的查找基准。
    /// `None` 时裸模块名一律返回 [`ResolveError::ModuleNotFound`]。
    pub std_path: Option<PathBuf>,
}

impl StandardResolver {
    /// 创建解析器。`std_path` 为 `None` 时裸模块名语法返回 [`ResolveError::ModuleNotFound`]。
    pub fn new(std_path: Option<PathBuf>) -> Self {
        Self { std_path }
    }

    /// 判断 import_path 是否为「裸模块名」：
    /// 不含路径分隔符（`/` 或 `\`）且不含 `.`（扩展名分隔符）。
    fn is_module_name(import_path: &str) -> bool {
        !import_path.contains('/') && !import_path.contains('\\') && !import_path.contains('.')
    }

    /// 计算相对路径基准目录：当前模块所在目录，或进程工作目录。
    fn base_dir(current: Option<&Path>) -> PathBuf {
        match current {
            Some(cur) => cur.parent().map(Path::to_path_buf).unwrap_or_default(),
            None => std::env::current_dir().unwrap_or_default(),
        }
    }

    /// 若候选路径无扩展名则追加 `.nuzo`，否则原样返回。
    fn ensure_nuzo_ext(mut candidate: PathBuf) -> PathBuf {
        if candidate.extension().is_none() {
            candidate.set_extension("nuzo");
        }
        candidate
    }

    /// 构造 [`ResolveError::ModuleNotFound`]，携带原始 import_path。
    fn not_found(import_path: &str) -> ResolveError {
        ResolveError::ModuleNotFound {
            path: import_path.to_string(),
            location: SourceLocation::default(),
        }
    }

    /// canonicalize 候选路径；失败返回 [`ResolveError::ModuleNotFound`]。
    fn canonicalize_or_not_found(
        import_path: &str,
        candidate: &Path,
    ) -> Result<PathBuf, ResolveError> {
        std::fs::canonicalize(candidate).map_err(|_| Self::not_found(import_path))
    }
}

impl Default for StandardResolver {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ModuleResolver for StandardResolver {
    fn resolve(&self, current: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError> {
        // 1. 空路径 → ModuleNotFound
        if import_path.is_empty() {
            return Err(Self::not_found(import_path));
        }

        // 2. 裸模块名 → std_path/<name>.nuzo
        if Self::is_module_name(import_path) {
            let std_path = self.std_path.as_ref().ok_or_else(|| Self::not_found(import_path))?;
            let candidate = Self::ensure_nuzo_ext(std_path.join(import_path));
            return Self::canonicalize_or_not_found(import_path, &candidate);
        }

        // 3-5. 路径语法：绝对/相对解析 + 自动扩展名
        let p = Path::new(import_path);
        let candidate =
            if p.is_absolute() { p.to_path_buf() } else { Self::base_dir(current).join(p) };
        let candidate = Self::ensure_nuzo_ext(candidate);

        // 6. canonicalize（要求文件存在）
        Self::canonicalize_or_not_found(import_path, &candidate)
    }

    fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
        std::fs::read_to_string(path).map_err(|e| ResolveError::IoError {
            path: path.display().to_string(),
            message: e.to_string(),
            location: SourceLocation::default(),
        })
    }

    fn check_circular(&self, path: &Path, stack: &[PathBuf]) -> Result<(), ResolveError> {
        // DFS 灰白标记：若 path 已在 import 栈中，则构成环。
        // 链按 import 顺序排列，最后一项指向链中已存在的项（与 EngineInner 实现一致）。
        if stack.iter().any(|p| p == path) {
            let mut chain: Vec<PathBuf> = stack.to_vec();
            chain.push(path.to_path_buf());
            Err(ResolveError::CircularImport { chain, location: SourceLocation::default() })
        } else {
            Ok(())
        }
    }
}

// ============================================================================
// MemoryResolver — 基于内存模块表(wasm32 / 测试场景)
// ============================================================================

/// 内存模块表解析器
///
/// 用于不依赖文件系统的场景(如 wasm32 Playground、单元测试)。
/// 模块源码在初始化时通过 [`add_module`](Self::add_module) 注入,
/// 解析时直接从内存表查找,不执行任何文件系统访问。
///
/// # 解析算法
///
/// 1. import_path 为空 → [`ResolveError::ModuleNotFound`]
/// 2. 内存表查找 import_path(原样 key)
/// 3. 命中 → 返回 `PathBuf::from(import_path)`(虚拟路径,用于后续 `load_source` 查表)
/// 4. 未命中 → [`ResolveError::ModuleNotFound`]
///
/// # 循环检测
///
/// 与 [`StandardResolver`] 共享 DFS 灰白标记算法。
pub struct MemoryResolver {
    /// 模块表:虚拟路径 → 源码
    modules: std::collections::HashMap<String, String>,
}

impl MemoryResolver {
    /// 创建空的内存解析器。
    pub fn new() -> Self {
        Self { modules: std::collections::HashMap::new() }
    }

    /// 注入模块源码。
    ///
    /// `path` 作为虚拟路径 key,可使用任意字符串(如 `"math"`、`"utils.nuzo"`)。
    /// `source` 为模块的 Nuzo 源码。
    pub fn add_module(&mut self, path: impl Into<String>, source: impl Into<String>) {
        self.modules.insert(path.into(), source.into());
    }

    /// 查询当前注入的模块数量(主要用于测试)。
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

impl Default for MemoryResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleResolver for MemoryResolver {
    fn resolve(&self, _current: Option<&Path>, import_path: &str) -> Result<PathBuf, ResolveError> {
        if import_path.is_empty() {
            return Err(ResolveError::ModuleNotFound {
                path: import_path.to_string(),
                location: SourceLocation::default(),
            });
        }
        if self.modules.contains_key(import_path) {
            Ok(PathBuf::from(import_path))
        } else {
            Err(ResolveError::ModuleNotFound {
                path: import_path.to_string(),
                location: SourceLocation::default(),
            })
        }
    }

    fn load_source(&self, path: &Path) -> Result<String, ResolveError> {
        let key = path.to_string_lossy().into_owned();
        self.modules.get(&key).cloned().ok_or_else(|| ResolveError::ModuleNotFound {
            path: key,
            location: SourceLocation::default(),
        })
    }

    fn check_circular(&self, path: &Path, stack: &[PathBuf]) -> Result<(), ResolveError> {
        // 与 StandardResolver 相同的 DFS 灰白标记算法
        if stack.iter().any(|p| p == path) {
            let mut chain: Vec<PathBuf> = stack.to_vec();
            chain.push(path.to_path_buf());
            Err(ResolveError::CircularImport { chain, location: SourceLocation::default() })
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 创建唯一临时目录，返回其路径（调用方负责清理）。
    /// 用 prefix 区分不同测试，避免并行执行时纳秒时间戳碰撞。
    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before UNIX_EPOCH")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nuzo_ir_test_{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    /// canonicalize 用于断言比较（跨平台一致）。
    fn canon(path: impl AsRef<Path>) -> PathBuf {
        std::fs::canonicalize(path.as_ref()).expect("canonicalize for assertion")
    }

    // --- 模块名语法（import math）---

    #[test]
    fn test_module_name_resolves_to_std_path() {
        // import math → std_path/math.nuzo（文件存在）
        let std_dir = make_temp_dir("std_ok");
        let math_file = std_dir.join("math.nuzo");
        fs::write(&math_file, "// math module\n").unwrap();

        let resolver = StandardResolver::new(Some(std_dir.clone()));
        let resolved = resolver.resolve(None, "math").expect("bare module name should resolve");

        assert_eq!(resolved, canon(&math_file));

        let _ = fs::remove_dir_all(&std_dir);
    }

    #[test]
    fn test_module_name_not_found_when_std_path_unset() {
        // std_path 未配置 → 裸模块名一律 ModuleNotFound
        let resolver = StandardResolver::new(None);
        let err = resolver.resolve(None, "math").unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { ref path, .. } if path == "math"),
            "expected ModuleNotFound for \"math\", got {err:?}"
        );
    }

    #[test]
    fn test_module_name_not_found_when_file_missing() {
        // std_path 已配置但 <name>.nuzo 不存在 → ModuleNotFound
        let std_dir = make_temp_dir("std_missing");
        let resolver = StandardResolver::new(Some(std_dir.clone()));
        let err = resolver.resolve(None, "ghost").unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { ref path, .. } if path == "ghost"),
            "expected ModuleNotFound for \"ghost\", got {err:?}"
        );

        let _ = fs::remove_dir_all(&std_dir);
    }

    // --- 路径语法（import "utils.nuzo" / "sub/mod"）---

    #[test]
    fn test_relative_path_resolves_against_current() {
        // import "utils.nuzo" → current 所在目录/utils.nuzo
        let dir = make_temp_dir("rel");
        let main_file = dir.join("main.nuzo");
        fs::write(&main_file, "// main\n").unwrap();
        let utils_file = dir.join("utils.nuzo");
        fs::write(&utils_file, "// utils\n").unwrap();

        let resolver = StandardResolver::new(None);
        let resolved =
            resolver.resolve(Some(&main_file), "utils.nuzo").expect("relative path should resolve");

        assert_eq!(resolved, canon(&utils_file));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_relative_subdir_path_resolves() {
        // import "sub/mod.nuzo" → current 所在目录/sub/mod.nuzo
        let dir = make_temp_dir("relsub");
        let main_file = dir.join("main.nuzo");
        fs::write(&main_file, "// main\n").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        let mod_file = dir.join("sub").join("mod.nuzo");
        fs::write(&mod_file, "// mod\n").unwrap();

        let resolver = StandardResolver::new(None);
        let resolved =
            resolver.resolve(Some(&main_file), "sub/mod.nuzo").expect("subdir path should resolve");

        assert_eq!(resolved, canon(&mod_file));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_path_without_extension_auto_appends_nuzo() {
        // import "sub/mod"（无扩展名、含分隔符 → 路径语法）→ 自动追加 .nuzo
        let dir = make_temp_dir("autoext");
        let main_file = dir.join("main.nuzo");
        fs::write(&main_file, "// main\n").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        let mod_file = dir.join("sub").join("mod.nuzo");
        fs::write(&mod_file, "// mod\n").unwrap();

        let resolver = StandardResolver::new(None);
        let resolved =
            resolver.resolve(Some(&main_file), "sub/mod").expect("auto-ext path should resolve");

        assert_eq!(resolved, canon(&mod_file));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dotted_path_uses_path_branch_not_module_name() {
        // "utils.nuzo" 含 . → 路径语法分支，即使 std_path=None 也能相对解析
        let dir = make_temp_dir("dot");
        let main_file = dir.join("main.nuzo");
        fs::write(&main_file, "// main\n").unwrap();
        let target = dir.join("utils.nuzo");
        fs::write(&target, "// utils\n").unwrap();

        let resolver = StandardResolver::new(None);
        let resolved = resolver
            .resolve(Some(&main_file), "utils.nuzo")
            .expect("dotted path should resolve via path branch");

        assert_eq!(resolved, canon(&target));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_relative_path_not_found() {
        // 路径语法但文件不存在 → ModuleNotFound
        let dir = make_temp_dir("relmissing");
        let main_file = dir.join("main.nuzo");
        fs::write(&main_file, "// main\n").unwrap();

        let resolver = StandardResolver::new(None);
        let err = resolver.resolve(Some(&main_file), "nonexistent.nuzo").unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { .. }),
            "expected ModuleNotFound, got {err:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_empty_path_returns_not_found() {
        let resolver = StandardResolver::new(None);
        let err = resolver.resolve(None, "").unwrap_err();
        assert!(matches!(err, ResolveError::ModuleNotFound { .. }));
    }

    // --- load_source ---

    #[test]
    fn test_load_source_reads_file() {
        let dir = make_temp_dir("loadsrc");
        let file = dir.join("mod.nuzo");
        fs::write(&file, "fn add(a, b) = a + b\n").unwrap();

        let resolver = StandardResolver::new(None);
        let src = resolver.load_source(&file).expect("load source");
        assert_eq!(src, "fn add(a, b) = a + b\n");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_source_missing_returns_io_error() {
        let resolver = StandardResolver::new(None);
        let err = resolver.load_source(Path::new("/nonexistent/does/not/exist.nuzo")).unwrap_err();
        assert!(matches!(err, ResolveError::IoError { .. }), "got {err:?}");
    }

    // --- check_circular ---

    #[test]
    fn test_check_circular_detects_cycle() {
        let resolver = StandardResolver::new(None);
        let a = PathBuf::from("/fake/a.nuzo");
        let b = PathBuf::from("/fake/b.nuzo");
        let stack = vec![a.clone(), b.clone()];

        // 再次导入 a → 构成环 a → b → a
        let err = resolver.check_circular(&a, &stack).unwrap_err();
        match err {
            ResolveError::CircularImport { chain, .. } => {
                assert_eq!(chain, vec![a.clone(), b.clone(), a]);
            }
            other => panic!("expected CircularImport, got {other:?}"),
        }
    }

    #[test]
    fn test_check_circular_allows_new_module() {
        let resolver = StandardResolver::new(None);
        let a = PathBuf::from("/fake/a.nuzo");
        let b = PathBuf::from("/fake/b.nuzo");
        let stack = vec![a];

        // 导入 b（不在栈中）→ Ok
        assert!(resolver.check_circular(&b, &stack).is_ok());
    }

    // --- MemoryResolver (wasm32 / 内存场景) ---

    #[test]
    fn test_memory_resolver_lookup() {
        let mut resolver = MemoryResolver::new();
        resolver.add_module("math", "// math module\n");

        let resolved = resolver.resolve(None, "math").expect("injected module should resolve");
        assert_eq!(resolved, PathBuf::from("math"));

        let src = resolver.load_source(&PathBuf::from("math")).expect("load_source");
        assert_eq!(src, "// math module\n");
    }

    #[test]
    fn test_memory_resolver_missing() {
        let resolver = MemoryResolver::new();
        let err = resolver.resolve(None, "ghost").unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { ref path, .. } if path == "ghost"),
            "expected ModuleNotFound, got {err:?}"
        );
    }

    #[test]
    fn test_memory_resolver_empty_path() {
        let resolver = MemoryResolver::new();
        let err = resolver.resolve(None, "").unwrap_err();
        assert!(matches!(err, ResolveError::ModuleNotFound { .. }));
    }

    #[test]
    fn test_memory_resolver_load_source_missing() {
        let resolver = MemoryResolver::new();
        let err = resolver.load_source(&PathBuf::from("never_injected")).unwrap_err();
        assert!(
            matches!(err, ResolveError::ModuleNotFound { ref path, .. } if path == "never_injected"),
            "expected ModuleNotFound, got {err:?}"
        );
    }

    #[test]
    fn test_memory_resolver_check_circular() {
        let resolver = MemoryResolver::new();
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        let stack = vec![a.clone(), b.clone()];

        // 再次导入 a → 构成环
        let err = resolver.check_circular(&a, &stack).unwrap_err();
        match err {
            ResolveError::CircularImport { chain, .. } => {
                assert_eq!(chain, vec![a.clone(), b.clone(), a]);
            }
            other => panic!("expected CircularImport, got {other:?}"),
        }
    }

    #[test]
    fn test_memory_resolver_check_circular_allows_new() {
        let resolver = MemoryResolver::new();
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        let stack = vec![a];
        assert!(resolver.check_circular(&b, &stack).is_ok());
    }

    #[test]
    fn test_memory_resolver_len_and_is_empty() {
        let mut resolver = MemoryResolver::new();
        assert!(resolver.is_empty());
        assert_eq!(resolver.len(), 0);

        resolver.add_module("a", "src_a");
        resolver.add_module("b", "src_b");
        assert_eq!(resolver.len(), 2);
        assert!(!resolver.is_empty());
    }

    #[test]
    fn test_memory_resolver_default_is_empty() {
        let resolver = MemoryResolver::default();
        assert!(resolver.is_empty());
    }

    #[test]
    fn test_memory_resolver_overwrite_module() {
        let mut resolver = MemoryResolver::new();
        resolver.add_module("foo", "v1");
        resolver.add_module("foo", "v2"); // 覆盖
        assert_eq!(resolver.len(), 1);
        let src = resolver.load_source(&PathBuf::from("foo")).unwrap();
        assert_eq!(src, "v2");
    }
}
