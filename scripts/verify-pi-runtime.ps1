# 校验随包 Pi 运行时（runtime/pi）的 manifest 与文件清单。
#
# 打包阶段必须调用它：manifest 缺失、平台不符、Node / Pi / host / 协议版本不对、
# 文件缺失或哈希不符都直接抛终止错误，绝不发出一个「只有按钮没有运行时」的包。
#
# 用法：
#   verify-pi-runtime.ps1 -RuntimeDir runtime/pi -Platform win-x64 -ListFiles
# `-ListFiles` 在成功时按行输出相对路径（正斜杠），供打包脚本复制。

param(
    [string]$RuntimeDir = "",
    [string]$Platform = "",
    [switch]$ListFiles
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))

if ([string]::IsNullOrWhiteSpace($RuntimeDir)) {
    $RuntimeDir = [System.IO.Path]::Combine($projectRoot, "runtime", "pi")
}
$RuntimeDir = [System.IO.Path]::GetFullPath($RuntimeDir)
if ([string]::IsNullOrWhiteSpace($Platform)) {
    # 本脚本只处理 Windows 打包（macOS / Linux 走 pi-runtime-verify.mjs）。
    $Platform = "win-x64"
}

$configPath = [System.IO.Path]::Combine($projectRoot, "packaging", "pi-runtime.json")
if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    throw "Missing Pi runtime config: $configPath"
}
$config = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $config.platforms.PSObject.Properties.Name.Contains($Platform)) {
    throw "Unsupported Pi runtime platform: $Platform (this script only validates win-x64; macOS / Linux use scripts/pi-runtime-verify.mjs)"
}
$platformConfig = $config.platforms.$Platform

$manifestPath = [System.IO.Path]::Combine($RuntimeDir, "manifest.json")
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Missing Pi runtime manifest: $manifestPath. 先运行 scripts/stage-pi-runtime.ps1（或 .sh）组装运行时。"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json

if ([int]$manifest.schema -ne 1) {
    throw "Pi runtime manifest schema is $($manifest.schema), expected 1"
}
if ($manifest.source -ne "npm-ci") {
    throw "Pi runtime was not produced by npm ci (source: $($manifest.source)); refuse to package it"
}
if ($manifest.platform -ne $Platform) {
    throw "Pi runtime was staged for $($manifest.platform), but this package is $Platform"
}
if ($manifest.node.version -ne $config.node) {
    throw "Pi runtime Node is $($manifest.node.version), config requires $($config.node)"
}
if ($manifest.node.sha256 -ne $platformConfig.sha256) {
    throw "Pi runtime Node archive hash does not match packaging/pi-runtime.json"
}
if ($manifest.host.version -ne $config.host.version) {
    throw "Pi host version is $($manifest.host.version), app expects $($config.host.version)"
}
if ([int]$manifest.host.protocol -ne [int]$config.host.protocol) {
    throw "Pi host protocol is $($manifest.host.protocol), app expects $($config.host.protocol)"
}
$lockfilePath = [System.IO.Path]::Combine($projectRoot, $config.host.lockfile)
$lockfileHash = (Get-FileHash -LiteralPath $lockfilePath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($manifest.host.lockfileSha256 -ne $lockfileHash) {
    throw "Pi runtime dependency lockfile is stale; rebuild the runtime"
}
foreach ($name in $config.pi.PSObject.Properties.Name) {
    $actual = $manifest.pi.$name
    if ($actual -ne $config.pi.$name) {
        throw "Pi package $name is $actual, config requires $($config.pi.$name)"
    }
}

$required = @(
    $platformConfig.binary,
    "host/main.mjs",
    "host/package.json",
    "host/node_modules/@earendil-works/pi-durable/package.json",
    "host/node_modules/@earendil-works/pi-ai/package.json",
    "host/node_modules/@earendil-works/chord/package.json",
    "licenses/LICENSE.node",
    "licenses/DEPENDENCIES.json"
)
$listed = @{}
foreach ($file in @($manifest.files)) {
    $relative = [string]$file.path
    if ([string]::IsNullOrWhiteSpace($relative) -or
        $relative.Contains("..") -or
        $relative.StartsWith("/") -or
        $relative.Contains(":") -or
        $relative.Contains([char]0) -or
        $relative.Contains("\")) {
        throw "Pi runtime manifest has an unsafe path: $relative"
    }
    $absolute = [System.IO.Path]::Combine($RuntimeDir, $relative.Replace("/", [System.IO.Path]::DirectorySeparatorChar))
    if (-not (Test-Path -LiteralPath $absolute -PathType Leaf)) {
        throw "Pi runtime file listed in manifest is missing: $absolute"
    }
    $info = Get-Item -LiteralPath $absolute
    if ($null -ne $file.bytes -and [int64]$file.bytes -ne [int64]$info.Length) {
        throw "Pi runtime file size mismatch: $absolute"
    }
    $actual = (Get-FileHash -LiteralPath $absolute -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne ([string]$file.sha256).ToLowerInvariant()) {
        throw "Pi runtime file hash mismatch: $absolute"
    }
    $listed[$relative] = $true
}
foreach ($relative in $required) {
    if (-not $listed.ContainsKey($relative)) {
        throw "Pi runtime manifest does not list required file: $relative"
    }
}
$nodePath = [System.IO.Path]::Combine($RuntimeDir, $platformConfig.binary.Replace("/", [System.IO.Path]::DirectorySeparatorChar))
$version = (& $nodePath --version).Trim()
if ($version -ne "v$($config.node)") {
    throw "Bundled Node reports $version, expected v$($config.node)"
}

if ($ListFiles) {
    $listed.Keys | Sort-Object | ForEach-Object { $_ }
}
Write-Host "Pi runtime verified: $Platform · Node $($config.node) · host $($config.host.version) · $($listed.Count) files"
