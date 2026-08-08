param(
    [string]$OutputDir = "",
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$runtimeRoot = Join-Path $projectRoot "runtime"
$checksumFile = Join-Path $runtimeRoot "SHA256SUMS.txt"

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $projectRoot "dist\gongwen-assistant-win-x64"
}
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

# Overwrite removes the output recursively, so protect drive and project roots.
$protectedPaths = @(
    [System.IO.Path]::GetPathRoot($OutputDir),
    $projectRoot,
    $runtimeRoot,
    (Join-Path $projectRoot ".git"),
    (Join-Path $projectRoot "src"),
    (Join-Path $projectRoot "scripts"),
    (Join-Path $projectRoot "font"),
    (Join-Path $projectRoot "target")
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

if (-not (Test-Path -LiteralPath $checksumFile -PathType Leaf)) {
    throw "Missing runtime checksum manifest: $checksumFile"
}

foreach ($line in Get-Content -LiteralPath $checksumFile -Encoding UTF8) {
    if ([string]::IsNullOrWhiteSpace($line)) {
        continue
    }
    $parts = $line -split "\s+", 2
    if ($parts.Count -ne 2) {
        throw "Invalid checksum line: $line"
    }
    $expected = $parts[0].ToUpperInvariant()
    $relative = $parts[1].Trim().Replace("/", [System.IO.Path]::DirectorySeparatorChar)
    $asset = Join-Path $runtimeRoot $relative
    if (-not (Test-Path -LiteralPath $asset -PathType Leaf)) {
        throw "Missing portable runtime asset: $asset"
    }
    $actual = (Get-FileHash -LiteralPath $asset -Algorithm SHA256).Hash.ToUpperInvariant()
    if ($actual -ne $expected) {
        throw "SHA-256 mismatch for $asset`nexpected: $expected`nactual:   $actual"
    }
}

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
Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\gongwen-assistant.exe") -Destination $OutputDir
Copy-Item -LiteralPath $runtimeRoot -Destination (Join-Path $OutputDir "runtime") -Recurse
Copy-Item -LiteralPath (Join-Path $projectRoot "README.md") -Destination $OutputDir
Copy-Item -LiteralPath (Join-Path $projectRoot "THIRD_PARTY_NOTICES.md") -Destination $OutputDir

$outputPrefix = $OutputDir.TrimEnd("\", "/") + [System.IO.Path]::DirectorySeparatorChar
$manifest = Get-ChildItem -LiteralPath $OutputDir -Recurse -File |
    Sort-Object FullName |
    ForEach-Object {
        $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        $relative = $_.FullName.Substring($outputPrefix.Length).Replace("\", "/")
        "$hash  $relative"
    }
$manifest | Set-Content -LiteralPath (Join-Path $OutputDir "SHA256SUMS.txt") -Encoding UTF8

Write-Output "Portable package created: $OutputDir"
