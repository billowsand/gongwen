param(
    [string]$Suffix = "",
    [string]$BinaryName = "",
    [string]$RuntimeManifest = "",
    [string]$OutputDir = "",
    [ValidateSet("none", "zip", "tar.gz")]
    [string]$ArchiveFormat = "none",
    [string]$ArchivePath = "",
    [switch]$Force,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$runtimeRoot = [System.IO.Path]::Combine($projectRoot, "runtime")
$isWindowsHost = $env:OS -eq "Windows_NT"

# Get-FileHash 是 PowerShell 4.0+ 的内置 cmdlet（位于 Microsoft.PowerShell.Utility）。
# 极少数精简过的 Windows 镜像（特别是 LTSC 去掉 PSReadLine + 删了一些内置 cmdlet 的
# 镜像）会缺它；用 .NET 的 SHA256 兜底，输出格式与 `Get-FileHash -Algorithm SHA256`
# 完全一致（小写 64 位十六进制）。PowerShell 5.1 不允许在 `if` 块内用 `function`
# 关键字定义 advanced function（语法解析报错），所以 polyfill 放到独立 .ps1 文件，
# dot-source 进来一次性声明完整函数。
if (-not (Get-Command -Name "Get-FileHash" -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot "package-portable.hash-polyfill.ps1")
}

if ([string]::IsNullOrWhiteSpace($Suffix)) {
    $Suffix = if ($isWindowsHost) { "win-x64" } else { "linux-arm64" }
}
if ([string]::IsNullOrWhiteSpace($BinaryName)) {
    $BinaryName = if ($Suffix -eq "win-x64") { "gongwen-assistant.exe" } else { "gongwen-assistant" }
}
if ([string]::IsNullOrWhiteSpace($RuntimeManifest)) {
    $RuntimeManifest = [System.IO.Path]::Combine($runtimeRoot, "SHA256SUMS.$Suffix.txt")
    if (-not (Test-Path -LiteralPath $RuntimeManifest -PathType Leaf)) {
        $RuntimeManifest = [System.IO.Path]::Combine($runtimeRoot, "SHA256SUMS.txt")
    }
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = [System.IO.Path]::Combine($projectRoot, "dist", "gongwen-assistant-$Suffix")
}
$RuntimeManifest = [System.IO.Path]::GetFullPath($RuntimeManifest)
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)

