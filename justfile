# justfile — Nuzo Lang 项目自动化命令
#
# 跨平台说明：所有配方使用 POSIX sh 语法（just 默认）。
# Windows 用户需安装 Git Bash 或 WSL，让 just 能找到 sh.exe。
# 不要在此添加 PowerShell 专有语法（Get-Command / $env:VAR / Select-String 等）。
# 若需检测可选工具（如 sccache），用 `command -v <tool>` 而非 PowerShell cmdlet。
#
# 用法:
#   just              # 显示所有可用命令
#   just sync         # 一键同步：callgraph + fmt
#   just check-all    # 全量检查：check + test + lint + fmt-check
#   just clean-vendor # 清理 vendor/ console/ 重复依赖
#   just check        # 全量编译检查
#   just test         # 全量测试
#   just lint         # clippy 静态分析
#   just audit        # 安全审计（依赖漏洞 + 许可证）
#   just coverage     # 测试覆盖率报告
#
# 安装: cargo install just

# =====================================================================
# 一键同步（改动一处，自动同步所有产物）
# =====================================================================

# 一键同步：改动代码后，自动生成所有衍生产物
# AI 用途: 同步代码衍生产物（opcode 定义/测试桩/CALL_GRAPH/fmt），改动代码后跑
sync: sync-opcode-apply sync-tests-apply callgraph fmt
    @echo "=== 同步完成（opcode + tests + callgraph + fmt）==="

# Opcode 全链路同步（dry-run 报告，不修改文件）
# AI 用途: Opcode 同步 dry-run 报告（只读，不修改文件）
sync-opcode:
    python scripts/sync_opcode.py --project . --dry-run
    @echo "=== Opcode 同步报告完成（dry-run）==="

# Opcode 全链路同步（实际修改文件）
# AI 用途: Opcode 同步（实际修改文件，写盘）
sync-opcode-apply:
    python scripts/sync_opcode.py --project . --no-dry-run
    @echo "=== Opcode 同步已应用 ==="

# 测试桩同步（dry-run 报告，不修改文件）
# AI 用途: 测试桩同步 dry-run 报告（只读，不修改文件）
sync-tests:
    python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md --dry-run
    @echo "=== 测试桩同步报告完成（dry-run）==="

# 测试桩同步（实际生成文件）
# AI 用途: 测试桩同步（实际生成文件，写盘）
sync-tests-apply:
    python scripts/sync_tests.py --project . --callgraph CALL_GRAPH.md --no-dry-run
    @echo "=== 测试桩同步已应用 ==="

# Watch 模式：文件变化时自动同步 + 编译检查（需先安装: cargo install cargo-watch）
# AI 用途: Watch 模式文件变化自动同步+编译（长驻进程，人工用）
watch-sync:
    cargo watch -i target -i vendor -s "just sync-opcode-apply; just sync-tests-apply; just check"
    @echo "=== Watch-sync 已启动（保存文件自动同步）==="

# =====================================================================
# 依赖管理（消灭 P0: 重复 vendor）
# =====================================================================

# 清理手动拷贝的 vendor 依赖，重新生成精准依赖列表
# AI 用途: 清理 vendor/ console/ 重复依赖（释放空间）
clean-vendor:
    rm -rf vendor/ console/
    @echo "=== 已清理 vendor/ + console/ (释放 ~14MB) ==="
    @echo "如需离线构建，运行: cargo vendor --manifest-path Cargo.toml"

# 检查是否有不该存在的目录
# AI 用途: 检查残留 vendor/ console/ 目录（CI 门禁）
check-vendor:
    @if [ -d "vendor" ]; then echo "ERROR: vendor/ 不应存在 (运行 just clean-vendor)"; exit 1; fi
    @if [ -d "console" ]; then echo "ERROR: console/ 不应存在 (运行 just clean-vendor)"; exit 1; fi
    @echo "OK: 无残留 vendor 目录"

# =====================================================================
# 编译与测试
# =====================================================================

# 全量编译检查（零警告目标）
# AI 用途: 快速编译检查（比 test 快），提交前验证语法
check:
    cargo check --workspace --all-targets

# 快速编译（仅 lib）
# AI 用途: 最快编译检查（仅 lib，开发内环用）
check-lib:
    cargo check --workspace --lib

# 快速编译（开发内环，自动使用 sccache 如可用）
# AI 用途: 开发内环快速编译（自动启用 sccache）
check-fast:
    #!/usr/bin/env sh
    if command -v sccache >/dev/null 2>&1; then
        export RUSTC_WRAPPER=sccache
        sccache --start-server >/dev/null 2>&1 || true
    else
        unset RUSTC_WRAPPER 2>/dev/null || true
    fi
    cargo check --workspace --lib

