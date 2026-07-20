#!/bin/bash
# Nuzo Lang Watch Mode (Enhanced)
# 监听文件变化自动 check + test + callgraph
#
# 监听 crates/ 目录下的 .rs 文件和 Cargo.toml 文件变化，自动触发：
#   - .rs 文件变化      -> cargo check --workspace -> cargo test --workspace --lib -> 重新生成 CALL_GRAPH.md
#   - Cargo.toml 变化   -> cargo check --workspace
# 使用防抖机制（默认 2 秒）避免连续保存触发多次。
# 某个步骤失败不阻止后续步骤，只打印错误。
#
# 用法:
#   ./scripts/watch.sh [选项] [路径]
#
# 选项:
#   --skip-test          跳过测试步骤
#   --skip-callgraph     跳过 CALL_GRAPH 生成
#   --debounce SECONDS   防抖延迟秒数 (默认: 2)
#   -h, --help           显示帮助
#
# 示例:
#   ./scripts/watch.sh                              # 监听 crates/
#   ./scripts/watch.sh crates/nuzo-vm            # 监听指定目录
#   ./scripts/watch.sh --skip-test                  # 跳过测试
#   ./scripts/watch.sh --skip-callgraph crates/api  # 组合使用

set -uo pipefail

# === 配置 ===
SKIP_TEST=false
SKIP_CALLGRAPH=false
DEBOUNCE_SECONDS=2

# === 颜色定义（仅在终端输出时启用）===
if [[ -t 1 ]]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    MAGENTA='\033[0;35m'
    CYAN='\033[0;36m'
    GRAY='\033[0;90m'
    WHITE='\033[1;37m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' MAGENTA='' CYAN='' GRAY='' WHITE='' RESET=''
fi

# === 参数解析 ===
usage() {
    cat <<EOF
用法: $0 [选项] [路径]

选项:
  --skip-test          跳过测试步骤
  --skip-callgraph     跳过 CALL_GRAPH 生成
  --debounce SECONDS   防抖延迟秒数 (默认: 2)
  -h, --help           显示帮助

路径:
  监听的目录 (默认: crates)

示例:
  $0                              # 监听 crates/
  $0 crates/vm/nuzo_vm            # 监听指定目录
  $0 --skip-test                  # 跳过测试
  $0 --skip-callgraph crates/api  # 组合使用
EOF
}

POSITIONAL=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-test)
            SKIP_TEST=true
            shift
            ;;
        --skip-callgraph)
            SKIP_CALLGRAPH=true
            shift
            ;;
        --debounce)
            DEBOUNCE_SECONDS="${2:?--debounce 需要参数}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -*)
            echo "未知选项: $1" >&2
            usage >&2
            exit 1
            ;;
        *)
            POSITIONAL+=("$1")
            shift
            ;;
    esac
done

WATCH_SUBPATH="${POSITIONAL[0]:-crates}"

# === 路径解析（基于脚本位置，不硬编码项目路径）===
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
WATCH_PATH="$PROJECT_ROOT/$WATCH_SUBPATH"
CALLGRAPH_OUTPUT="$PROJECT_ROOT/CALL_GRAPH.md"

# CALL_GRAPH 工具目录（优先环境变量，否则尝试常见路径）
if [[ -n "${NUZO_CALLGRAPH_DIR:-}" ]]; then
    CALLGRAPH_TOOL_DIR="$NUZO_CALLGRAPH_DIR"
elif [[ -d "/d/10/nuzo_callgraph" ]]; then
    # Git Bash on Windows
    CALLGRAPH_TOOL_DIR="/d/10/nuzo_callgraph"
elif [[ -d "d:/10/nuzo_callgraph" ]]; then
    # MSYS2/Cygwin style
    CALLGRAPH_TOOL_DIR="d:/10/nuzo_callgraph"
elif [[ -d "$PROJECT_ROOT/../nuzo_callgraph" ]]; then
    # 同级目录
    CALLGRAPH_TOOL_DIR="$(cd "$PROJECT_ROOT/../nuzo_callgraph" && pwd)"
else
    CALLGRAPH_TOOL_DIR=""
fi

# === 验证 ===
if [[ ! -d "$WATCH_PATH" ]]; then
    echo -e "${RED}[ERROR] 监听路径不存在: $WATCH_PATH${RESET}" >&2
    exit 1
