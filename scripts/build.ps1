$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    & cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed' }
    New-Item -ItemType Directory -Path 'dist' -Force | Out-Null
    Copy-Item -LiteralPath 'target/release/windows-awake.exe' -Destination 'dist/WindowsAwake.exe' -Force
    Get-Item -LiteralPath 'dist/WindowsAwake.exe' | Select-Object FullName, Length
} finally { Pop-Location }
