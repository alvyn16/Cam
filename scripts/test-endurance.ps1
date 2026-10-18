param(
    [ValidateRange(30,14400)][int]$Seconds = 120,
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$binary = if ($Release) { 'target/release/cam.exe' } else { 'target/debug/cam.exe' }
$testDir = Join-Path $env:TEMP ('cam-endurance-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDir | Out-Null
# Keep Cargo free to replace its build artifact during this longer test.
$testBinary = Join-Path $testDir 'cam.exe'
Copy-Item -LiteralPath (Join-Path $PWD $binary) -Destination $testBinary
$process = Start-Process -FilePath $testBinary -ArgumentList @('record','--area','100,100,320,200','--duration',$Seconds,'--fps','15','--mic','--camera','--dir',('"' + $testDir + '"'),'--name','Endurance') -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $testDir 'stdout.log') -RedirectStandardError (Join-Path $testDir 'stderr.log')
$clock = [Diagnostics.Stopwatch]::StartNew()
$samples = [Collections.Generic.List[object]]::new()
try {
    while (-not $process.HasExited) {
        $process.Refresh()
        $pendingBytes = 0L
        foreach ($file in Get-ChildItem -LiteralPath $testDir -Recurse -Filter '*.pending.wav') {
            # Windows directory entries can retain length 0 until a writer closes.
            # Query the open file rather than the directory's cached metadata.
            try {
                $stream = [IO.File]::Open($file.FullName,[IO.FileMode]::Open,[IO.FileAccess]::Read,([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
                try { $pendingBytes += $stream.Length } finally { $stream.Dispose() }
            } catch [IO.FileNotFoundException] { } # atomically finalized between queries
        }
        $samples.Add([pscustomobject]@{ Seconds=$clock.Elapsed.TotalSeconds; PrivateBytes=$process.PrivateMemorySize64; PendingAudioBytes=$pendingBytes })
        if ($clock.Elapsed.TotalSeconds -gt $Seconds + 30) { throw 'Recording timed out.' }
        Start-Sleep -Seconds 1
    }
    $samples | Export-Csv -LiteralPath (Join-Path $testDir 'memory.csv') -NoTypeInformation
    if ($process.ExitCode -ne 0) { throw "Recording exited $($process.ExitCode)." }
    $steady = @($samples | Where-Object { $_.Seconds -ge 15 -and $_.Seconds -lt $Seconds })
    $range = $steady | Measure-Object -Property PrivateBytes -Minimum -Maximum
    $growthMB = ($range.Maximum - $range.Minimum) / 1MB
    if ($growthMB -gt 24) { throw "Capture memory grew by $growthMB MiB after warmup." }
    $audioBytes = ($samples | Measure-Object -Property PendingAudioBytes -Maximum).Maximum
    if ($audioBytes -lt ($Seconds - 10) * 48000 * 8 * 2) { throw 'Audio was not streamed to disk while recording.' }
    $recording = Join-Path $testDir 'Endurance.cam/recording'
    foreach ($file in @('screen.mov','camera.mov','mic.wav','system.wav')) {
        $path = Join-Path $recording $file
        $probeJson = & ffprobe -v error -show_entries format=duration -of json $path
        if ($LASTEXITCODE -ne 0) { throw "Cannot probe $file." }
        $duration = [double](($probeJson | ConvertFrom-Json).format.duration)
        if ($duration -lt $Seconds - 3 -or $duration -gt $Seconds + 3) { throw "$file duration incorrect: $duration seconds." }
        & ffmpeg -v error -i $path -f null NUL
        if ($LASTEXITCODE -ne 0) { throw "Cannot fully decode $file." }
    }
    if (Get-ChildItem -LiteralPath $testDir -Recurse -Filter '*.pending.wav') { throw 'Unfinalized audio files remain.' }
    Write-Output "Endurance passed: $Seconds seconds; private-memory range after warmup $([math]::Round($growthMB,2)) MiB; audio streamed $audioBytes bytes; artifacts: $testDir"
} catch {
    if (-not $process.HasExited) { Stop-Process -Id $process.Id }
    throw "$_ Artifacts: $testDir"
} finally { $process.Dispose() }
