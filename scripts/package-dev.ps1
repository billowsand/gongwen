# 一键打包供本机安装测试的开发版安装包。
#
# 流程：cargo build --release --locked --features ime-dev-tables → package-portable
# 组装 dist\win-x64-full → package-installer 打出 setup.exe。版本号默认取 Cargo.toml
# 当前版本号的下一个补丁号并加 -dev 后缀（例如当前 0.6.2 → 0.6.3-dev），与正式发布区分开；
# 要指定版本就传 -Version。只重打包不重编译就传 -SkipBuild。
#
# 带 `ime-dev-tables` 是为了把 assets\fuma\danzi.txt（小鹤辅码）与 quan.txt（小鹤音形）
# 编进二进制、首次运行默认启用（不用手动导入）。**仅供本机自用**：这两份码表权利归
# 小鹤方案作者、无再分发授权，产出的安装包不要外传；正式发布（release.yml）与 CI 都不带。
param(
    [string]$Version = "",
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

if (-not $SkipBuild) {
    Push-Location $projectRoot
    try {
        cargo build --release --locked --features ime-dev-tables
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release --locked --features ime-dev-tables failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

# 两个子脚本自带 $ErrorActionPreference="Stop"，失败会以终止错误向上抛，
# 不需要（也不能）用 $LASTEXITCODE 检查它们的退出码——那个变量只对原生命令有效。
& (Join-Path $PSScriptRoot "package-portable.ps1") `
    -Suffix win-x64 -Force -SkipBuild `
    -OutputDir (Join-Path $projectRoot "dist\win-x64-full")

& (Join-Path $PSScriptRoot "package-installer.ps1") -Version $Version -Force
