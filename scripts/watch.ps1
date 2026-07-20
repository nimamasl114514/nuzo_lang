#requires -Version 5.1
<#
.SYNOPSIS
    Nuzo Lang Watch Mode - 文件变化自动 check + test + callgraph

.DESCRIPTION
    监听 crates/ 目录下的 .rs 文件和 Cargo.toml 文件变化，自动触发：
    - .rs 文件变化      -> cargo check --workspace -> cargo test --workspace --lib -> 重新生成 CALL_GRAPH.md
    - Cargo.toml 变化   -> cargo check --workspace
    使用防抖机制（默认 2 秒）避免连续保存触发多次。
    某个步骤失败不阻止后续步骤，只打印错误。

.PARAMETER Path
    监听路径（相对项目根目录或绝对路径），默认 crates

.PARAMETER SkipTest
    跳过测试步骤

.PARAMETER SkipCallgraph
    跳过 CALL_GRAPH 生成步骤

.PARAMETER DebounceSeconds
    防抖等待秒数，默认 2

.EXAMPLE
    .\scripts\watch.ps1
    .\scripts\watch.ps1 -Path crates\nuzo-vm
    .\scripts\watch.ps1 -SkipTest -SkipCallgraph
    .\scripts\watch.ps1 -Path crates\nuzo-vm -SkipTest -DebounceSeconds 3
#>
[CmdletBinding()]
param(
    [string]$Path = "crates",
    [switch]$SkipTest,
    [switch]$SkipCallgraph,
    [int]$DebounceSeconds = 2
)

# === UTF-8 编码（处理中文输出）===
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding = [System.Text.Encoding]::UTF8
# "Continue" surfaces errors without aborting; cargo step failures are detected
# via $LASTEXITCODE and handled per-step (one failing step must not kill the watch loop).
# Never use "SilentlyContinue" here — it would hide script-level bugs (e.g. bad path
# resolution) and make the watch appear to no-op silently.
$ErrorActionPreference = "Continue"

# === 路径解析（基于脚本位置，不硬编码项目路径）===
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$projectRoot = Split-Path -Parent $scriptDir

if ([System.IO.Path]::IsPathRooted($Path)) {
    $watchPath = $Path
} else {
    $watchPath = Join-Path $projectRoot $Path
}

# CALL_GRAPH 工具目录：与 justfile/check_callgraph.ps1 一致，取项目根的同级目录
# 可通过环境变量 NUZO_CALLGRAPH_DIR 覆盖（CI / 非标准布局时使用）
$callgraphToolDir = $env:NUZO_CALLGRAPH_DIR
if (-not $callgraphToolDir) {
    $callgraphToolDir = Join-Path (Split-Path -Parent $projectRoot) "nuzo_callgraph"
}
# CALL_GRAPH.md 输出位置：与 justfile 一致（CALL_GRAPH.md，根目录）
$callgraphOutput = Join-Path $projectRoot "CALL_GRAPH.md"

# === 验证 ===
if (-not (Test-Path $watchPath)) {
    Write-Host "[ERROR] 监听路径不存在: $watchPath" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path (Join-Path $projectRoot "Cargo.toml"))) {
    Write-Host "[ERROR] 未找到 Cargo.toml，请在项目根目录运行" -ForegroundColor Red
    exit 1
}

# === 同步状态（事件 Action -> 主循环，跨 runspace 安全）===
$global:state = [hashtable]::Synchronized(@{
    PendingChanges = [System.Collections.Queue]::Synchronized([System.Collections.Queue]::new())
    Watcher        = $null
    Subscriptions  = @()
})

# === 清理函数（释放 FileSystemWatcher 和事件订阅）===
function Stop-Watch {
    if ($global:state.Watcher) {
        try {
            $global:state.Watcher.EnableRaisingEvents = $false
            $global:state.Watcher.Dispose()
        } catch {}
        $global:state.Watcher = $null
    }
    foreach ($subId in $global:state.Subscriptions) {
        try { Unregister-Event -SubscriptionId $subId -ErrorAction SilentlyContinue } catch {}
    }
    $global:state.Subscriptions = @()
}

# === FileSystemWatcher 设置（实时监听，非 polling）===
$global:state.Watcher = New-Object System.IO.FileSystemWatcher
$global:state.Watcher.Path = $watchPath
$global:state.Watcher.IncludeSubdirectories = $true
$global:state.Watcher.EnableRaisingEvents = $true
$global:state.Watcher.NotifyFilter = [System.IO.NotifyFilters]::LastWrite -bor `
                                    [System.IO.NotifyFilters]::FileName -bor `
                                    [System.IO.NotifyFilters]::DirectoryName

