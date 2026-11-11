# 一键打包供本机安装测试的开发版安装包。
#
# 流程：cargo build --release --locked → package-portable
# 组装 dist\win-x64-full → package-installer 打出 setup.exe。版本号默认取 Cargo.toml
# 当前版本号的下一个补丁号并加 -dev 后缀（例如当前 0.6.2 → 0.6.3-dev），与正式发布区分开；
# 要指定版本就传 -Version。只重打包不重编译就传 -SkipBuild。
#
# 基础词表由 package-portable 独立复制，修改词表无需重新编译。
param(
    [string]$Version = "",
    [ValidateSet("ai", "no-ai")]
    [string]$Edition = "ai",
    [string]$BinaryPath = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))

if ([string]::IsNullOrWhiteSpace($Version)) {
    $manifest = [System.IO.File]::ReadAllText((Join-Path $projectRoot "Cargo.toml"))
    $versionMatch = [regex]::Match($manifest, '(?m)^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"')
    if (-not $versionMatch.Success) {
        throw "Cannot read an x.x.x package.version from Cargo.toml."
    }
    $parts = $versionMatch.Groups["version"].Value.Split(".")
    $Version = "{0}.{1}.{2}-dev" -f $parts[0], $parts[1], ([int]$parts[2] + 1)
}

$editionSuffix = if ($Edition -eq "ai") { "" } else { "-no-ai" }
$staging = Join-Path $projectRoot "dist/win-x64-full$editionSuffix"
& (Join-Path $PSScriptRoot "package-portable.ps1") `
    -Edition $Edition -BinaryPath $BinaryPath -Suffix win-x64 -Force -SkipBuild:$SkipBuild `
    -OutputDir $staging

& (Join-Path $PSScriptRoot "package-installer.ps1") -Edition $Edition -Version $Version -SourceDir $staging -Force
