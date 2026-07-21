//! nuzo_run binary — single entry point CLI.

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use nuzo_core::{LangMode, SourceLocation};
use nuzo_error::{DiagnosticRenderer, StackFrameInfo};
use nuzo_run::print_test_summary;
use nuzo_run::{BenchMode, Engine, NuzoError, OutputSink};

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// CLI definition (clap derive)
// ---------------------------------------------------------------------------

/// 解析 `--lang` 选项的值：`zh` | `en` | `both`（不区分大小写）。
fn parse_lang_mode(input: &str) -> Result<LangMode, String> {
    match input.to_lowercase().as_str() {
        "zh" => Ok(LangMode::Zh),
        "en" => Ok(LangMode::En),
        "both" => Ok(LangMode::Both),
        other => Err(format!(
            "invalid value '{}' for '--lang <LANG>': expected one of zh|en|both",
            other
        )),
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "nuzo",
    version = VERSION,
    disable_version_flag = true,
    disable_help_flag = true,
    about = "Nuzo programming language",
    long_about = None
)]
struct Cli {
    #[arg(short = 'h', long = "help", help = "Show this help")]
    help: bool,

    #[arg(short = 'v', long = "version", help = "Show version")]
    version: bool,

    #[command(subcommand)]
    command: Option<CliCommand>,

    #[arg(short = 'e', long = "eval", help = "Evaluate inline code")]
    eval: Option<String>,

    #[arg(long = "trace", help = "Enable execution tracing", global = true)]
    trace: bool,

    #[arg(short = 'V', long = "verbose", help = "Verbose output", global = true)]
    verbose: bool,

    #[arg(long = "std-path", help = "Path to standard library", global = true)]
    std_path: Option<PathBuf>,

    #[arg(short = 'k', long = "filter", help = "Filter tests by name/path", global = true)]
    filter: Option<String>,

    #[arg(short = 't', long = "timeout", help = "Per-test timeout in ms", global = true)]
    timeout: Option<u64>,

    /// Error message / fix-suggestion language. Overrides the `NUZO_LANG` env var.
    /// Accepted values: `zh` | `en` | `both`.
    #[arg(
        long = "lang",
        value_name = "LANG",
        value_parser = parse_lang_mode,
        help = "Error message language: zh|en|both (overrides NUZO_LANG)",
        global = true
    )]
    lang: Option<LangMode>,

    #[arg(help = "Script file to run")]
    file: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone, PartialEq)]
enum CliCommand {
    /// Run a script file
    Run { file: PathBuf },
    /// Compile and show bytecode
    Compile {
        file: PathBuf,
        /// Output bytecode disassembly (human-readable)
        #[arg(long = "disassemble")]
        disassemble: bool,
    },
    /// Check syntax / compile without running
    Check { file: PathBuf },
    /// Benchmark script execution
    Bench { file: PathBuf },
    /// Start interactive REPL
    Repl,
    /// Auto-discover and run *.nuzo tests
    Test {
        #[arg(help = "Directories or files to scan")]
        dirs: Vec<PathBuf>,
    },
    /// Run end-to-end tests
    E2e {
        #[arg(help = "Directories or files to scan")]
        dirs: Vec<PathBuf>,
    },
}

// ---------------------------------------------------------------------------
// main / run
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();
    let lang = cli.lang;
    let exit_code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if let Some(nuzo_err) = e.downcast_ref::<NuzoError>() {
                render_nuzo_error(nuzo_err, &[], lang);
            } else {
                eprintln!("nuzo: {}", e);
            }
            1
        }
    };
    process::exit(exit_code);
}

/// 根据可选的语言覆盖构造渲染器：
/// - `None` → 使用 `NUZO_LANG` 环境变量（默认行为）
/// - `Some(lang)` → 显式覆盖
fn diagnostic_renderer(lang: Option<LangMode>) -> DiagnosticRenderer {
    let renderer = DiagnosticRenderer::new();
    match lang {
        Some(l) => renderer.with_lang(l),
        None => renderer,
    }
}

fn render_nuzo_error(err: &NuzoError, stack: &[StackFrameInfo], lang: Option<LangMode>) {
    let renderer = diagnostic_renderer(lang);
    eprintln!("{}", renderer.render_nuzo_error(err, stack));
}

