param([switch]$Release)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
foreach ($tool in @('ffmpeg', 'ffprobe')) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is required. Install FFmpeg and add its bin directory to PATH."
    }
}
if ($Release) { & cargo build -j 1 --release } else { & cargo build -j 1 }
if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