# 事件 Action：过滤 .rs 和 Cargo.toml，加入同步队列
$onChanged = {
    try {
        $name = $Event.SourceEventArgs.Name
        $fullPath = $Event.SourceEventArgs.FullPath
        $changeType = $Event.SourceEventArgs.ChangeType

        # 只关心 .rs 和 Cargo.toml
        if (-not ($name -match '\.rs$' -or $name -match '^Cargo\.toml$')) {
            return
        }

        $global:state.PendingChanges.Enqueue(@{
            File         = $name
            Path         = $fullPath
            Type         = $changeType
            Time         = Get-Date
            IsCargoToml  = ($name -match '^Cargo\.toml$')
        })
    } catch {}
}

# 注册 Changed / Created / Renamed 三类事件
$sub1 = Register-ObjectEvent -InputObject $global:state.Watcher -EventName Changed -Action $onChanged
$sub2 = Register-ObjectEvent -InputObject $global:state.Watcher -EventName Created -Action $onChanged
$sub3 = Register-ObjectEvent -InputObject $global:state.Watcher -EventName Renamed -Action $onChanged
$global:state.Subscriptions = @($sub1.Id, $sub2.Id, $sub3.Id)

# === 输出处理：过滤 cargo 输出，只显示关键信息 ===
function Show-CargoOutput {
    param($output)
    foreach ($line in $output) {
        $lineStr = "$line"
        if ($lineStr -match "^error") {
            Write-Host "  $lineStr" -ForegroundColor Red
        } elseif ($lineStr -match "^warning") {
            Write-Host "  $lineStr" -ForegroundColor Yellow
        } elseif ($lineStr -match "^Compiling") {
            Write-Host "  $lineStr" -ForegroundColor Cyan
        } elseif ($lineStr -match "^Finished") {
            Write-Host "  $lineStr" -ForegroundColor Green
        } elseif ($lineStr -match "test result:") {
            Write-Host "  $lineStr" -ForegroundColor White
        } elseif ($lineStr -match "^test .* FAILED") {
            Write-Host "  $lineStr" -ForegroundColor Red
        } elseif ($lineStr -match "^running \d+ test") {
            Write-Host "  $lineStr" -ForegroundColor White
        }
    }
}

