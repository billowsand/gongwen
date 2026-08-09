param(
    [string]$Version = "",
    [string]$SourceDir = "",
    [string]$OutputDir = "",
    [string]$CompilerPath = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$scriptPath = Join-Path $PSScriptRoot "gongwen-assistant.iss"
$iconPath = Join-Path $projectRoot "assets\app-icon\app-icon.ico"
$languageFile = Join-Path $PSScriptRoot "ChineseSimplified.isl"

if (-not (Test-Path -LiteralPath $iconPath -PathType Leaf)) {
    throw "App icon not found: $iconPath"
}
if (-not (Test-Path -LiteralPath $languageFile -PathType Leaf)) {
    throw "Chinese language file not found: $languageFile"
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $manifestPath = Join-Path $projectRoot "Cargo.toml"
    $manifest = [System.IO.File]::ReadAllText($manifestPath)
    $versionMatch = [regex]::Match(
        $manifest,
        '(?m)^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"'
    )
    if (-not $versionMatch.Success) {
        throw "Cannot read an x.x.x package.version from Cargo.toml."
    }
    $Version = $versionMatch.Groups["version"].Value
}

if ([string]::IsNullOrWhiteSpace($SourceDir)) {
    $SourceDir = Join-Path $projectRoot "dist\win-x64-full"
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $projectRoot "dist"
}
$SourceDir = [System.IO.Path]::GetFullPath($SourceDir)
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir)

if (-not (Test-Path -LiteralPath $SourceDir -PathType Container)) {
    throw "Portable package directory not found: $SourceDir"
}

$requiredFiles = @(
    "gongwen-assistant.exe",
    "runtime\tectonic\tectonic.exe",
    "runtime\texbundle\gongwen-texlive.ttb",
    "runtime\fonts\FangSong.ttf",
    "runtime\fonts\KaiTi.ttf",
    "runtime\fonts\SimHei.ttf",
    "runtime\fonts\SimSun.ttf",
    "runtime\fonts\XiaoBiaoSong.ttf"
)
foreach ($relative in $requiredFiles) {
    $requiredPath = Join-Path $SourceDir $relative
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Missing portable package file: $requiredPath"
    }
}

if ([string]::IsNullOrWhiteSpace($CompilerPath)) {
    $CompilerPath = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Source
}
if ([string]::IsNullOrWhiteSpace($CompilerPath)) {
    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidates += Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $candidates += Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $candidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\app\ISCC.exe"
        $candidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $CompilerPath = $candidate
            break
        }
    }
}
if ([string]::IsNullOrWhiteSpace($CompilerPath) -or -not (Test-Path -LiteralPath $CompilerPath -PathType Leaf)) {
    throw "Inno Setup compiler (ISCC.exe) not found. Install Inno Setup 6 or pass -CompilerPath."
}
$CompilerPath = [System.IO.Path]::GetFullPath($CompilerPath)

$outputExe = Join-Path $OutputDir "gongwen-assistant-$Version-win-x64-setup.exe"
if (Test-Path -LiteralPath $outputExe -PathType Leaf) {
    if (-not $Force) {
        throw "Installer already exists: $outputExe`nPass -Force to overwrite."
    }
    Remove-Item -LiteralPath $outputExe -Force
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$defines = @(
    "/DMyAppVersion=$Version",
    "/DMySourceDir=$SourceDir",
    "/DMyOutputDir=$OutputDir",
    "/DMyIconPath=$iconPath",
    "/DMyLanguageFile=$languageFile"
)
& $CompilerPath @defines $scriptPath
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $outputExe -PathType Leaf)) {
    throw "ISCC did not produce the expected installer: $outputExe"
}

Write-Output "Installer created: $outputExe"