# 全量测试（不含 benches，CI 用，~7s）
# AI 用途: 全量测试（workspace 所有 crate，不含 benches），提交前必跑
test:
    cargo test --workspace --tests
    cargo test -p nuzo_config -- --test-threads=1

# 全量测试含 benches（完整测试，~6min，慎用）
# AI 用途: 全量测试含 benches（完整测试，慢，慎用）
test-all:
    cargo test --workspace --all-targets
    cargo test -p nuzo_config -- --test-threads=1

# 仅运行 benches（性能基准，独立 job 用）
# AI 用途: 仅运行 benches（性能基准，CI 独立 job 用，非阻塞）
test-bench:
    cargo test --workspace --benches

# 按 crate 分片测试（IDX 从 0 开始，TOTAL 总片数）
# AI 用途: CI matrix 分片测试，并行加速
test-shard IDX TOTAL:
    #!/usr/bin/env sh
    # 列出所有有测试的 crate（按耗时均衡分布）
    crates="nuzo_vm nuzo_values nuzo_proc_core nuzo_ir nuzo_error nuzo_compiler nuzo_helpers nuzo_core nuzo_signal nuzo_bytecode nuzo_class nuzo_config nuzo_opcode nuzo_frontend nuzo_run nuzo_codegen nuzo_gui"
    idx={{IDX}}
    mod={{TOTAL}}
    # 简单取模分片
    i=0
    for c in $crates; do
        if [ $((i % mod)) -eq $idx ]; then
            echo "=== Shard {{IDX}}/{{TOTAL}}: testing $c ==="
            cargo test -p "$c" --tests || exit 1
        fi
        i=$((i+1))
    done

# 快速测试（仅 lib，自动使用 sccache 如可用）
# AI 用途: 快速测试（仅 lib，开发内环用）
test-fast:
    #!/usr/bin/env sh
    if command -v sccache >/dev/null 2>&1; then
        export RUSTC_WRAPPER=sccache
        sccache --start-server >/dev/null 2>&1 || true
    else
        unset RUSTC_WRAPPER 2>/dev/null || true
    fi
    cargo test --workspace --lib

# 指定 crate 测试
# AI 用途: 指定 crate 测试（修改单 crate 后快速验证）
test-crate CRATE:
    cargo test -p {{CRATE}} --lib

# UI compile-fail 测试（显式启用）
# AI 用途: UI compile-fail 测试（宏编译错误用例）
test-ui:
    #!/usr/bin/env sh
    if command -v sccache >/dev/null 2>&1; then
        export RUSTC_WRAPPER=sccache
        sccache --start-server >/dev/null 2>&1 || true
    else
        unset RUSTC_WRAPPER 2>/dev/null || true
    fi
    cargo test -p nuzo_class_macros --features ui-tests --test compile_errors -- --test-threads=1

# 自动生成测试桩（显式编译，不执行）
# AI 用途: 编译生成测试桩（不执行，验证生成代码可编译）
test-generated:
    @echo "提示：generated_stubs 已移除"

# 基准测试
# AI 用途: 运行全量基准测试
bench:
    # CARGO_INCREMENTAL=0 规避 Windows 上 rustc 增量编译 ThinLTO 缓存 bug
    CARGO_INCREMENTAL=0 cargo bench --workspace

# 属性驱动测试（quickcheck，快速模式：100 案例）
# 注：设置 CARGO_INCREMENTAL=0 避免 rustc 1.96.0 Windows 增量编译 bug
# AI 用途: 属性驱动测试 quickcheck（当前已禁用，crate 未创建）
proptest:
    # nuzo_proptest crate 尚未创建，暂时禁用
    # CARGO_INCREMENTAL=0 cargo test -p nuzo_proptest -- --test-threads=1

# 属性驱动测试（深度模式：10000 案例）
# AI 用途: 属性驱动测试深度模式 10000 案例（当前已禁用）
proptest-deep:
    # nuzo_proptest crate 尚未创建，暂时禁用
    # CARGO_INCREMENTAL=0 QUICKCHECK_TESTS=10000 cargo test -p nuzo_proptest -- --test-threads=1

# 配置管理测试
# AI 用途: 配置管理测试（nuzo_config crate，单线程）
config-test:
    cargo test -p nuzo_config -- --test-threads=1

# 发布模式编译（优化检查）
# AI 用途: 发布模式编译检查（验证 release 优化无问题）
check-release:
    cargo check --workspace --release

# 全量检查：check + test + lint + fmt-check（提交前必跑）
# AI 用途: 全量检查门禁（check+test+lint+fmt），提交前必跑
check-all: check test lint fmt-check config-test
    @echo "=== 全量检查通过 ==="