function Test-SamePath([string]$Left, [string]$Right) {
    return [string]::Equals(
        $Left.TrimEnd("\", "/"),
        $Right.TrimEnd("\", "/"),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Assert-DirectoryNotInUse([string]$Path) {
    foreach ($file in Get-ChildItem -LiteralPath $Path -Recurse -File) {
        $stream = $null
        try {
            $stream = [System.IO.File]::Open(
                $file.FullName,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::None
            )
        }
        catch {
            throw "Cannot overwrite the portable directory because a file is in use: $($file.FullName). Close the running application and try again."
        }
        finally {
            if ($null -ne $stream) {
                $stream.Dispose()
            }
        }
    }
}

function Get-RuntimeDestination([string]$Relative, [string]$PlatformSuffix) {
    $normalized = $Relative.Replace("/", [System.IO.Path]::DirectorySeparatorChar)
    $platformPrefix = [System.IO.Path]::Combine("tectonic", $PlatformSuffix) + [System.IO.Path]::DirectorySeparatorChar
    if ($normalized.StartsWith($platformPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $leaf = [System.IO.Path]::GetFileName($normalized)
        return [System.IO.Path]::Combine("tectonic", $leaf)
    }
    return $normalized
}

# Overwrite removes the output recursively, so protect drive and project roots.
$protectedPaths = @(
    [System.IO.Path]::GetPathRoot($OutputDir),
    $projectRoot,
    $runtimeRoot,
    [System.IO.Path]::Combine($projectRoot, ".git"),
    [System.IO.Path]::Combine($projectRoot, "src"),
    [System.IO.Path]::Combine($projectRoot, "scripts"),
    [System.IO.Path]::Combine($projectRoot, "font"),
    [System.IO.Path]::Combine($projectRoot, "target")
)
foreach ($protectedPath in $protectedPaths) {
    if (Test-SamePath $OutputDir ([System.IO.Path]::GetFullPath($protectedPath))) {
        throw "Refusing to use a protected directory as portable output: $OutputDir"
    }
}

$overwriteExisting = $false
if (Test-Path -LiteralPath $OutputDir) {
    if (-not (Test-Path -LiteralPath $OutputDir -PathType Container)) {
        throw "Portable output path exists but is not a directory: $OutputDir"
    }

    if ($Force) {
        $overwriteExisting = $true
    }
    else {
        while ($true) {
            $answer = Read-Host "Output directory already exists: $OutputDir`nOverwrite it? [Y/n]"
            switch ($answer.Trim().ToLowerInvariant()) {
                { $_ -in @("", "y", "yes") } {
                    $overwriteExisting = $true
                    break
                }
                { $_ -in @("n", "no") } {
                    Write-Output "Packaging cancelled; existing directory was not changed: $OutputDir"
                    return
                }
                default {
                    Write-Host "Enter Y or N; pressing Enter defaults to Y."
                }
            }
            if ($overwriteExisting) {
                break
            }
        }
    }
}

if (-not (Test-Path -LiteralPath $RuntimeManifest -PathType Leaf)) {
    throw "Missing runtime checksum manifest: $RuntimeManifest"
}

$runtimeEntries = @()
foreach ($line in Get-Content -LiteralPath $RuntimeManifest -Encoding UTF8) {
    $trimmed = $line.Trim()
    if ([string]::IsNullOrWhiteSpace($trimmed) -or $trimmed.StartsWith("#")) {
        continue
    }
    $parts = $trimmed -split "\s+", 2
    if ($parts.Count -ne 2) {
        throw "Invalid checksum line: $line"
    }
    $expected = $parts[0].ToUpperInvariant()
    $relative = $parts[1].Trim().Replace("/", [System.IO.Path]::DirectorySeparatorChar)
    $asset = [System.IO.Path]::Combine($runtimeRoot, $relative)
    if (-not (Test-Path -LiteralPath $asset -PathType Leaf)) {
        throw "Missing portable runtime asset: $asset"
    }
    $actual = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 mismatch for $asset`nexpected: $expected`nactual:   $actual"
    }
    $runtimeEntries += [PSCustomObject]@{
        Source = $asset
        Destination = [System.IO.Path]::Combine("runtime", (Get-RuntimeDestination $relative $Suffix))
    }
}

if (-not $SkipBuild) {
    Push-Location $projectRoot
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

$binarySource = [System.IO.Path]::Combine($projectRoot, "target", "release", $BinaryName)
if (-not (Test-Path -LiteralPath $binarySource -PathType Leaf)) {
    throw "Built binary not found: $binarySource"
}

if ($overwriteExisting) {
    Assert-DirectoryNotInUse $OutputDir
    try {
        Remove-Item -LiteralPath $OutputDir -Recurse -Force
    }
    catch {
        throw "Unable to replace portable output directory: $OutputDir. $($_.Exception.Message)"
    }
}

New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
Copy-Item -LiteralPath $binarySource -Destination (Join-Path $OutputDir $BinaryName)

foreach ($entry in $runtimeEntries) {
    $destination = Join-Path $OutputDir $entry.Destination
    $destinationDirectory = Split-Path -Parent $destination
    New-Item -ItemType Directory -Force -Path $destinationDirectory | Out-Null
    Copy-Item -LiteralPath $entry.Source -Destination $destination
}

Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "README.md")) -Destination $OutputDir
Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "THIRD_PARTY_NOTICES.md")) -Destination $OutputDir
Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "LICENSE")) -Destination $OutputDir
Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "config.example.json")) -Destination $OutputDir
# AI 技能包：把 skills/gongwen-markdown/ 打成 skills/gongwen-markdown.skill 随包分发。
# .skill 就是 zip，顶层一个 gongwen-markdown/ 文件夹。条目名手工拼成正斜杠——
# Compress-Archive 在 PowerShell 5.1 上会写反斜杠，别的工具解压会得到一个怪文件名。
# 程序里的「导出 AI 技能包」按钮用的是编进二进制的同一套文件（src/skill_pack.rs）。
$skillName = "gongwen-markdown"
$skillSource = [System.IO.Path]::Combine($projectRoot, "skills", $skillName)
if (-not (Test-Path -LiteralPath (Join-Path $skillSource "SKILL.md") -PathType Leaf)) {
    throw "Skill source not found: $skillSource"
}
$skillOutputDir = Join-Path $OutputDir "skills"
New-Item -ItemType Directory -Force -Path $skillOutputDir | Out-Null
$skillArchive = Join-Path $skillOutputDir "$skillName.skill"
if (Test-Path -LiteralPath $skillArchive) {
    Remove-Item -LiteralPath $skillArchive -Force
}
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$skillZip = [System.IO.Compression.ZipFile]::Open($skillArchive, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    $skillPrefix = $skillSource.TrimEnd("\", "/").Length + 1
    foreach ($file in Get-ChildItem -LiteralPath $skillSource -Recurse -File | Where-Object { $_.FullName -notmatch "__pycache__" } | Sort-Object FullName) {
        $entryName = "$skillName/" + $file.FullName.Substring($skillPrefix).Replace("\", "/")
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($skillZip, $file.FullName, $entryName, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
}
finally {
    $skillZip.Dispose()
}
if (-not $isWindowsHost) {
    foreach ($executable in @($BinaryName, [System.IO.Path]::Combine("runtime", "tectonic", "tectonic"))) {
        $executablePath = Join-Path $OutputDir $executable
        if (Test-Path -LiteralPath $executablePath) {
            & chmod +x $executablePath
            if ($LASTEXITCODE -ne 0) {
                throw "chmod +x failed: $executablePath"
            }
        }
    }
}

$outputPrefix = $OutputDir.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
$manifest = Get-ChildItem -LiteralPath $OutputDir -Recurse -File |
    Sort-Object FullName |
    ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        $relative = $_.FullName.Substring($outputPrefix.Length).Replace("\", "/")
        "$hash  $relative"
    }
$manifest | Set-Content -LiteralPath (Join-Path $OutputDir "SHA256SUMS.txt") -Encoding UTF8

if ($ArchiveFormat -ne "none") {
    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        $extension = if ($ArchiveFormat -eq "tar.gz") { "tar.gz" } else { "zip" }
        $ArchivePath = "$OutputDir.$extension"
    }
    $ArchivePath = [System.IO.Path]::GetFullPath($ArchivePath)
    $archiveDirectory = Split-Path -Parent $ArchivePath
    New-Item -ItemType Directory -Force -Path $archiveDirectory | Out-Null
    if (Test-Path -LiteralPath $ArchivePath) {
        Remove-Item -LiteralPath $ArchivePath -Force
    }
    if ($ArchiveFormat -eq "zip") {
        Compress-Archive -Path (Join-Path $OutputDir "*") -DestinationPath $ArchivePath -CompressionLevel Optimal
        if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
            throw "Compress-Archive did not create: $ArchivePath"
        }
    }
    else {
        Push-Location $OutputDir
        try {
            & tar -czf $ArchivePath .
            if ($LASTEXITCODE -ne 0) {
                throw "tar -czf failed with exit code $LASTEXITCODE"
            }
        }
        finally {
            Pop-Location
        }
    }
    Write-Output "Portable archive created: $ArchivePath"
}
else {
    Write-Output "Portable package created: $OutputDir"
}
