param([switch]$Release)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Join-Path $env:TEMP ('cam-speech-quality-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$profile = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $root 'cam.exe'
Copy-Item -LiteralPath "target/$profile/cam.exe" -Destination $exe
$sentence = 'The quick brown fox jumps over the lazy dog. Please check that my microphone sounds clear while I record a game and export the video.'
$expected = ($sentence.ToLowerInvariant() -replace '[^a-z ]','' -replace '\s+',' ').Trim()
Add-Type -AssemblyName System.Speech
$speech = [System.Speech.Synthesis.SpeechSynthesizer]::new()
$speechPath = Join-Path $root 'speech.wav'
try {
    $speech.SetOutputToWaveFile($speechPath)
    $speech.Speak($sentence)
} finally { $speech.Dispose() }
$probe = & ffprobe -v error -show_entries format=duration -of json $speechPath | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Cannot probe speech.' }
$seconds = [double]$probe.format.duration
$package = Join-Path $root 'Speech.cam'
& $exe synth $package --duration $seconds --width 160 --height 90 2> (Join-Path $root 'synth.log') | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Cannot create speech package.' }
Copy-Item -LiteralPath $speechPath -Destination (Join-Path $package 'recording/mic.wav')
$metaPath = Join-Path $package 'recording/meta.json'
$meta = Get-Content $metaPath -Raw | ConvertFrom-Json
$meta | Add-Member -NotePropertyName mic -NotePropertyValue @{file='mic.wav';track=0} -Force
$results = @()
foreach ($case in @('source','raw-export','enhanced-export')) {
    if ($case -ne 'source') {
        $meta.mic = @{file='mic.wav';track=0}
        $meta | ConvertTo-Json -Depth 15 | Set-Content $metaPath -Encoding utf8NoBOM
        @{name='Speech quality';style=@{audio=@{enhance_mic=($case -eq 'enhanced-export');mute_system=$true}}} |
            ConvertTo-Json -Depth 8 | Set-Content (Join-Path $package 'project.json') -Encoding utf8NoBOM
        $output = Join-Path $root "$case.mp4"
        & $exe export $package --output $output --height 96 --fps 10 --quality web 2> (Join-Path $root "$case-export.log") | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "$case failed to export." }
        & ffmpeg -v error -nostdin -i $output -f null - 2> (Join-Path $root "$case-decode.log")
        if ($LASTEXITCODE -ne 0) { throw "$case failed independent decoding." }
        # Transcribe the independently encoded export, not its original WAV.
        Copy-Item -LiteralPath $output -Destination (Join-Path $package 'recording/export.mp4') -Force
        $meta.mic = @{file='export.mp4';track=0}
    }
    $meta | ConvertTo-Json -Depth 15 | Set-Content $metaPath -Encoding utf8NoBOM
    & $exe transcribe $package --engine whisper-base 2> (Join-Path $root "$case-transcribe.log") | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "$case speech recognition failed." }
    $words = @(Get-Content (Join-Path $package 'recording/transcript.json') -Raw | ConvertFrom-Json)
    $actual = (($words.text -join ' ').ToLowerInvariant() -replace '[^a-z ]','' -replace '\s+',' ').Trim()
    Copy-Item -LiteralPath (Join-Path $package 'recording/transcript.json') -Destination (Join-Path $root "$case-transcript.json")
    $results += @{Case=$case;Transcript=$actual;Matches=($actual -eq $expected)}
    if ($actual -ne $expected) { throw "Speech mismatch in ${case}: $actual" }
}
@{Status='PASS';SHA256=(Get-FileHash $exe).Hash;Results=$results;Artifacts=$root} |
    ConvertTo-Json -Depth 8 | Tee-Object -FilePath (Join-Path $root 'result.json')
