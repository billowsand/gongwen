# 增量升级包：只把与已安装版本不同的文件打进安装包。
#
# 完整包每次都要带上字体（runtime/fonts 约 40 MB 未压缩）与许可证；这些文件没改动的
# 时候重打一遍纯属浪费带宽和时间。本脚本按两份 SHA256SUMS.txt（基线 = 用户机器上
# 已安装的那份，新版 = 本次完整构建的那份）做差：
#
#   - 哈希不同的或基线里没有的文件 → 复制进暂存目录，随安装包安装（覆盖）；
#   - 基线里有、新版里没有的文件与目录 → 写成 Inno 的 [InstallDelete] 片段，
#     安装前删掉；
#   - 其余文件（字体等）一律不进包，安装时原样保留。
#
# 用法（先跑 package-portable 或 package-dev 得到新版完整目录）：
#   ./scripts/package-upgrade.ps1 -Version 0.9.3
#   ./scripts/package-upgrade.ps1 -Version 0.9.3 -SkipPortable   # 复用已有完整目录
#   ./scripts/package-upgrade.ps1 -Version 0.9.3 -BaselineDir D:\旧版本目录
#
# 基线默认取已安装目录 %LOCALAPPDATA%\Programs\GongwenAssistant 里的 SHA256SUMS.txt；
# 要给别的机器做包就传 -BaselineManifest 指向旧包的清单，或 -BaselineDir 指向旧包目录。
param(
    [string]$Version = "",
    # 新版本的完整包目录（package-portable.ps1 的产物），默认 dist\win-x64-full
    [string]$SourceDir = "",
    # 基线：旧版本清单或旧包目录；两者都不给就用本机已安装的那份
    [string]$BaselineManifest = "",
    [string]$BaselineDir = "",
    # 只复制暂存目录（与 ISCC 输出同名），不重新组装完整目录
    [switch]$SkipPortable,
    [switch]$SkipBuild,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))

if (-not (Get-Command -Name "Get-FileHash" -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot "package-portable.hash-polyfill.ps1")
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $manifest = [System.IO.File]::ReadAllText((Join-Path $projectRoot "Cargo.toml"))
    $versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"')
    if (-not $versionMatch.Success) {
        throw "Cannot read an x.x.x package.version from Cargo.toml."
    }
    $Version = $versionMatch.Groups["version"].Value
}

if ([string]::IsNullOrWhiteSpace($SourceDir)) {
    $SourceDir = Join-Path $projectRoot "dist\win-x64-full"
}
$SourceDir = [System.IO.Path]::GetFullPath($SourceDir)

# 读 SHA256SUMS.txt（"HASH  相对路径" 每行一条，# 开头是注释）成 hashtable。
function Read-ChecksumManifest([string]$Path) {
    $entries = @{}
    foreach ($line in Get-Content -LiteralPath $Path -Encoding UTF8) {
        $trimmed = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed.StartsWith("#")) {
            continue
        }
        $parts = $trimmed -split "\s+", 2
        if ($parts.Count -ne 2) {
            throw "Invalid checksum line in ${Path}: $line"
        }
        $relative = $parts[1].Trim().Replace("\", "/")
        $entries[$relative] = $parts[0].ToUpperInvariant()
    }
    if ($entries.Count -eq 0) {
        throw "Checksum manifest has no entries: $Path"
    }
    return $entries
}

# 基线定位：显式清单 > 显式目录 > 本机已安装目录。
if (-not [string]::IsNullOrWhiteSpace($BaselineManifest)) {
    $baselineManifestPath = [System.IO.Path]::GetFullPath($BaselineManifest)
}
elseif (-not [string]::IsNullOrWhiteSpace($BaselineDir)) {
    $baselineManifestPath = Join-Path ([System.IO.Path]::GetFullPath($BaselineDir)) "SHA256SUMS.txt"
}
else {
    $installedDir = if ($env:OS -eq "Windows_NT") {
        Join-Path $env:LOCALAPPDATA "Programs\GongwenAssistant"
    } else {
        Join-Path $env:HOME ".local/share/GongwenAssistant"
    }
    $baselineManifestPath = Join-Path $installedDir "SHA256SUMS.txt"
}
if (-not (Test-Path -LiteralPath $baselineManifestPath -PathType Leaf)) {
    throw "Baseline manifest not found: $baselineManifestPath`n" +
    "Pass -BaselineManifest (old package's SHA256SUMS.txt) or -BaselineDir (old package directory)."
}

# 新版完整目录：不存在（或显式要求重组装）就跑一次 package-portable。
if (-not $SkipPortable -or -not (Test-Path -LiteralPath (Join-Path $SourceDir "SHA256SUMS.txt") -PathType Leaf)) {
    $portableArgs = @{
        Suffix = "win-x64"
        Force = $true
        OutputDir = $SourceDir
    }
    if ($SkipBuild) { $portableArgs["SkipBuild"] = $true }
    & (Join-Path $PSScriptRoot "package-portable.ps1") @portableArgs
}