# =====================================================================
# 代码质量
# =====================================================================

# Clippy 静态分析
# AI 用途: clippy 检查（CI 强制 -D warnings，本地可先跑防 CI 挂）
lint:
    cargo clippy --workspace -- -D warnings

# Clippy + 自动修复（谨慎使用）
# AI 用途: 自动修复 clippy 警告（谨慎使用，会改代码）
lint-fix:
    cargo clippy --workspace --fix -- -D warnings

# 格式化代码
# AI 用途: 格式化代码（提交前跑）
fmt:
    cargo fmt --all

# 检查格式是否正确（CI 用）
# AI 用途: 格式校验（CI 用，不修改代码）
fmt-check:
    cargo fmt --all -- --check

# 生成测试覆盖率报告（需要 cargo-llvm-cov）
# AI 用途: 生成测试覆盖率 HTML 报告（需 cargo-llvm-cov）
coverage:
    cargo llvm-cov --workspace --html --output-dir target/coverage

# 覆盖率摘要（仅打印摘要不生成 HTML）
# AI 用途: 覆盖率摘要（仅打印，不生成 HTML）
coverage-summary:
    cargo llvm-cov --workspace --summary

# =====================================================================
# 文档与 CALL_GRAPH
# =====================================================================

# 生成 CALL_GRAPH.md（项目规则强制要求修改代码后执行）
# 版本号从根 Cargo.toml 动态提取，避免硬编码
# AI 用途: 生成/校验 CALL_GRAPH.md，增删函数/opcode 后必跑
callgraph:
    #!/usr/bin/env sh
    root="{{justfile_directory()}}"
    # 提取 workspace.package.version（多行 toml，取第一次出现的 version = "..."）
    ver=$(grep '^version = "' "$root/Cargo.toml" | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')
    if [ -z "$ver" ]; then ver="0.0.0"; fi
    cargo run --manifest-path "$root/../nuzo_callgraph/Cargo.toml" -- \
        --project "$root" \
        --output "$root/CALL_GRAPH.md" \
        --format markdown --visibility pub-super --workspace \
        --name "Nuzo Lang" --version "$ver"

# 检查 CALL_GRAPH 是否需要更新
# AI 用途: 提示 CALL_GRAPH 是否需更新（仅打印提示，不执行）
callgraph-check:
    @echo "提示：任何增删函数/opcode/源文件后必须重新生成 CALL_GRAPH.md"

# 生成 Rust 文档并在浏览器打开
# AI 用途: 生成 Rust 文档并打开浏览器（人工查看用，AI 用 doc-serve）
doc:
    cargo doc --workspace --no-deps --open

# 生成文档到 target/doc/（不打开浏览器，适合部署）
# AI 用途: 生成文档到 target/doc/（不打开浏览器，适合部署/CI）
doc-serve:
    cargo doc --workspace --no-deps
    @echo "文档已生成到 target/doc/"

# =====================================================================
# 清理命令
# =====================================================================

# 深度清理（保留 target/ 以加速增量编译）
# AI 用途: 清理产物（保留 target/ 加速增量编译）
clean:
    cargo clean
    rm -rf .dbg/ .regression_cache/
    rm -f debug_*.rs debug_*.md CALL_GRAPH.md

# 完全清理（包括 target/）
# AI 用途: 完全清理（含 target/，慎用，会拖慢下次编译）
clean-all:
    cargo clean
    rm -rf target/ .dbg/ .regression_cache/ vendor/ console/
    rm -f debug_*.rs debug_*.md CALL_GRAPH.md

# =====================================================================
# 开发辅助
# =====================================================================

# 生成 cargo timings 报告（自动使用 sccache 如可用）
# AI 用途: 生成 cargo timings 编译耗时报告
timings:
    #!/usr/bin/env sh
    if command -v sccache >/dev/null 2>&1; then
        export RUSTC_WRAPPER=sccache
        sccache --start-server >/dev/null 2>&1 || true
    else
        unset RUSTC_WRAPPER 2>/dev/null || true
    fi
    cargo check --workspace --all-targets --timings

# Watch 模式：文件变化时自动编译
# AI 用途: Watch 模式文件变化自动编译（长驻进程，人工用）
watch:
    cargo watch -x "check --workspace"

# 运行 REPL
# AI 用途: 运行 Nuzo REPL（交互式，人工用）
repl:
    cargo run -p nuzo_run --bin nuzo_run -- repl

# 运行单个 .nuzo 文件
# AI 用途: 运行单个 .nuzo 文件（验证脚本行为）
run FILE:
    cargo run -p nuzo_run --bin nuzo_run -- {{FILE}}

# =====================================================================
# 规范检查与提交前钩子（pre-commit 用）
# =====================================================================

# 提交前检查（lefthook / git hook 调用）
# AI 用途: 提交前检查（fmt-check + vendor 检查），git hook 调用
pre-commit: fmt-check
    just check-vendor

# 安全审计：检查依赖漏洞 + 许可证 + 过时依赖
# AI 用途: 安全审计（cargo audit 漏洞扫描 + cargo deny 许可证）
audit: check-vendor fmt-check lint
    cargo audit 2>/dev/null || echo "提示: 运行 cargo install cargo-audit 安装"
    cargo deny check 2>/dev/null || echo "提示: 运行 cargo install cargo-deny 安装"
    @echo "=== 审计通过 ==="

# =====================================================================
# CHANGELOG 与发布
# =====================================================================

# 从 commit 信息生成 CHANGELOG（需要 scripts/generate_changelog.py）
# AI 用途: 从 commit 信息生成 CHANGELOG
changelog:
    @echo "提示：手动维护 CHANGELOG.md"

# 发布新版本（需要 cargo-release：cargo install cargo-release）
# AI 用途: 发布新版本（会推送 git+crate，需用户授权）
release LEVEL:
    cargo release {{LEVEL}} --execute

# 干运行发布（不实际推送，仅预览将要执行的步骤）
# AI 用途: 干运行发布预览（不实际推送，安全）
release-dry LEVEL:
    cargo release {{LEVEL}} --dry-run

# =====================================================================
# Opcode 文档生成
# =====================================================================

# 生成 Opcode 文档参考表 → docs/opcode-reference.md
# 由 nuzo_bytecode/build.rs 在 OUT_DIR/opcode_docs.md 输出，此处拷贝到 docs/
# AI 用途: 生成 Opcode 文档参考表到 docs/opcode-reference.md
gen-opcode-docs:
    #!/usr/bin/env sh
    cargo build -p nuzo_bytecode
    f=$(find target/debug/build -name 'opcode_docs.md' -path '*nuzo_bytecode-*' 2>/dev/null | head -n1)
    if [ -z "$f" ]; then
        echo "error: 未找到 opcode_docs.md (build.rs 是否执行?)" >&2
        exit 1
    fi
    cp "$f" docs/opcode-reference.md
    echo "=== 已生成 docs/opcode-reference.md ==="

# =====================================================================
# 一键文档同步
# =====================================================================

# 一键同步所有文档（opcode + tests + callgraph + dev-guide + opcode-docs + fmt）
# AI 用途: 一键同步所有文档（opcode+tests+callgraph+dev-guide+fmt）
doc-sync: sync-opcode-apply sync-tests-apply callgraph gen-opcode-docs
    just fmt
    @echo "=== 文档同步完成（opcode + tests + callgraph + dev-guide + opcode-docs + fmt）==="

# =====================================================================
# GitHub CLI 封装（AI 友好，非交互式，JSON 输出）
# =====================================================================
# 所有 gh-* 命令依赖 GH_TOKEN 环境变量，未设置时立即失败（不挂起等待输入）
# 所有 gh 调用附加 --no-pager 避免分页器挂起 AI

# AI 用途: 创建 GitHub PR（非交互式，HEAD 自动取当前分支）
gh-pr-create title body base:
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh pr create --title {{title}} --body {{body}} --base {{base}} --head {{`git rev-parse --abbrev-ref HEAD`}} --no-pager

# AI 用途: 列出 GitHub PR（默认 open，JSON 输出便于 AI 解析）
gh-pr-list state="open":
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh pr list --state {{state}} --json number,title,state,labels --no-pager

# AI 用途: 列出 GitHub issues（默认 open，JSON 输出便于 AI 解析）
gh-issue-list state="open":
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh issue list --state {{state}} --json number,title,state,labels --no-pager

# AI 用途: 查看 GitHub issue 详情（JSON 输出）
gh-issue-view number:
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh issue view {{number}} --json number,title,state,body,labels,comments --no-pager

# AI 用途: 创建 GitHub Release（非交互式，NOTES 省略时自动生成）
gh-release-create tag name notes="--generate-notes":
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh release create {{tag}} --title {{name}} --notes {{notes}} --no-pager

# AI 用途: 查看当前 PR 的 CI 检查状态（JSON 输出，非 PR 分支时 fallback 到最近 runs）
gh-check-status:
    @{{ if env_var_or_default("GH_TOKEN", "") == "" { error("error[GH_AUTH]: GH_TOKEN 未设置，请运行 set GH_TOKEN=xxx (Windows) 或 export GH_TOKEN=xxx (Linux) 后重试") } else { "" } }}
    gh pr checks --json name,state,link --no-pager || gh run list --limit 5 --json status,conclusion,name,link --no-pager
