# 组装随包 Pi 运行时（Windows x64）：固定 Node 22.22.3 + Pi 1.4.0 host + 生产依赖。
#
# 只做构建期工作：校验官方 Node 压缩包的 SHA256 后才解包；用该 Node 自带的 npm
# 跑 `npm ci --omit=dev --ignore-scripts --no-audit --no-fund`；最后写 manifest、
# 依赖许可证与 THIRD_PARTY 所需的清单。目标机不需要 npm / Python / 全局 Pi。
#
# 用法：
#   scripts/stage-pi-runtime.ps1 [-NodeArchive <zip>] [-OutputDir runtime/pi] [-Force]
#
# 离线组装：把官方压缩包放进缓存目录（默认 tmp/pi-implementation/downloads），
# 或传 -NodeArchive；仍然按 packaging/pi-runtime.json 的固定 SHA256 校验。
# 生产依赖只允许由该 Node 自带的 npm 跑 `npm ci`（可设 GONGWEN_PI_NPM_OFFLINE=1
# 用完整 npm 缓存离线装）；不接受把开发目录直接复制进安装包。

param(
    [string]$Platform = "win-x64",
    [string]$NodeArchive = "",
    [string]$OutputDir = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))

# Get-FileHash 是 PowerShell 4.0+ 的内置 cmdlet（位于 Microsoft.PowerShell.Utility）。
# 精简过的 Windows 镜像（只带 Utility 3.1.0.0）会缺它；用与 package-portable.ps1
# 相同的 .NET SHA256 兜底，输出格式完全一致（小写 64 位十六进制）。
if (-not (Get-Command -Name "Get-FileHash" -CommandType Cmdlet -ErrorAction SilentlyContinue)) {
    . (Join-Path $PSScriptRoot "package-portable.hash-polyfill.ps1")
}

$configPath = [System.IO.Path]::Combine($projectRoot, "packaging", "pi-runtime.json")
$config = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json

if ($Platform -ne "win-x64") {
    throw "Windows staging only builds win-x64; use scripts/stage-pi-runtime.sh for $Platform"
}
$platformConfig = $config.platforms.'win-x64'

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = [System.IO.Path]::Combine($projectRoot, "runtime", "pi")
}
$OutputDir = [System.IO.Path]::GetFullPath($OutputDir).TrimEnd("\", "/")
$runtimeRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($projectRoot, "runtime")).TrimEnd("\", "/")
$tmpRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine($projectRoot, "tmp")).TrimEnd("\", "/")

# 严格子目录：不允许等于 runtime / tmp 根本身，也不允许 runtime-evil / tmp2 这类前缀串。
function Test-StrictChild([string]$parent, [string]$child) {
    $prefix = $parent + [System.IO.Path]::DirectorySeparatorChar
    return $child.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
}
if (-not (Test-StrictChild $runtimeRoot $OutputDir) -and
    -not (Test-StrictChild $tmpRoot $OutputDir)) {
    throw "Refusing to stage Pi runtime outside a strict child of runtime/ or tmp/: $OutputDir"
}
# 路径上任何已有的 junction / 符号链接都拒绝：删除或写入不能穿过 reparse point。
$cursor = $OutputDir
while (-not [string]::IsNullOrEmpty($cursor)) {
    if (Test-Path -LiteralPath $cursor) {
        $item = Get-Item -LiteralPath $cursor -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing to use a path that contains a reparse point: $cursor"
        }
    }
    $parent = [System.IO.Path]::GetDirectoryName($cursor)
    if ([string]::IsNullOrEmpty($parent) -or $parent -eq $cursor) { break }
    $cursor = $parent
}

# 官方压缩包：显式参数 → 缓存目录 → 下载。任何来源都必须先校验 SHA256。
$cacheDirs = @()
if (-not [string]::IsNullOrWhiteSpace($env:GONGWEN_PI_NODE_CACHE)) {
    $cacheDirs += [System.IO.Path]::GetFullPath($env:GONGWEN_PI_NODE_CACHE)
}
$cacheDirs += [System.IO.Path]::Combine($projectRoot, "tmp", "pi-implementation", "downloads")
$cacheDirs += [System.IO.Path]::Combine($projectRoot, "tmp", "pi-runtime-cache")

$archive = $NodeArchive
if ([string]::IsNullOrWhiteSpace($archive)) {
    foreach ($dir in $cacheDirs) {
        $candidate = [System.IO.Path]::Combine($dir, $platformConfig.file)
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $archive = $candidate
            break
        }
    }
}
if ([string]::IsNullOrWhiteSpace($archive)) {
    if ($env:GONGWEN_PI_OFFLINE -eq "1") {
        throw "离线模式缺少 Node 压缩包：$($platformConfig.file)。请放进缓存目录或传 -NodeArchive。"
    }
    $downloadDir = $cacheDirs[0]
    New-Item -ItemType Directory -Force -Path $downloadDir | Out-Null
    $archive = [System.IO.Path]::Combine($downloadDir, $platformConfig.file)
    Write-Output "Downloading $($config.downloadBase)$($platformConfig.file)"
    Invoke-WebRequest -Uri ($config.downloadBase + $platformConfig.file) -OutFile $archive -UseBasicParsing
}
$archive = [System.IO.Path]::GetFullPath($archive)
if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    throw "Node archive not found: $archive"
}
$actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne ([string]$platformConfig.sha256).ToLowerInvariant()) {
    throw "Node archive SHA-256 mismatch: $archive`nexpected: $($platformConfig.sha256)`nactual:   $actual"
}

