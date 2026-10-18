param([Parameter(Mandatory=$true)][string]$Project,[switch]$Release)
$ErrorActionPreference='Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$root=Join-Path $env:TEMP ('cam-scrub-check-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$profile=if ($Release) {'release'} else {'debug'}
$exe=Join-Path $root 'cam.exe'
Copy-Item -LiteralPath "target/$profile/cam.exe" -Destination $exe
$source=(Resolve-Path -LiteralPath $Project).Path
& $exe bench-preview $source --scrub --width 1280 --height 720 2> (Join-Path $root 'bench.stderr.log') | Set-Content (Join-Path $root 'bench.json')
if ($LASTEXITCODE -ne 0) { throw 'Rapid-scrub renderer check failed.' }
$metrics=Get-Content (Join-Path $root 'bench.json') -Raw | ConvertFrom-Json
if ($null -eq $metrics.final_frame_mse) { throw 'Build lacks final-frame verification; rebuild before running this check.' }
if ($metrics.p95_ui_ms -gt 50 -or $metrics.worst_ui_ms -gt 200 -or $metrics.final_settle_ms -gt 3000) { throw "Scrub responsiveness outside budget: $($metrics | ConvertTo-Json -Compress)" }
if ($metrics.final_frame_mse -gt 100) { throw 'Final preview differs too far from the full-resolution reference.' }
@{Status='PASS';Metrics=$metrics;SHA256=(Get-FileHash $exe).Hash;Artifacts=$root} | ConvertTo-Json -Depth 6 | Tee-Object -FilePath (Join-Path $root 'result.json')
