# 公文助手 · Pi host 本地开发设置（PowerShell 5.1）
#
# 用法（在当前 PowerShell 窗口里 dot-source，环境变量只影响这个窗口）：
#   . .\scripts\pi-dev.ps1
#   . .\scripts\pi-dev.ps1 -Run       # 设置后直接 cargo run -- --new-window
#
# 只做三件事：校验 host 依赖已在本机、把 node 与 host 的绝对路径写进
# GONGWEN_PI_NODE / GONGWEN_PI_RUNTIME_DIR、提示下一步。
# 不执行 npm install，不下载任何东西，不写 PATH 或用户级环境变量。

param(
    [string]$RuntimeDir = "",
    [string]$Node = "",
    [switch]$Run
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$hostSource = Join-Path $repo "pi-host"
$modules = Join-Path $hostSource "node_modules"
$hostEntry = Join-Path $hostSource "src\main.mjs"

if (-not (Test-Path -LiteralPath $hostEntry)) {
    throw "找不到 host 入口：$hostEntry"
}
if (-not (Test-Path -LiteralPath $modules)) {
    throw "缺少 pi-host/node_modules。请先在本机（联网构建阶段）到 pi-host 目录执行 npm install --cache ..\tmp\pi-implementation\npm-cache；本脚本不会自动下载依赖。"
}

if ([string]::IsNullOrWhiteSpace($RuntimeDir)) {
    $RuntimeDir = Join-Path $repo "tmp\pi-runtime"
}
$RuntimeDir = [System.IO.Path]::GetFullPath($RuntimeDir)
if (-not [System.IO.Path]::IsPathRooted($RuntimeDir)) {
    throw "GONGWEN_PI_RUNTIME_DIR 必须是绝对路径：$RuntimeDir"
}
New-Item -ItemType Directory -Force -Path $RuntimeDir | Out-Null
$hostLink = Join-Path $RuntimeDir "host"
if (-not (Test-Path -LiteralPath $hostLink)) {
    # 目录 junction：host/main.mjs 的真实位置仍是 pi-host/src，Node 能找到
    # pi-host/node_modules，不需要复制依赖。
    $linkArg = '"' + $hostLink + '"';
    $targetArg = '"' + (Join-Path $hostSource 'src') + '"';
    & cmd /c mklink /J $linkArg $targetArg | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "创建 host junction 失败：$hostLink"
    }
}

if ([string]::IsNullOrWhiteSpace($Node)) {
    $command = Get-Command node.exe -ErrorAction SilentlyContinue
    if ($null -eq $command) {
        $command = Get-Command node -ErrorAction SilentlyContinue
    }
    if ($null -eq $command) {
        throw "找不到 Node。请安装 Node >= 22.19，或用 -Node 指定 node 可执行文件的绝对路径。"
    }
    $Node = $command.Source
}
$Node = [System.IO.Path]::GetFullPath($Node)
if (-not (Test-Path -LiteralPath $Node)) {
    throw "Node 不存在：$Node"
}
$versionText = (& $Node --version).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Node 版本检查失败：$Node"
}
$versionText = $versionText.TrimStart("v")
$parts = $versionText.Split(".")
if ([int]$parts[0] -lt 22 -or ([int]$parts[0] -eq 22 -and [int]$parts[1] -lt 19)) {
    throw "Pi host 需要 Node >= 22.19.0，当前是 $versionText。"
}

$env:GONGWEN_PI_RUNTIME_DIR = $RuntimeDir
$env:GONGWEN_PI_NODE = $Node

Write-Host "Pi 开发运行时已设置（仅当前 PowerShell 窗口）："
Write-Host "  GONGWEN_PI_RUNTIME_DIR = $RuntimeDir"
Write-Host "  GONGWEN_PI_NODE        = $Node"
Write-Host "  host 入口              = $(Join-Path $hostLink 'main.mjs')"
Write-Host ""
Write-Host "下一步：cargo run -- --new-window"
Write-Host "应用使用本机已配置的 OpenAI 兼容模型；不会读取 Pi 个人配置，也不会联网安装依赖。"

if ($Run) {
    Push-Location $repo
    try {
        & cargo run -- --new-window
    } finally {
        Pop-Location
    }
}