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
Copy-Item -LiteralPath ([System.IO.Path]::Combine($projectRoot, "font", "licenses")) -Destination ([System.IO.Path]::Combine($OutputDir, "licenses", "fonts")) -Recurse

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