$newManifestPath = Join-Path $SourceDir "SHA256SUMS.txt"
if (-not (Test-Path -LiteralPath $newManifestPath -PathType Leaf)) {
    throw "New package manifest not found: $newManifestPath"
}

$baseline = Read-ChecksumManifest $baselineManifestPath
$current = Read-ChecksumManifest $newManifestPath

# SHA256SUMS.txt 不列在自己的清单里（自引用），所以差集要单独处理：它永远随包更新，
# 也永远不进删除清单——否则升级后机器上会留下一份描述旧版本的清单。
$manifestName = "SHA256SUMS.txt"

# 差集：变化 / 新增的文件，以及需要删掉的过时文件。
$changed = @($manifestName)
foreach ($relative in ($current.Keys | Sort-Object)) {
    if (-not $baseline.ContainsKey($relative) -or $baseline[$relative] -ne $current[$relative]) {
        $changed += $relative
    }
}
$removedFiles = @(
    $baseline.Keys |
        Where-Object { $_ -ne $manifestName -and -not $current.ContainsKey($_) } |
        Sort-Object
)
# 基线里有、新版里整个目录都没了的，连目录一起删（避免留下空壳目录）。
$removedDirs = @(
    $removedFiles |
        ForEach-Object { $index = $_.LastIndexOf("/"); if ($index -ge 0) { $_.Substring(0, $index) } } |
        Sort-Object -Unique |
        Where-Object { $dir = $_; -not ($baseline.Keys | Where-Object { $_.StartsWith("$dir/") }) } |
        Sort-Object
)

if ($changed.Count -eq 0 -and $removedFiles.Count -eq 0 -and $removedDirs.Count -eq 0) {
    Write-Output "Nothing to package: $newManifestPath is identical to $baselineManifestPath."
    return
}

# 暂存目录：只放变化的文件，保持相对路径，Inno 用同一份 [Files] 规则安装。
$stagingDir = Join-Path $projectRoot "dist\win-x64-upgrade-$Version"
if (Test-Path -LiteralPath $stagingDir) {
    if (-not $Force) {
        throw "Staging directory already exists: $stagingDir`nPass -Force to overwrite."
    }
    Remove-Item -LiteralPath $stagingDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

foreach ($relative in $changed) {
    $source = Join-Path $SourceDir $relative.Replace("/", "\")
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Listed in the new manifest but missing on disk: $source"
    }
    $destination = Join-Path $stagingDir $relative.Replace("/", "\")
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
    Copy-Item -LiteralPath $source -Destination $destination
}

# [InstallDelete] 片段：升级安装时先删掉这些，再覆盖其余文件。
$staleListPath = Join-Path $projectRoot "dist\win-x64-stale-$Version.iss"
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $staleListPath) | Out-Null
$staleLines = @("; Generated by package-upgrade.ps1: files present in the baseline but gone in the new build.")
foreach ($relative in $removedFiles) {
    $staleLines += "Type: files; Name: ""{app}\$($relative.Replace("/", "\"))"""
}
foreach ($relative in $removedDirs) {
    $staleLines += "Type: filesandordirs; Name: ""{app}\$($relative.Replace("/", "\"))"""
}
# Inno 的 #include 不吃 UTF-8 BOM，这里显式用无 BOM 的 UTF-8。
[System.IO.File]::WriteAllLines($staleListPath, $staleLines, (New-Object System.Text.UTF8Encoding($false)))

$stagedBytes = (Get-ChildItem -LiteralPath $stagingDir -Recurse -File |
    Measure-Object -Property Length -Sum).Sum
$fullBytes = (Get-ChildItem -LiteralPath $SourceDir -Recurse -File |
    Measure-Object -Property Length -Sum).Sum

Write-Output "Baseline : $baselineManifestPath ($($baseline.Count) files)"
Write-Output "New build: $newManifestPath ($($current.Count) files)"
Write-Output "Packaged : $($changed.Count) changed files, $([math]::Round($stagedBytes / 1MB, 1)) MB of $([math]::Round($fullBytes / 1MB, 1)) MB"
if ($removedFiles.Count -gt 0 -or $removedDirs.Count -gt 0) {
    Write-Output "Removed on install: $($removedFiles.Count) files, $($removedDirs.Count) directories"
}
foreach ($relative in $changed) {
    Write-Output "  + $relative"
}

& (Join-Path $PSScriptRoot "package-installer.ps1") `
    -Version $Version `
    -SourceDir $stagingDir `
    -Upgrade `
    -StaleListInclude $staleListPath `
    -Force

Remove-Item -LiteralPath $stagingDir -Recurse -Force