fi
if [[ ! -f "$PROJECT_ROOT/Cargo.toml" ]]; then
    echo -e "${RED}[ERROR] 未找到 Cargo.toml，请在项目根目录运行${RESET}" >&2
    exit 1
fi

# === 清理函数（Ctrl+C 时清理后台进程）===
WATCHER_PID=""
cleanup() {
    echo ""
    echo -e "${CYAN}[INFO] 收到中断信号，正在清理...${RESET}"
    if [[ -n "$WATCHER_PID" ]] && kill -0 "$WATCHER_PID" 2>/dev/null; then
        kill "$WATCHER_PID" 2>/dev/null
        wait "$WATCHER_PID" 2>/dev/null
    fi
    echo -e "${CYAN}[INFO] 已停止，资源已清理${RESET}"
    exit 0
}

trap cleanup SIGINT SIGTERM

# === 输出处理：过滤 cargo 输出，只显示关键信息 ===
colorize_cargo() {
    while IFS= read -r line; do
        if echo "$line" | grep -qE "^error"; then
            echo -e "  ${RED}${line}${RESET}"
        elif echo "$line" | grep -qE "^warning"; then
            echo -e "  ${YELLOW}${line}${RESET}"
        elif echo "$line" | grep -qE "^Compiling"; then
            echo -e "  ${CYAN}${line}${RESET}"
        elif echo "$line" | grep -qE "^Finished"; then
            echo -e "  ${GREEN}${line}${RESET}"
        elif echo "$line" | grep -qE "test result:"; then
            echo -e "  ${WHITE}${line}${RESET}"
        elif echo "$line" | grep -qE "^test .* FAILED"; then
            echo -e "  ${RED}${line}${RESET}"
        elif echo "$line" | grep -qE "^running [0-9]+ test"; then
            echo -e "  ${WHITE}${line}${RESET}"
        fi
    done
}

# === 执行步骤 1：cargo check ===
run_cargo_check() {
    echo -e "${BLUE}>> [1/3] cargo check --workspace ...${RESET}"
    local start_time
    start_time=$(date +%s)
    (cd "$PROJECT_ROOT" && cargo check --workspace 2>&1) | colorize_cargo
    local exit_code=${PIPESTATUS[0]}
    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))
    if [[ $exit_code -eq 0 ]]; then
        echo -e "  ${GREEN}[OK] cargo check 通过 (${duration}s)${RESET}"
        return 0
    else
        echo -e "  ${RED}[FAIL] cargo check 失败 (${duration}s)${RESET}"
        return 1
    fi
}

# === 执行步骤 2：cargo test ===
run_cargo_test() {
    echo -e "${GREEN}>> [2/3] cargo test --workspace --lib ...${RESET}"
    local start_time
    start_time=$(date +%s)
    (cd "$PROJECT_ROOT" && cargo test --workspace --lib 2>&1) | colorize_cargo
    local exit_code=${PIPESTATUS[0]}
    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))
    if [[ $exit_code -eq 0 ]]; then
        echo -e "  ${GREEN}[OK] cargo test 通过 (${duration}s)${RESET}"
        return 0
    else
        echo -e "  ${RED}[FAIL] cargo test 失败 (${duration}s)${RESET}"
        return 1
    fi
}

# === 执行步骤 3：重新生成 CALL_GRAPH ===
run_callgraph() {
    echo -e "${MAGENTA}>> [3/3] 重新生成 CALL_GRAPH.md ...${RESET}"
    if [[ -z "$CALLGRAPH_TOOL_DIR" ]]; then
        echo -e "  ${YELLOW}[SKIP] 未配置 CALL_GRAPH 工具目录（设置 NUZO_CALLGRAPH_DIR 环境变量）${RESET}"
        return 1
    fi
    if [[ ! -d "$CALLGRAPH_TOOL_DIR" ]]; then
        echo -e "  ${YELLOW}[SKIP] CALL_GRAPH 工具目录不存在: $CALLGRAPH_TOOL_DIR${RESET}"
        return 1
    fi
    local start_time
    start_time=$(date +%s)
    (cd "$CALLGRAPH_TOOL_DIR" && cargo run -- --project "$PROJECT_ROOT" --output "$CALLGRAPH_OUTPUT" --format markdown --visibility pub-super --workspace --name "Nuzo Lang" 2>&1) | colorize_cargo
    local exit_code=${PIPESTATUS[0]}
    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))
    if [[ $exit_code -eq 0 ]]; then
        echo -e "  ${GREEN}[OK] CALL_GRAPH.md 已生成 (${duration}s)${RESET}"
        return 0
    else
        echo -e "  ${RED}[FAIL] CALL_GRAPH 生成失败 (${duration}s)${RESET}"
        return 1
    fi
}

