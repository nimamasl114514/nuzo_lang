#!/bin/bash
# Check CALL_GRAPH.md is in sync with codebase
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# 从 workspace.package.version 提取版本号，与 justfile callgraph 命令保持一致
# 否则 --check 模式生成的临时文件版本号与现有 CALL_GRAPH.md 不一致，误报 out of date
ver=$(grep '^version = "' "$PROJECT_ROOT/Cargo.toml" | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')
if [ -z "$ver" ]; then ver="0.0.0"; fi

cargo run --manifest-path "$PROJECT_ROOT/../nuzo_callgraph/Cargo.toml" -- \
  --project "$PROJECT_ROOT" \
  --output "$PROJECT_ROOT/CALL_GRAPH.md" \
  --check \
  --format markdown \
  --visibility pub-super \
  --workspace \
  --name "Nuzo Lang" \
  --version "$ver"

if [ $? -eq 0 ]; then
    echo "CALL_GRAPH.md is in sync"
else
    echo "ERROR: CALL_GRAPH.md is OUT OF SYNC! Run: just callgraph"
    exit 1
fi