fn render_compile_error(err: &NuzoError, file: &str, source: &str, lang: Option<LangMode>) {
    let (line, column) =
        err.source_location.as_ref().map(|loc| (loc.line, loc.column)).unwrap_or((0, 0));
    let source_line = if line > 0 {
        source.lines().nth(line.saturating_sub(1)).map(|s| s.to_string())
    } else {
        None
    };
    let loc =
        SourceLocation { file: file.to_string(), line, column, source_line, function_name: None };
    // 注入完整源码以启用多行上下文 snippet；空源码退化为单行渲染
    let mut renderer = diagnostic_renderer(lang);
    if !source.is_empty() {
        renderer = renderer.with_source_context(source);
    }
    eprintln!("{}", renderer.render_compile_error(err, loc));
}

fn run(cli: Cli) -> Result<i32, Box<dyn std::error::Error>> {
    // --help / --version handled first (they short-circuit before engine setup)
    if cli.help {
        print_help();
        return Ok(0);
    }
    if cli.version {
        println!("nuzo v{}", VERSION);
        return Ok(0);
    }

    // Validate incompatible combinations
    if cli.eval.is_some() && cli.command.is_some() {
        return Err("cannot use -e/--eval with a subcommand".into());
    }
    if cli.eval.is_some() && cli.file.is_some() {
        return Err("eval does not take files".into());
    }

    let mut builder = Engine::builder().with_default_config();
    if cli.trace {
        builder = builder.trace();
    }
    if let Some(std_path) = cli.std_path {
        builder = builder.with_std_path(std_path);
    }
    let engine = builder.build()?;
    let lang = cli.lang;

    match cli.command {
        Some(CliCommand::Run { file }) => cmd_run_file(&engine, &file, lang),
        Some(CliCommand::Compile { file, disassemble }) => {
            cmd_compile(&engine, &file, disassemble, lang)
        }
        Some(CliCommand::Check { file }) => cmd_check(&engine, &file, lang),
        Some(CliCommand::Bench { file }) => cmd_bench(&engine, &file),
        Some(CliCommand::Repl) => cmd_repl(&engine, lang),
        Some(CliCommand::Test { dirs }) => {
            cmd_test(&engine, &dirs, cli.filter.as_deref(), cli.timeout, cli.verbose)
        }
        Some(CliCommand::E2e { dirs }) => {
            cmd_e2e(&engine, &dirs, cli.filter.as_deref(), cli.timeout, cli.verbose)
        }
        None => {
            if let Some(code) = cli.eval {
                cmd_eval(&engine, &code, lang)
            } else {
                match cli.file {
                    Some(path) => cmd_run_file(&engine, &path, lang),
                    None => cmd_repl(&engine, lang),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// command handlers
// ---------------------------------------------------------------------------

fn is_nil(v: &nuzo_run::Value) -> bool {
    v.is_nil()
}

fn cmd_eval(
    engine: &Engine,
    code: &str,
    lang: Option<LangMode>,
) -> Result<i32, Box<dyn std::error::Error>> {
    let (sink, _buf) = OutputSink::new_capture();
    let mut session = engine.new_session_with(sink);
    match session.eval(code) {
        Ok(out) => {
            for line in &out.stdout {
                print!("{}", line);
            }
            if !is_nil(&out.value) {
                println!("{}", out.value);
            }
            Ok(0)
        }
        Err(e) => {
            let stack = session.vm_mut().last_call_stack().to_vec();
            render_nuzo_error(&e, &stack, lang);
            Ok(1)
        }
    }
}

fn cmd_run_file(
    engine: &Engine,
    path: &std::path::Path,
    lang: Option<LangMode>,
) -> Result<i32, Box<dyn std::error::Error>> {
    match engine.run_file(path) {
        Ok(out) => {
            for line in &out.stdout {
                print!("{}", line);
            }
            if !is_nil(&out.value) {
                println!("{}", out.value);
            }
            Ok(0)
        }
        Err(e) => {
            let stack: Vec<StackFrameInfo> = Vec::new();
            render_nuzo_error(&e, &stack, lang);
            Ok(1)
        }
    }
}

fn cmd_compile(
    engine: &Engine,
    path: &std::path::Path,
    disassemble: bool,
    lang: Option<LangMode>,
) -> Result<i32, Box<dyn std::error::Error>> {
    match engine.compile_file(path) {
        Ok(chunk) => {
            if disassemble {
                print!("{}", chunk.disassemble());
            } else {
                println!(
                    "=== Chunk: {} ({} bytes, {} constants) ===",
                    path.display(),
                    chunk.code().len(),
                    chunk.constants().len()
                );
                for (i, byte) in chunk.code().iter().enumerate() {
                    print!("{:02x} ", byte);
                    if (i + 1) % 16 == 0 {
                        println!();
                    }
                }
                println!();
                println!("=== Constants ===");
                for (i, c) in chunk.constants().iter().enumerate() {
                    println!("{:4}: {:?}", i, c);
                }
            }
            Ok(0)
        }
        Err(e) => {
            // 读取完整源码以启用多行上下文 snippet
            let source = std::fs::read_to_string(path).unwrap_or_default();
            render_compile_error(&e, &path.display().to_string(), &source, lang);
            Ok(1)
        }
    }
}

fn cmd_check(
    engine: &Engine,
    path: &std::path::Path,
    lang: Option<LangMode>,
) -> Result<i32, Box<dyn std::error::Error>> {
    match engine.compile_file(path) {
        Ok(_) => {
            println!("✓ {}", path.display());
            Ok(0)
        }
        Err(e) => {
            let source = std::fs::read_to_string(path).unwrap_or_default();
            render_compile_error(&e, &path.display().to_string(), &source, lang);
            Ok(1)
        }
    }
}

fn cmd_bench(engine: &Engine, path: &std::path::Path) -> Result<i32, Box<dyn std::error::Error>> {
    let source = std::fs::read_to_string(path)?;
    let bench_name = format!("bench:{}", path.display());
    for mode in [BenchMode::CompileOnly, BenchMode::ExecuteOnly, BenchMode::EndToEnd] {
        let result =
            engine.bench().warmup(3).iterations(50).run_script_mode(&bench_name, &source, mode)?;
        println!("{}", result.format());
    }
    Ok(0)
}

fn cmd_test(
    engine: &Engine,
    paths: &[PathBuf],
    filter: Option<&str>,
    timeout_ms: Option<u64>,
    verbose: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
    let dirs: Vec<PathBuf> = if paths.is_empty() {
        let candidates = ["tests/e2e", "tests", "test", "."];
        let found = candidates
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_dir())
            .unwrap_or_else(|| PathBuf::from("."));
        vec![found]
    } else {
        paths.to_vec()
    };

    run_test_harness(engine, &dirs, filter, timeout_ms, verbose, "tests")
}

fn cmd_e2e(
    engine: &Engine,
    paths: &[PathBuf],
    filter: Option<&str>,
    timeout_ms: Option<u64>,
    verbose: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
    let dirs: Vec<PathBuf> = if paths.is_empty() {
        let mut defaults = Vec::new();
        for p in ["tests/e2e", "examples/nuzo"] {
            let pb = PathBuf::from(p);
            if pb.is_dir() {
                defaults.push(pb);
            }
        }
        if defaults.is_empty() {
            defaults.push(PathBuf::from("."));
        }
        defaults
    } else {
        paths.to_vec()
    };

    run_test_harness(engine, &dirs, filter, timeout_ms, verbose, "e2e tests")
}

fn run_test_harness(
    engine: &Engine,
    dirs: &[PathBuf],
    filter: Option<&str>,
    timeout_ms: Option<u64>,
    verbose: bool,
    label: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let mut harness = engine.test().verbose(verbose);
    if let Some(ms) = timeout_ms {
        harness = harness.timeout(ms);
    }
    if let Some(pat) = filter {
        harness = harness.filter(pat);
    }

    let dir_strs: Vec<String> = dirs.iter().map(|p| p.display().to_string()).collect();
    println!("Running {} in: {}", label, dir_strs.join(", "));

    let mut all_passed = true;
    for dir in dirs {
        let summary = if dir.is_file() {
            harness.run_files(std::slice::from_ref(dir))?
        } else {
            harness.run_dir(dir)?
        };
        print_test_summary(&summary);
        if summary.failed() > 0 || summary.timeouts() > 0 {
            all_passed = false;
        }
    }

    Ok(if all_passed { 0 } else { 1 })
}

fn cmd_repl(engine: &Engine, lang: Option<LangMode>) -> Result<i32, Box<dyn std::error::Error>> {
    println!("Nuzo v{} — type .exit to quit", VERSION);
    let mut session = engine.new_session();
    let mut line = String::new();
    loop {
        print!(">>> ");
        use std::io::Write;
        std::io::stdout().flush()?;
        line.clear();
        if std::io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        if s == ".exit" || s == ".quit" {
            break;
        }
        if s == ".reset" {
            session.reset();
            println!("[session reset]");
            continue;
        }
        match session.eval(s) {
            Ok(out) => {
                for l in &out.stdout {
                    print!("{}", l);
                }
                if !is_nil(&out.value) {
                    println!("{}", out.value);
                }
            }
            Err(e) => {
                let stack = session.vm_mut().last_call_stack().to_vec();
                render_nuzo_error(&e, &stack, lang);
            }
        }
    }
    Ok(0)
}

fn print_help() {
    println!(
        r#"Nuzo v{} — the Nuzo programming language

USAGE:
    nuzo [OPTIONS] [COMMAND] [FILE]

COMMANDS:
    (none)              Start REPL
    <file>              Run a script file
    run <file>          Run a script file (explicit)
    compile <file>      Compile and show bytecode
    bench <file>        Benchmark script execution
    check <file>        Check syntax / compile without running
    test [dir]          Auto-discover and run *.nuzo tests (default: tests/e2e)
    e2e [dir]           Run end-to-end tests (default: tests/e2e + examples/nuzo)
    repl                Start interactive REPL

OPTIONS:
    -e, --eval <code>   Evaluate inline code
    -k, --filter <pat>  Filter tests by name/path (test/e2e commands)
    -t, --timeout <ms>  Set per-test timeout in ms (default: 5000)
    -V, --verbose       Verbose output (show each test result)
    --trace             Enable execution tracing
    --disassemble       Output bytecode disassembly (compile command)
    --lang <LANG>       Error message language: zh|en|both (overrides NUZO_LANG)
    -h, --help          Show this help
    -v, --version       Show version"#,
        VERSION
    );
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_no_args() {
        let cli = Cli::try_parse_from(args(&["nuzo"])).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.help);
        assert!(!cli.version);
        assert!(cli.eval.is_none());
        assert!(cli.file.is_none());
        assert!(!cli.trace);
        assert!(!cli.verbose);
    }

    #[test]
    fn parse_help() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--help"])).unwrap();
        assert!(cli.help);
    }

    #[test]
    fn parse_help_short() {
        let cli = Cli::try_parse_from(args(&["nuzo", "-h"])).unwrap();
        assert!(cli.help);
    }

    #[test]
    fn parse_version() {
        let cli = Cli::try_parse_from(args(&["nuzo", "-v"])).unwrap();
        assert!(cli.version);
    }

    #[test]
    fn parse_version_long() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--version"])).unwrap();
        assert!(cli.version);
    }

    #[test]
    fn parse_run_file() {
        let cli = Cli::try_parse_from(args(&["nuzo", "script.nu"])).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.file, Some(PathBuf::from("script.nu")));
    }

    #[test]
    fn parse_run_command() {
        let cli = Cli::try_parse_from(args(&["nuzo", "run", "script.nu"])).unwrap();
        assert_eq!(cli.command, Some(CliCommand::Run { file: PathBuf::from("script.nu") }));
    }

    #[test]
    fn parse_eval() {
        let cli = Cli::try_parse_from(args(&["nuzo", "-e", "print(1)"])).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.eval, Some("print(1)".to_string()));
    }

    #[test]
    fn parse_compile() {
        let cli = Cli::try_parse_from(args(&["nuzo", "compile", "script.nu"])).unwrap();
        assert_eq!(
            cli.command,
            Some(CliCommand::Compile { file: PathBuf::from("script.nu"), disassemble: false })
        );
    }

    #[test]
    fn parse_compile_with_disassemble() {
        let cli =
            Cli::try_parse_from(args(&["nuzo", "compile", "--disassemble", "script.nu"])).unwrap();
        assert_eq!(
            cli.command,
            Some(CliCommand::Compile { file: PathBuf::from("script.nu"), disassemble: true })
        );
    }

    #[test]
    fn parse_check() {
        let cli = Cli::try_parse_from(args(&["nuzo", "check", "script.nu"])).unwrap();
        assert_eq!(cli.command, Some(CliCommand::Check { file: PathBuf::from("script.nu") }));
    }

    #[test]
    fn parse_bench() {
        let cli = Cli::try_parse_from(args(&["nuzo", "bench", "script.nu"])).unwrap();
        assert_eq!(cli.command, Some(CliCommand::Bench { file: PathBuf::from("script.nu") }));
    }

    #[test]
    fn parse_test_defaults() {
        let cli = Cli::try_parse_from(args(&["nuzo", "test"])).unwrap();
        assert_eq!(cli.command, Some(CliCommand::Test { dirs: vec![] }));
        assert!(!cli.verbose);
    }

    #[test]
    fn parse_test_with_dir_and_filter_and_timeout() {
        let cli =
            Cli::try_parse_from(args(&["nuzo", "test", "tests", "-k", "foo", "-t", "1000", "-V"]))
                .unwrap();
        assert_eq!(cli.command, Some(CliCommand::Test { dirs: vec![PathBuf::from("tests")] }));
        assert_eq!(cli.filter, Some("foo".to_string()));
        assert_eq!(cli.timeout, Some(1000));
        assert!(cli.verbose);
    }

    #[test]
    fn parse_e2e_defaults() {
        let cli = Cli::try_parse_from(args(&["nuzo", "e2e"])).unwrap();
        assert_eq!(cli.command, Some(CliCommand::E2e { dirs: vec![] }));
        assert!(!cli.verbose);
    }

    #[test]
    fn parse_e2e_with_dir_and_filter_and_timeout() {
        let cli = Cli::try_parse_from(args(&[
            "nuzo",
            "e2e",
            "examples/nuzo",
            "-k",
            "closure",
            "-t",
            "2000",
            "-V",
        ]))
        .unwrap();
        assert_eq!(
            cli.command,
            Some(CliCommand::E2e { dirs: vec![PathBuf::from("examples/nuzo")] })
        );
        assert_eq!(cli.filter, Some("closure".to_string()));
        assert_eq!(cli.timeout, Some(2000));
        assert!(cli.verbose);
    }

    #[test]
    fn parse_trace() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--trace"])).unwrap();
        assert!(cli.trace);
    }

    #[test]
    fn parse_verbose() {
        let cli = Cli::try_parse_from(args(&["nuzo", "-V"])).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn parse_unknown_option() {
        let err = Cli::try_parse_from(args(&["nuzo", "--unknown"])).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown") || msg.contains("unexpected") || msg.contains("--unknown"),
            "expected error about unknown option, got: {}",
            msg
        );
    }

    #[test]
    fn parse_compile_missing_file() {
        // clap reports missing required positional argument
        let err = Cli::try_parse_from(args(&["nuzo", "compile"])).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("file") || msg.contains("required"),
            "expected error about missing file, got: {}",
            msg
        );
    }

    #[test]
    fn parse_multiple_commands() {
        // With clap, "nuzo compile check" parses "check" as the file argument to compile
        let cli = Cli::try_parse_from(args(&["nuzo", "compile", "check"])).unwrap();
        assert_eq!(
            cli.command,
            Some(CliCommand::Compile { file: PathBuf::from("check"), disassemble: false })
        );
    }

    #[test]
    fn parse_eval_with_file_rejected() {
        // clap will parse both --eval and file successfully; the conflict is caught in run()
        let cli = Cli::try_parse_from(args(&["nuzo", "-e", "1", "file.nu"])).unwrap();
        assert_eq!(cli.eval, Some("1".to_string()));
        assert_eq!(cli.file, Some(PathBuf::from("file.nu")));
        // run() should detect this conflict
        let result = run(cli);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("eval does not take files"));
    }

    // ------------------------------------------------------------------
    // --lang option
    // ------------------------------------------------------------------

    #[test]
    fn parse_lang_zh() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--lang", "zh"])).unwrap();
        assert_eq!(cli.lang, Some(LangMode::Zh));
    }

    #[test]
    fn parse_lang_en() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--lang", "en"])).unwrap();
        assert_eq!(cli.lang, Some(LangMode::En));
    }

    #[test]
    fn parse_lang_both() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--lang", "both"])).unwrap();
        assert_eq!(cli.lang, Some(LangMode::Both));
    }

    #[test]
    fn parse_lang_case_insensitive() {
        let cli = Cli::try_parse_from(args(&["nuzo", "--lang", "ZH"])).unwrap();
        assert_eq!(cli.lang, Some(LangMode::Zh));
    }

    #[test]
    fn parse_lang_absent_defaults_none() {
        let cli = Cli::try_parse_from(args(&["nuzo"])).unwrap();
        assert!(cli.lang.is_none(), "未指定 --lang 时应为 None，由渲染器读取 NUZO_LANG");
    }

    #[test]
    fn parse_lang_invalid_rejected() {
        let err = Cli::try_parse_from(args(&["nuzo", "--lang", "fr"])).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid value") && msg.contains("zh|en|both"),
            "应拒绝未知语言值，got: {}",
            msg
        );
    }

    #[test]
    fn parse_lang_applies_to_subcommand() {
        // --lang is global, can be used with subcommands
        let cli = Cli::try_parse_from(args(&["nuzo", "run", "--lang", "en", "script.nu"])).unwrap();
        assert_eq!(cli.lang, Some(LangMode::En));
        assert_eq!(cli.command, Some(CliCommand::Run { file: PathBuf::from("script.nu") }));
    }
}
