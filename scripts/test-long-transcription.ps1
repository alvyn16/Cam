param(
    [Parameter(Mandatory=$true)][string]$Speech,
    [ValidateRange(120,7200)][int]$Seconds = 3600,
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Join-Path $env:TEMP ('cam-long-transcription-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$profile = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $root 'cam.exe'
Copy-Item -LiteralPath (Join-Path $PWD "target/$profile/cam.exe") -Destination $exe
$speechPath = (Resolve-Path -LiteralPath $Speech).Path
$shortPackage = Join-Path $root 'Short.cam'
$longPackage = Join-Path $root 'Long.cam'
foreach ($package in @($shortPackage,$longPackage)) {
    New-Item -ItemType Directory -Path (Join-Path $package 'recording') | Out-Null
    @{name='Long input verification';style=@{captions=@{show=$true}}} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $package 'project.json') -Encoding utf8NoBOM
}
$speechProbe = & ffprobe -v error -show_entries format=duration -of json $speechPath | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot probe speech fixture.' }
$shortSeconds = [double]$speechProbe.format.duration
Copy-Item -LiteralPath $speechPath -Destination (Join-Path $shortPackage 'recording/mic.wav')
$positions = @(47.0, ($Seconds/2.0), ($Seconds-10.0))
if ($shortSeconds -gt 9) { throw 'Use the known 14-word speech fixture (under nine seconds).' }
$rawSpeech = Join-Path $root 'speech.s16le'
& ffmpeg -v error -nostdin -i $speechPath -ar 16000 -ac 1 -f s16le $rawSpeech
if ($LASTEXITCODE -ne 0) { throw 'Cannot convert speech fixture.' }
$speechBytes = [IO.File]::ReadAllBytes($rawSpeech)
$longWave = [IO.File]::Create((Join-Path $longPackage 'recording/mic.wav'))
try {
    $header = [IO.BinaryWriter]::new($longWave, [Text.Encoding]::ASCII, $true)
    $dataBytes = [uint32]($Seconds*16000*2)
    $header.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $header.Write([uint32](36+$dataBytes))
    $header.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $header.Write([uint32]16)
    $header.Write([uint16]1); $header.Write([uint16]1); $header.Write([uint32]16000); $header.Write([uint32]32000)
    $header.Write([uint16]2); $header.Write([uint16]16); $header.Write([Text.Encoding]::ASCII.GetBytes('data')); $header.Write($dataBytes)
    $header.Flush(); $header.Dispose()
    # Extending a new file supplies zero-filled silence without a whole-track array.
    $longWave.SetLength(44+[long]$dataBytes)
    foreach ($position in $positions) { $longWave.Position = 44+[long]($position*32000); $longWave.Write($speechBytes,0,$speechBytes.Length) }
    $longWave.Flush()
} finally { $longWave.Dispose() }
& ffmpeg -v error -nostdin -f lavfi -i 'testsrc2=size=64x48:rate=1' -t $Seconds -an -c:v libx264 -preset ultrafast -threads 1 (Join-Path $longPackage 'recording/screen.mov')
if ($LASTEXITCODE -ne 0) { throw 'Cannot create video fixture.' }
Copy-Item -LiteralPath (Join-Path $longPackage 'recording/screen.mov') -Destination (Join-Path $shortPackage 'recording/screen.mov')
foreach ($case in @(@($shortPackage,$shortSeconds,'mic.wav'),@($longPackage,[double]$Seconds,'mic.wav'))) {
    @{version=1;kind='display';duration_ms=([double]$case[1]*1000);scale=1.0;screen=@{file='screen.mov';width=64;height=48;fps=1.0};bounds=@{x=0.0;y=0.0;w=64.0;h=48.0};mic=@{file=$case[2];track=0}} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $case[0] 'recording/meta.json') -Encoding utf8NoBOM
}
function Run-Measured([string]$name,[string[]]$arguments) {
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root "$name.stdout.log") -RedirectStandardError (Join-Path $root "$name.stderr.log")
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $samples = [Collections.Generic.List[object]]::new()
    try {
        while (-not $process.HasExited) {
            $process.Refresh()
            if ($process.HasExited) { break }
            $samples.Add([pscustomobject]@{Seconds=$clock.Elapsed.TotalSeconds;PrivateBytes=$process.PrivateMemorySize64;Handles=$process.HandleCount})
            if ($clock.Elapsed.TotalSeconds -gt 600) { throw "$name timed out" }
            Start-Sleep -Milliseconds 100
        }
        $process.WaitForExit()
        $samples | Export-Csv (Join-Path $root "$name.memory.csv") -NoTypeInformation
        if ($process.ExitCode -ne 0) { throw "$name failed; inspect stderr" }
        if ($samples.Count -eq 0) { throw "$name completed without a memory sample" }
        return [pscustomobject]@{Peak=($samples | Measure-Object PrivateBytes -Maximum).Maximum;Seconds=$clock.Elapsed.TotalSeconds}
    } finally {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id }
        $process.Dispose()
    }
}
function Words([string]$package) { return @(Get-Content (Join-Path $package 'recording/transcript.json') -Raw | ConvertFrom-Json) }
function Normalize([object[]]$words) { return ((($words | ForEach-Object text) -join ' ') -replace '[^a-zA-Z ]','').ToLowerInvariant() }
Write-Output "Long input artifacts: $root"
$short = Run-Measured 'short' @('transcribe',('"'+$shortPackage+'"'),'--engine','whisper-base')
$long = Run-Measured 'long' @('transcribe',('"'+$longPackage+'"'),'--engine','whisper-base')
$shortWords = Words $shortPackage
$longWords = Words $longPackage
$expected = Normalize $shortWords
if ($expected -ne 'this is a recording test the quick brown fox jumps over the lazy dog') { throw 'Speech fixture did not match the known 14-word sentence.' }
if ($longWords.Count -ne $shortWords.Count*3) { throw 'Long transcription added or lost words.' }
foreach ($position in $positions) {
    $section = @($longWords | Where-Object {$_.start -ge ($position-1)*1000 -and $_.end -le ($position+$shortSeconds+1)*1000})
    if ((Normalize $section) -ne $expected) { throw "Wrong words/timing near $position seconds." }
}
if (($long.Peak-$short.Peak)/1MB -gt 128) { throw 'Transcription sample memory grew beyond the 128 MiB allowance.' }
$diagnostic = Run-Measured 'audio-check' @('audio-check',('"'+(Join-Path $longPackage 'recording/mic.wav')+'"'),'--freq','880')
$diagnosticText = Get-Content (Join-Path $root 'audio-check.stdout.log') -Raw
if ($diagnosticText -notmatch ("audio0: " + $Seconds + '\.0 s') -or $diagnostic.Peak/1MB -gt 256) { throw 'Long audio diagnostic duration or memory is incorrect.' }
& $exe frame $longPackage --time ($positions[1]+1) --height 400 --output (Join-Path $root 'caption-middle.png') 1> (Join-Path $root 'frame.stdout.log') 2> (Join-Path $root 'frame.stderr.log')
if ($LASTEXITCODE -ne 0) { throw 'Caption preview frame failed.' }
[pscustomobject]@{VideoSeconds=$Seconds;Words=$longWords.Count;ShortPeakMiB=$short.Peak/1MB;LongPeakMiB=$long.Peak/1MB;DifferenceMiB=($long.Peak-$short.Peak)/1MB;LongTranscriptionSeconds=$long.Seconds;DiagnosticPeakMiB=$diagnostic.Peak/1MB;DiagnosticSeconds=$diagnostic.Seconds;Artifacts=$root} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'result.json')
