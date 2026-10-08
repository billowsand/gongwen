param(
    [string]$Version = "",
    [string]$SourceDir = "",
    [string]$OutputDir = "",
    [string]$CompilerPath = "",
    # 增量升级包：只携带变化的文件，输出名带 -upgrade-setup，
    # 且不检查字体等资源是否齐全（那些没变化的文件不在包里）。
    [switch]$Upgrade,
    # 升级包安装前要删除的过时文件清单（Inno 的 [InstallDelete] 片段），由
    # package-upgrade.ps1 生成。
    [string]$StaleListInclude = "",
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

if (-not $Upgrade) {
    # 升级包只携带变化的文件，"必带字体" 这类检查对它没有意义。
    $requiredFiles = @(
        "gongwen-assistant.exe",
        "runtime\fonts\GWFangSong.ttf",
        "runtime\fonts\GWKai.ttf",
        "runtime\fonts\FZHei.ttf",
        "runtime\fonts\FZShuSong.ttf",
        "runtime\fonts\XiaoBiaoSong.ttf",
        "runtime\fonts\GWSimSunLatin.ttf",
        "runtime\fonts\JetBrainsMono-Regular.ttf",
        "runtime\fonts\texgyretermes-regular.otf",
        "runtime\fonts\texgyretermes-bold.otf",
        "runtime\fonts\texgyretermes-italic.otf",
        "runtime\fonts\texgyretermes-bolditalic.otf"
    )
    foreach ($relative in $requiredFiles) {
        $requiredPath = Join-Path $SourceDir $relative
        if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
            throw "Missing portable package file: $requiredPath"
        }
    }
}
elseif ([string]::IsNullOrWhiteSpace($StaleListInclude)) {
    throw "Upgrade packages need -StaleListInclude; run scripts/package-upgrade.ps1 instead."
}

if ([string]::IsNullOrWhiteSpace($CompilerPath)) {
    $CompilerPath = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty Source
}
if ([string]::IsNullOrWhiteSpace($CompilerPath)) {
$candidates = @()
    if (-not [string]::IsNullOrWhiteSpace(${env:ProgramFiles(x86)})) {
        $candidates += Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
        $candidates += Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\app\ISCC.exe"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramFiles)) {
        $candidates += Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"
        $candidates += Join-Path $env:ProgramFiles "Inno Setup 6\app\ISCC.exe"
    }
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $candidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"
        $candidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\app\ISCC.exe"
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

$packageKind = if ($Upgrade) { "-win-x64-upgrade-setup" } else { "-win-x64-setup" }
$outputExe = Join-Path $OutputDir "gongwen-assistant-$Version$packageKind.exe"
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
if ($Upgrade) {
    # ISPP 的 #include 把反斜杠当转义符，路径一律用正斜杠。
    $staleInclude = [System.IO.Path]::GetFullPath($StaleListInclude).Replace("\", "/")
    $defines += "/DMyUpgradeMode=1"
    $defines += "/DMyStaleListInclude=$staleInclude"
}
& $CompilerPath @defines $scriptPath
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $outputExe -PathType Leaf)) {
    throw "ISCC did not produce the expected installer: $outputExe"
}

Write-Output "Installer created: $outputExe"
