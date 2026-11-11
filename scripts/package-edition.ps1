function Assert-PackageEdition([string]$Binary, [string]$Edition) {
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $Binary
    $info.Arguments = "--build-edition"
    $info.UseShellExecute = $false
    $info.RedirectStandardOutput = $true
    $process = [System.Diagnostics.Process]::Start($info)
    try {
        $actual = $process.StandardOutput.ReadToEnd().Trim()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0 -or $actual -ne $Edition) {
            throw "Expected $Edition binary, got '$actual': $Binary"
        }
    }
    finally {
        $process.Dispose()
    }
}

function Assert-NoAiResources([string]$Directory) {
    foreach ($relative in @("runtime/pi", "skills", "config.example.json")) {
        $path = Join-Path $Directory $relative
        if (Test-Path -LiteralPath $path) {
            throw "AI resource in no-ai staging: $path"
        }
    }
}