if (Test-Path -LiteralPath $OutputDir) {
    $manifest = [System.IO.Path]::Combine($OutputDir, "manifest.json")
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
        throw "Refusing to replace a directory that was not produced by this script: $OutputDir"
    }
    if (-not $Force) {
        throw "Pi runtime already staged at $OutputDir; pass -Force to replace it"
    }
    Remove-Item -LiteralPath $OutputDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null

$temp = [System.IO.Path]::Combine([System.IO.Path]::GetTempPath(), "gongwen-pi-stage-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $temp | Out-Null
try {
    Expand-Archive -LiteralPath $archive -DestinationPath $temp -Force
    $nodeRoot = Get-ChildItem -LiteralPath $temp -Directory | Select-Object -First 1
    if ($null -eq $nodeRoot) {
        throw "Node archive did not contain a top-level directory"
    }
    $nodeExe = [System.IO.Path]::Combine($nodeRoot.FullName, "node.exe")
    if (-not (Test-Path -LiteralPath $nodeExe -PathType Leaf)) {
        throw "node.exe missing in $($nodeRoot.FullName)"
    }
    $nodeVersion = (& $nodeExe --version).Trim()
    if ($nodeVersion -ne "v$($config.node)") {
        throw "Bundled Node reports $nodeVersion, expected v$($config.node)"
    }
    Copy-Item -LiteralPath $nodeExe -Destination ([System.IO.Path]::Combine($OutputDir, "node.exe"))
    $nodeLicense = [System.IO.Path]::Combine($nodeRoot.FullName, "LICENSE")
    New-Item -ItemType Directory -Force -Path ([System.IO.Path]::Combine($OutputDir, "licenses")) | Out-Null
    if (Test-Path -LiteralPath $nodeLicense -PathType Leaf) {
        Copy-Item -LiteralPath $nodeLicense -Destination ([System.IO.Path]::Combine($OutputDir, "licenses", "LICENSE.node"))
    }
    else {
        throw "Node LICENSE missing in the official archive"
    }

    $hostOut = [System.IO.Path]::Combine($OutputDir, "host")
    New-Item -ItemType Directory -Force -Path $hostOut | Out-Null
    $modulesOut = [System.IO.Path]::Combine($hostOut, "node_modules")
    $hostBuild = [System.IO.Path]::Combine($temp, "hostbuild")
    New-Item -ItemType Directory -Force -Path $hostBuild | Out-Null
    Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "pi-host", "package.json")) -Destination $hostBuild
    Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "pi-host", "package-lock.json")) -Destination $hostBuild
    $npmCli = [System.IO.Path]::Combine($nodeRoot.FullName, "node_modules", "npm", "bin", "npm-cli.js")
    if (-not (Test-Path -LiteralPath $npmCli -PathType Leaf)) {
        throw "Bundled npm not found: $npmCli"
    }
    Push-Location $hostBuild
    try {
        $npmArgs = @("ci", "--omit=dev", "--ignore-scripts", "--no-audit", "--no-fund")
        if ($env:GONGWEN_PI_NPM_OFFLINE -eq "1") {
            $npmArgs += "--offline"
        }
        & $nodeExe $npmCli @npmArgs
        if ($LASTEXITCODE -ne 0) {
            throw "npm ci failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
    Copy-Item -LiteralPath ([System.IO.Path]::Combine($hostBuild, "node_modules")) -Destination $modulesOut -Recurse

    # 校验生产依赖版本与 lockfile：不能把开发目录或错版本的包打进安装包。
    foreach ($name in $config.pi.PSObject.Properties.Name) {
        $pkgPath = [System.IO.Path]::Combine($modulesOut, $name.Replace("/", [System.IO.Path]::DirectorySeparatorChar), "package.json")
        if (-not (Test-Path -LiteralPath $pkgPath -PathType Leaf)) {
            throw "Missing production dependency: $name"
        }
        $pkg = Get-Content -LiteralPath $pkgPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($pkg.version -ne $config.pi.$name) {
            throw "Dependency $name is $($pkg.version), config requires $($config.pi.$name)"
        }
    }

    Get-ChildItem -LiteralPath ([System.IO.Path]::Combine($projectRoot, "pi-host", "src")) -Filter "*.mjs" |
        Copy-Item -Destination $hostOut
    $sourcePackage = Get-Content -LiteralPath ([System.IO.Path]::Combine($projectRoot, "pi-host", "package.json")) -Raw -Encoding UTF8 | ConvertFrom-Json
    $runtimePackage = [ordered]@{
        name = $sourcePackage.name
        version = $sourcePackage.version
        private = $true
        type = "module"
        main = "main.mjs"
        engines = $sourcePackage.engines
        dependencies = $sourcePackage.dependencies
    }
    $runtimePackageJson = $runtimePackage | ConvertTo-Json -Depth 6
    [System.IO.File]::WriteAllText(
        [System.IO.Path]::Combine($hostOut, "package.json"),
        $runtimePackageJson + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )

    & $nodeExe ([System.IO.Path]::Combine($projectRoot, "scripts", "pi-runtime-licenses.mjs")) $OutputDir
    if ($LASTEXITCODE -ne 0) {
        throw "Collecting dependency licenses failed"
    }
    & $nodeExe ([System.IO.Path]::Combine($projectRoot, "scripts", "pi-runtime-manifest.mjs")) $OutputDir $Platform $configPath
    if ($LASTEXITCODE -ne 0) {
        throw "Writing Pi runtime manifest failed"
    }

    & (Join-Path $PSScriptRoot "verify-pi-runtime.ps1") -RuntimeDir $OutputDir -Platform $Platform
}
finally {
    if (Test-Path -LiteralPath $temp) {
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Output "Pi runtime staged at $OutputDir"