# === 执行链：check -> test -> callgraph（错误不阻止后续步骤）===
run_full_pipeline() {
    local change_file="$1"
    local change_type="$2"
    local is_cargo_toml="$3"

    local timestamp
    timestamp=$(date '+%H:%M:%S')
    local start_time
    start_time=$(date +%s)

    echo ""
    echo -e "${GRAY}==================================================${RESET}"
    echo -e "${YELLOW}[$timestamp] 检测到变化 ($change_type): $change_file${RESET}"
    echo -e "${GRAY}==================================================${RESET}"

    # 步骤 1: cargo check（始终执行）
    run_cargo_check || true

    # Cargo.toml 变化只触发 check
    if [[ "$is_cargo_toml" == "true" ]]; then
        echo -e "${YELLOW}[INFO] Cargo.toml 变化，仅执行 check${RESET}"
    else
        # 步骤 2: cargo test（错误不阻止后续）
        if [[ "$SKIP_TEST" == "false" ]]; then
            run_cargo_test || true
        else
            echo -e "${GRAY}[SKIP] cargo test (--skip-test)${RESET}"
        fi

        # 步骤 3: CALL_GRAPH（错误不阻止后续）
        if [[ "$SKIP_CALLGRAPH" == "false" ]]; then
            run_callgraph || true
        else
            echo -e "${GRAY}[SKIP] CALL_GRAPH 生成 (--skip-callgraph)${RESET}"
        fi
    fi

    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))
    echo -e "${GRAY}--------------------------------------------------${RESET}"
    echo -e "${GRAY}[$(date '+%H:%M:%S')] 完成 (总耗时 ${duration}s)，等待下一次变化...${RESET}"
}

# === 文件过滤：只关心 .rs 和 Cargo.toml ===
should_watch() {
    local path="$1"
    if [[ "$path" =~ \.rs$ ]]; then
        return 0
    fi
    if [[ "$(basename "$path")" == "Cargo.toml" ]]; then
        return 0
    fi
    return 1
}

# === 检测可用的监听工具 ===
detect_watcher() {
    if command -v inotifywait &>/dev/null; then
        echo "inotify"
    elif command -v fswatch &>/dev/null; then
        echo "fswatch"
    else
        echo "polling"
    fi
}

# === Polling 模式：获取最新文件状态（mtime + 文件名）===
get_latest_state() {
    # 检测 stat 版本（GNU vs BSD），stdout/stderr 全部重定向避免污染函数输出
    if stat -c %Y /dev/null >/dev/null 2>&1; then
        # GNU stat (Linux)
        find "$WATCH_PATH" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
            -exec stat -c '%Y %n' {} \; 2>/dev/null | sort -rn | head -1
    else
        # BSD stat (macOS)
        find "$WATCH_PATH" -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
            -exec stat -f '%m %N' {} \; 2>/dev/null | sort -rn | head -1
    fi
}

# === 启动横幅 ===
echo -e "${CYAN}========================================${RESET}"
echo -e "${CYAN}  Nuzo Lang Watch Mode (Enhanced)${RESET}"
echo -e "${CYAN}========================================${RESET}"
echo ""
echo -e "  ${WHITE}监听路径   :${RESET} $WATCH_PATH"
echo -e "  ${WHITE}项目根目录 :${RESET} $PROJECT_ROOT"
echo -e "  ${WHITE}防抖延迟   :${RESET} ${DEBOUNCE_SECONDS}s"
echo -e "  ${WHITE}监听文件   :${RESET} *.rs, Cargo.toml"
echo -e "  ${WHITE}执行链     :${RESET} check -> test -> callgraph"
if [[ -n "$CALLGRAPH_TOOL_DIR" ]]; then
    echo -e "  ${WHITE}CALL_GRAPH :${RESET} $CALLGRAPH_TOOL_DIR"
else
    echo -e "  ${YELLOW}CALL_GRAPH :${RESET} 未配置 (设置 NUZO_CALLGRAPH_DIR)"
