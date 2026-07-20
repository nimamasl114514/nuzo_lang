$ErrorActionPreference = "Stop"

# Resolve paths relative to script location (no hardcoded absolute paths)
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$CallgraphTool = Join-Path (Split-Path -Parent $ProjectRoot) "nuzo_callgraph\Cargo.toml"
$CallgraphOutput = Join-Path $ProjectRoot "CALL_GRAPH.md"

# 从 workspace.package.version 提取版本号，与 justfile callgraph 命令保持一致
# 否则 --check 模式生成的临时文件版本号与现有 CALL_GRAPH.md 不一致，误报 out of date
$versionLine = Select-String -Path "$ProjectRoot\Cargo.toml" -Pattern '^version = "' | Select-Object -First 1
$ver = if ($versionLine) { $versionLine.Line -replace '^version = "([^"]+)".*', '$1' } else { "0.0.0" }

cargo run --manifest-path $CallgraphTool -- --project $ProjectRoot --output $CallgraphOutput --check --format markdown --visibility pub-super --workspace --name "Nuzo Lang" --version $ver

if ($LASTEXITCODE -eq 0) {
    Write-Host "CALL_GRAPH.md is in sync"
} else {
    Write-Error "CALL_GRAPH.md is OUT OF SYNC! Run regeneration command."
    exit 1
}
