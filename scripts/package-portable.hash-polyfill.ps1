# 兜底 `Get-FileHash` cmdlet：仅当 Microsoft.PowerShell.Utility 没自带它时 dot-source 进来。
# 详见 `package-portable.ps1` 顶部注释。PowerShell 5.1 严格解析模式下不能接受在 `if`
# 块内用 `function` 关键字定义带 `[CmdletBinding()]` 的 advanced function（语法解析
# 报"表达式或语句中包含意外的标记"}""），所以 polyfill 独立成文件、定义放顶层、不加
# CmdletBinding() —— 5.1 上加 CmdletBinding 同样触发该报错。

if (Get-Command -Name "Get-FileHash" -ErrorAction SilentlyContinue) {
    return
}

function Get-FileHash {
    param(
        [Parameter(Mandatory = $true)]
        [string]$LiteralPath,
        [Parameter(Mandatory = $true)]
        [ValidateSet("SHA1", "SHA256", "SHA384", "SHA512", "MD5")]
        [string]$Algorithm
    )
    process {
        $hasher = [System.Security.Cryptography.HashAlgorithm]::Create($Algorithm)
        if (-not $hasher) {
            throw "Get-FileHash polyfill: unsupported algorithm $Algorithm"
        }
        $stream = [System.IO.File]::OpenRead($LiteralPath)
        try {
            $bytes = $hasher.ComputeHash($stream)
        }
        finally {
            $stream.Close()
        }
        $hex = -join ($bytes | ForEach-Object { $_.ToString("x2") })
        $obj = New-Object psobject -Property @{ Algorithm = $Algorithm.ToUpperInvariant(); Hash = $hex; Path = $LiteralPath }
        return $obj
    }
}