fi
echo ""
if [[ "$SKIP_TEST" == "true" ]]; then
    echo -e "  ${YELLOW}[SKIP] test${RESET}"
fi
if [[ "$SKIP_CALLGRAPH" == "true" ]]; then
    echo -e "  ${YELLOW}[SKIP] callgraph${RESET}"
fi
echo ""
echo -e "${GRAY}  按 Ctrl+C 停止${RESET}"
echo ""

# === 检测监听方式 ===
WATCHER_TYPE=$(detect_watcher)
echo -e "${GRAY}  监听方式: $WATCHER_TYPE${RESET}"
echo ""

# === 主循环（根据监听方式分支）===
case "$WATCHER_TYPE" in
    inotify)
        # 使用 inotifywait（Linux）
        # -m 持续监听, -r 递归, --event 指定事件, --format 自定义输出格式
        inotifywait -m -r \
            --event modify,create,move \
            --format '%w%f|%e' \
            "$WATCH_PATH" 2>/dev/null | while true; do
            # 阻塞读取第一个事件
            if ! IFS='|' read -r filepath event; then
                break
            fi

            # 过滤：只关心 .rs 和 Cargo.toml
            if ! should_watch "$filepath"; then
                continue
            fi

            # 防抖：在 debounce 时间内收集后续事件，只保留最后一个
            while IFS='|' read -t "$DEBOUNCE_SECONDS" -r f e; do
                if should_watch "$f"; then
                    filepath="$f"
                    event="$e"
                fi
            done

            # 判断是否为 Cargo.toml
            local_filename=$(basename "$filepath")
            is_cargo_toml="false"
            if [[ "$local_filename" == "Cargo.toml" ]]; then
                is_cargo_toml="true"
            fi

            # 执行完整流水线
            run_full_pipeline "$filepath" "$event" "$is_cargo_toml"
        done
        ;;
    fswatch)
        # 使用 fswatch（macOS/Linux）
        # -r 递归, -l 1 内部延迟 1 秒
        fswatch -r -l 1 "$WATCH_PATH" 2>/dev/null | while true; do
            # 阻塞读取第一个事件
            if ! IFS= read -r filepath; then
                break
            fi

            # 过滤：只关心 .rs 和 Cargo.toml
            if ! should_watch "$filepath"; then
                continue
            fi

            # 防抖：在 debounce 时间内收集后续事件，只保留最后一个
            while IFS= read -t "$DEBOUNCE_SECONDS" -r f; do
                if should_watch "$f"; then
                    filepath="$f"
                fi
            done

            # 判断是否为 Cargo.toml
            local_filename=$(basename "$filepath")
            is_cargo_toml="false"
            if [[ "$local_filename" == "Cargo.toml" ]]; then
                is_cargo_toml="true"
            fi

            # 执行完整流水线
            run_full_pipeline "$filepath" "modified" "$is_cargo_toml"
        done
        ;;
    polling)
        # Polling 兜底（无 inotifywait/fswatch 时使用）
        echo -e "${YELLOW}[INFO] 未检测到 inotifywait/fswatch，使用 polling 模式${RESET}"
        echo -e "${YELLOW}[INFO] 建议安装: apt install inotify-tools (Linux) 或 brew install fswatch (macOS)${RESET}"
        echo ""

        LAST_STATE=$(get_latest_state)

        while true; do
            CURRENT=$(get_latest_state)

            if [[ -n "$CURRENT" ]] && [[ "$CURRENT" != "$LAST_STATE" ]]; then
                # 解析文件名（mtime + 路径，路径可能含空格，用 cut 取第一个字段后的所有内容）
                CURRENT_FILE=$(echo "$CURRENT" | cut -d' ' -f2-)

                local_filename=$(basename "$CURRENT_FILE")
                is_cargo_toml="false"
                if [[ "$local_filename" == "Cargo.toml" ]]; then
                    is_cargo_toml="true"
                fi

                # 防抖：等待 debounce 秒
                sleep "$DEBOUNCE_SECONDS"

                # 执行完整流水线
                run_full_pipeline "$CURRENT_FILE" "modified" "$is_cargo_toml"

                # 更新状态（重新获取，因为处理期间可能有新变化）
                LAST_STATE=$(get_latest_state)
            fi

            sleep 1
        done
        ;;
    *)
        echo -e "${RED}[ERROR] 未知监听方式: $WATCHER_TYPE${RESET}" >&2
        exit 1
        ;;
esac
