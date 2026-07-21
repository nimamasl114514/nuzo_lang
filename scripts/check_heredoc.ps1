# check_heredoc.ps1 — 拦截 commit message 中的 heredoc 标记
# 用法: powershell -ExecutionPolicy Bypass -File check_heredoc.ps1 <commit-msg-file>
# 返回 0 通过；1 拒绝（检测到 heredoc）；2 参数/文件异常
#
# 背景: PowerShell 不支持 heredoc（<<EOF / <<'EOF' / <<-EOF 等）。
#       这些标记出现在 commit message 中几乎一定是误用
#       （典型场景：在 PowerShell 里执行 git commit -m "$(cat <<EOF ...)"）。
#       本脚本把"规则约束"升级为"工具强制"，对齐 Enforcement Pyramid 第二层。

# 编码处理：彻底避免中文乱码
chcp 65001 >$null
$OutputEncoding = [Console]::OutputEncoding = [Text.Encoding]::UTF8

# 校验参数
if ($args.Count -lt 1) {
    Write-Host "ERROR: 缺少 commit message 文件路径参数"
    Write-Host "用法: powershell -File check_heredoc.ps1 <commit-msg-file>"
    exit 2
}

$msgFile = $args[0]
if (-not (Test-Path -LiteralPath $msgFile)) {
    Write-Host "ERROR: commit message 文件不存在: $msgFile"
    exit 2
}

# 读取 commit message，忽略 git 自动加的 # 注释行
$lines = Get-Content -LiteralPath $msgFile -Encoding UTF8 |
    Where-Object { -not $_.StartsWith('#') }
$msg = ($lines -join "`n")

# heredoc 标记正则：
#   <<              起始
#   -?              可选缩进修饰符（<<-EOF）
#   \\?             可选转义反斜杠（<<\EOF、<<-\EOF）
#   (''|")?         可选引号（单引号/双引号）
#   [A-Z][A-Z0-9_]* 标识符（大写字母开头，常见 EOF/EOL/END/HEREDOC 等）
#   (''|")?         可选闭合引号
# 注: PowerShell 单引号字符串里 '' 表示一个字面单引号
$pattern = '<<-?\\?(?:''|"|)[A-Z][A-Z0-9_]*(?:''|"|)'

$hits = [regex]::Matches($msg, $pattern)

if ($hits.Count -gt 0) {
    Write-Host "ERROR: 检测到 heredoc 标记，已拒绝本次 commit"
    Write-Host ""
    Write-Host "原因: PowerShell 不支持 heredoc 语法（<<EOF / <<'EOF' / <<-EOF / <<\EOF 等）。"
    Write-Host "      这些标记出现在 commit message 中几乎一定是误用，"
    Write-Host "      实际 commit message 会包含字面的 <<EOF 文本而非多行内容。"
    Write-Host ""
    Write-Host "检测到的标记:"
    foreach ($m in $hits) {
        Write-Host "  - $($m.Value)"
    }
    Write-Host ""
    Write-Host "正确做法（任选其一）:"
    Write-Host "  1) 用多个 -m 参数叠加（推荐）:"
    Write-Host "       git commit -m 'feat: 修复 XXX' -m '详细描述第一段' -m '详细描述第二段'"
    Write-Host "  2) 写临时文件用 -F 指定:"
    Write-Host "       git commit -F commit_msg.txt"
    Write-Host ""
    Write-Host "如确需跳过此检查（不推荐）: git commit --no-verify"
    exit 1
}

Write-Host "OK: 未检测到 heredoc 标记"
exit 0