# === 执行步骤 1：cargo check ===
function Invoke-CargoCheck {
    Write-Host ">> [1/3] cargo check --workspace ..." -ForegroundColor Blue
    $startTime = Get-Date
    Push-Location $projectRoot
    try {
        $output = cargo check --workspace 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $duration = ((Get-Date) - $startTime).TotalSeconds
    Show-CargoOutput $output
    if ($exitCode -eq 0) {
        Write-Host "  [OK] cargo check 通过 (${duration}s)" -ForegroundColor Green
        return $true
    } else {
        Write-Host "  [FAIL] cargo check 失败 (${duration}s)" -ForegroundColor Red
        return $false
    }
}

# === 执行步骤 2：cargo test ===
function Invoke-CargoTest {
    Write-Host ">> [2/3] cargo test --workspace --lib ..." -ForegroundColor Green
    $startTime = Get-Date
    Push-Location $projectRoot
    try {
        $output = cargo test --workspace --lib 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $duration = ((Get-Date) - $startTime).TotalSeconds
    Show-CargoOutput $output
    if ($exitCode -eq 0) {
        Write-Host "  [OK] cargo test 通过 (${duration}s)" -ForegroundColor Green
        return $true
    } else {
        Write-Host "  [FAIL] cargo test 失败 (${duration}s)" -ForegroundColor Red
        return $false
    }
}

# === 执行步骤 3：重新生成 CALL_GRAPH ===
function Invoke-Callgraph {
    Write-Host ">> [3/3] 重新生成 CALL_GRAPH.md ..." -ForegroundColor Magenta
    if (-not (Test-Path $callgraphToolDir)) {
        Write-Host "  [SKIP] CALL_GRAPH 工具目录不存在: $callgraphToolDir" -ForegroundColor Yellow
        return $false
    }
    $startTime = Get-Date
    Push-Location $callgraphToolDir
    try {
        $output = cargo run -- --project $projectRoot --output $callgraphOutput --format markdown --visibility pub-super --workspace --name "Nuzo Lang" 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $duration = ((Get-Date) - $startTime).TotalSeconds
    Show-CargoOutput $output
    if ($exitCode -eq 0) {
        Write-Host "  [OK] CALL_GRAPH.md 已生成 (${duration}s)" -ForegroundColor Green
        return $true
    } else {
        Write-Host "  [FAIL] CALL_GRAPH 生成失败 (${duration}s)" -ForegroundColor Red
        return $false
    }
}

# === 执行链：check -> test -> callgraph（错误不阻止后续步骤）===
function Invoke-FullPipeline {
    param($changeInfo)

    $now = Get-Date
    $timestamp = $now.ToString('HH:mm:ss')
    $fileRelative = $changeInfo.Path.Replace($projectRoot, "")

    Write-Host ""
    Write-Host "==================================================" -ForegroundColor DarkGray
    Write-Host "[$timestamp] 检测到变化 ($($changeInfo.Type)): $fileRelative" -ForegroundColor Yellow
    Write-Host "==================================================" -ForegroundColor DarkGray

    # 步骤 1: cargo check（始终执行）
    $null = Invoke-CargoCheck

    # Cargo.toml 变化只触发 check
    if ($changeInfo.IsCargoToml) {
        Write-Host "[INFO] Cargo.toml 变化，仅执行 check" -ForegroundColor DarkYellow
    } else {
        # 步骤 2: cargo test（错误不阻止后续）
        if (-not $SkipTest) {
            $null = Invoke-CargoTest
        } else {
            Write-Host "[SKIP] cargo test (-SkipTest)" -ForegroundColor DarkGray
        }

        # 步骤 3: CALL_GRAPH（错误不阻止后续）
        if (-not $SkipCallgraph) {
            $null = Invoke-Callgraph
        } else {
            Write-Host "[SKIP] CALL_GRAPH 生成 (-SkipCallgraph)" -ForegroundColor DarkGray
        }
    }

    $endNow = Get-Date
    $totalDuration = ($endNow - $now).TotalSeconds
    Write-Host "--------------------------------------------------" -ForegroundColor DarkGray
    Write-Host "[$($endNow.ToString('HH:mm:ss'))] 完成 (总耗时 ${totalDuration}s)，等待下一次变化..." -ForegroundColor DarkGray
}

# === 启动横幅 ===
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Nuzo Lang Watch Mode (Enhanced)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "  监听路径   : $watchPath" -ForegroundColor White
Write-Host "  项目根目录 : $projectRoot" -ForegroundColor White
Write-Host "  防抖延迟   : ${DebounceSeconds}s" -ForegroundColor White
Write-Host "  监听文件   : *.rs, Cargo.toml" -ForegroundColor White
Write-Host "  执行链     : check -> test -> callgraph" -ForegroundColor White
Write-Host ""
if ($SkipTest) { Write-Host "  [SKIP] test" -ForegroundColor DarkYellow }
if ($SkipCallgraph) { Write-Host "  [SKIP] callgraph" -ForegroundColor DarkYellow }
Write-Host ""
Write-Host "  按 Ctrl+C 停止" -ForegroundColor DarkGray
Write-Host ""

# === 主循环（防抖 + 处理，try/finally 确保 Ctrl+C 时清理资源）===
try {
    while ($true) {
        if ($global:state.PendingChanges.Count -gt 0) {
            # 取出所有待处理变化
            $changes = @()
            while ($global:state.PendingChanges.Count -gt 0) {
                $changes += $global:state.PendingChanges.Dequeue()
            }
            $lastChange = $changes[-1]

            # 防抖：等待 DebounceSeconds 秒
            Start-Sleep -Seconds $DebounceSeconds

            # 检查等待期间是否有新变化，如果有则继续等待（直到静默 DebounceSeconds 秒）
            while ($global:state.PendingChanges.Count -gt 0) {
                $newChanges = @()
                while ($global:state.PendingChanges.Count -gt 0) {
                    $newChanges += $global:state.PendingChanges.Dequeue()
                }
                $lastChange = $newChanges[-1]
                Start-Sleep -Seconds $DebounceSeconds
            }

            # 执行完整流水线
            Invoke-FullPipeline -changeInfo $lastChange
        }

        Start-Sleep -Milliseconds 200
    }
} finally {
    Stop-Watch
    Write-Host ""
    Write-Host "[INFO] Watch 已停止，资源已清理" -ForegroundColor Cyan
}
