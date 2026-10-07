param(
    [string]$Binary = (Join-Path $PSScriptRoot '../target/release/turntable.exe'),
    [string]$Version = $env:TURNTABLE_VERSION
)
$ErrorActionPreference = 'Stop'
if (-not $Version) { throw 'Set TURNTABLE_VERSION or pass -Version.' }
if (-not (Test-Path $Binary -PathType Leaf)) { throw "Missing executable: $Binary" }
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Dist = Join-Path $Root 'dist'
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
try {
    New-Item -ItemType Directory -Path $Stage | Out-Null
    Copy-Item $Binary (Join-Path $Stage 'turntable.exe')
    Copy-Item (Join-Path $Root 'LICENSE') $Stage
    Copy-Item (Join-Path $PSScriptRoot 'INSTALL-windows.md') (Join-Path $Stage 'INSTALL.md')
    Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath (Join-Path $Dist "turntable-$Version-windows-x86_64.zip") -Force
} finally {
    Remove-Item -Recurse -Force $Stage
}
