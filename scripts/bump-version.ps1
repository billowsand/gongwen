param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("major", "minor", "patch")]
    [string]$Part
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $projectRoot "Cargo.toml"
$lockPath = Join-Path $projectRoot "Cargo.lock"
$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)

$manifest = [System.IO.File]::ReadAllText($manifestPath)
$versionMatch = [regex]::Match(
    $manifest,
    '(?m)^version\s*=\s*"(?<version>\d+\.\d+\.\d+)"'
)
if (-not $versionMatch.Success) {
    throw "Cannot read an x.x.x package.version from Cargo.toml."
}

$current = $versionMatch.Groups["version"].Value
$numbers = $current.Split('.') | ForEach-Object { [int]$_ }
switch ($Part) {
    "major" { $numbers = @(($numbers[0] + 1), 0, 0) }
    "minor" { $numbers = @($numbers[0], ($numbers[1] + 1), 0) }
    "patch" { $numbers = @($numbers[0], $numbers[1], ($numbers[2] + 1)) }
}
$next = $numbers -join '.'

$manifest = $manifest.Remove($versionMatch.Groups["version"].Index, $current.Length)
$manifest = $manifest.Insert($versionMatch.Groups["version"].Index, $next)
[System.IO.File]::WriteAllText($manifestPath, $manifest, $utf8WithoutBom)

$lock = [System.IO.File]::ReadAllText($lockPath)
$packagePattern = '(?ms)(\[\[package\]\]\r?\nname = "gongwen-assistant"\r?\nversion = ")\d+\.\d+\.\d+("\r?\n)'
if (-not [regex]::IsMatch($lock, $packagePattern)) {
    throw "Cannot find the gongwen-assistant version in Cargo.lock."
}
$lock = ([regex]::new($packagePattern)).Replace($lock, "`${1}$next`${2}", 1)
[System.IO.File]::WriteAllText($lockPath, $lock, $utf8WithoutBom)

Write-Host "Version bumped from $current to $next ($Part)."
