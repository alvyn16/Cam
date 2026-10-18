param(
    [ValidateRange(60,14400)][int]$Seconds = 3600,
    [switch]$Release,
    [string]$Resume
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
if ((Get-PSDrive C).Free -lt 8GB) { throw 'This check needs 8 GiB of free fixture/cache space.' }
$root = if ($Resume) { (Resolve-Path -LiteralPath $Resume).Path } else { Join-Path $env:TEMP ('cam-long-audio-' + [guid]::NewGuid().ToString('N')) }
$package = Join-Path $root 'Long.cam'
$recording = Join-Path $package 'recording'
New-Item -ItemType Directory -Path $recording -Force | Out-Null
$binary = if ($Release) { 'target/release/cam.exe' } else { 'target/debug/cam.exe' }
$exe = Join-Path $root 'cam.exe'
if (-not $Resume) { Copy-Item -LiteralPath (Join-Path $PWD $binary) -Destination $exe }

function Write-Project([bool]$mute) {
    @{
        name='Long audio check'; style=@{ audio=@{ mute_system=$mute; enhance_mic=$false; volume=1.0 } }
        timeline=@{ slices=@(); zooms=@(); layouts=@(); masks=@(); voiceovers=@() }
    } | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $package 'project.json') -Encoding utf8NoBOM
}

function Run-Monitored([string]$name, [string[]]$arguments, [int]$timeout = 600) {
    $p = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root "$name.stdout.log") -RedirectStandardError (Join-Path $root "$name.stderr.log")
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $samples = [Collections.Generic.List[object]]::new()
    try {
        while (-not $p.HasExited) {
            $p.Refresh()
            $samples.Add([pscustomobject]@{ Seconds=$clock.Elapsed.TotalSeconds; PrivateBytes=$p.PrivateMemorySize64 })
            if ($clock.Elapsed.TotalSeconds -gt $timeout) { throw "$name timed out." }
            Start-Sleep -Milliseconds 250
        }
        $samples | Export-Csv -LiteralPath (Join-Path $root "$name.memory.csv") -NoTypeInformation
        if ($p.ExitCode -ne 0) { throw "$name exited $($p.ExitCode)." }
        return [pscustomobject]@{ Name=$name; Seconds=$clock.Elapsed.TotalSeconds; PeakPrivateBytes=($samples | Measure-Object PrivateBytes -Maximum).Maximum }
    } finally {
        if (-not $p.HasExited) { Stop-Process -Id $p.Id }
        $p.Dispose()
    }
}

function Read-Measurement([string]$name) {
    $rows = @(Import-Csv -LiteralPath (Join-Path $root "$name.memory.csv"))
    return [pscustomobject]@{ Name=$name; Seconds=[double]$rows[-1].Seconds; PeakPrivateBytes=($rows | ForEach-Object { [long]$_.PrivateBytes } | Measure-Object -Maximum).Maximum }
}

try {
    if (-not $Resume) {
        & ffmpeg -v error -nostdin -f lavfi -i 'testsrc2=size=64x48:rate=1' -t $Seconds -an -c:v libx264 -preset ultrafast -threads 1 (Join-Path $recording 'screen.mov')
        if ($LASTEXITCODE -ne 0) { throw 'Video fixture generation failed.' }
        & ffmpeg -v error -nostdin -f lavfi -i 'sine=frequency=880:sample_rate=48000' -t $Seconds -ac 1 -c:a flac -threads 1 (Join-Path $recording 'system.flac')
        if ($LASTEXITCODE -ne 0) { throw 'Audio fixture generation failed.' }
        @{
            version=1; kind='window'; duration_ms=($Seconds*1000.0); scale=1.0
            screen=@{file='screen.mov'; width=64; height=48; fps=1.0; offset_ms=0.0}
            bounds=@{x=0.0;y=0.0;w=64.0;h=48.0}
            system_audio=@{file='system.flac';track=0}
        } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $recording 'meta.json') -Encoding utf8NoBOM
        $quoted = '"' + $package + '"'
        Write-Project $true
        $baseline = Run-Monitored 'baseline' @('export',$quoted,'--output',('"' + (Join-Path $root 'baseline.mp4') + '"'),'--height','64','--fps','1','--quality','web')
        Write-Project $false
        $output = Join-Path $root 'audio.mp4'
        $audio = Run-Monitored 'audio' @('export',$quoted,'--output',('"' + $output + '"'),'--height','64','--fps','1','--quality','web')
    } else {
        $baseline = Read-Measurement 'baseline'
        $audio = Read-Measurement 'audio'
        $output = Join-Path $root 'audio.mp4'
    }
    $extra = ($audio.PeakPrivateBytes - $baseline.PeakPrivateBytes) / 1MB
    if ($extra -gt 128) { throw "Audio export added $extra MiB of private memory." }
    $probe = & ffprobe -v error -show_entries format=duration:stream=codec_type,duration -of json $output | ConvertFrom-Json
    if ([math]::Abs([double]$probe.format.duration - $Seconds) -gt 0.1) { throw 'Incorrect output duration.' }
    if (-not ($probe.streams | Where-Object codec_type -eq 'audio')) { throw 'Export has no audio stream.' }
    foreach ($at in @(0, [int]($Seconds/2), ($Seconds-3))) {
        $segment = Join-Path $root "at-$at.wav"
        & ffmpeg -v error -nostdin -y -ss $at -i $output -t 2 -vn -c:a pcm_s16le $segment
        if ($LASTEXITCODE -ne 0) { throw "Cannot decode audio at $at seconds." }
        & $exe audio-check $segment --freq 880 --require audio0 --min-seconds 1.8 *> (Join-Path $root "at-$at.check.log")
        if ($LASTEXITCODE -ne 0) { throw "Incorrect audio at $at seconds." }
    }
    $savedPlay = $env:CAM_GUI_PLAYTEST
    $savedSeek = $env:CAM_PLAYTEST_SEEK_FRACTION
    $savedRequire = $env:CAM_PLAYTEST_REQUIRE_SEEK_AUDIO
    try {
        $env:CAM_GUI_PLAYTEST = $package
        $env:CAM_PLAYTEST_REQUIRE_SEEK_AUDIO = '1'
        & ffmpeg -v error -nostdin -y -i (Join-Path $recording 'system.flac') -t 30 -c:a copy (Join-Path $recording 'short.flac')
        if ($LASTEXITCODE -ne 0) { throw 'Short audio control generation failed.' }
        $metaPath = Join-Path $recording 'meta.json'
        $meta = Get-Content -LiteralPath $metaPath -Raw | ConvertFrom-Json
        $meta.system_audio.file = 'short.flac'
        $meta | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $metaPath -Encoding utf8NoBOM
        $env:CAM_PLAYTEST_SEEK_FRACTION = '0.001'
        $editorBaseline = Run-Monitored 'editor-baseline' @('gui') 120
        if ((Get-Content (Join-Path $root 'editor-baseline.stderr.log') -Raw) -notmatch 'playtest: PASS') { throw 'Editor control check did not pass.' }
        $meta.system_audio.file = 'system.flac'
        $meta | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $metaPath -Encoding utf8NoBOM
        Write-Project $false
        $env:CAM_PLAYTEST_SEEK_FRACTION = '0.95'
        $gui = Run-Monitored 'editor' @('gui') 120
        if ((Get-Content (Join-Path $root 'editor.stderr.log') -Raw) -notmatch 'playtest: PASS') { throw 'Editor playback check did not pass.' }
        $editorExtra = ($gui.PeakPrivateBytes - $editorBaseline.PeakPrivateBytes) / 1MB
        if ($editorExtra -gt 128) { throw "Long audio added $editorExtra MiB of editor private memory." }
    } finally {
        $env:CAM_GUI_PLAYTEST = $savedPlay
        $env:CAM_PLAYTEST_SEEK_FRACTION = $savedSeek
        $env:CAM_PLAYTEST_REQUIRE_SEEK_AUDIO = $savedRequire
    }
    if (Get-ChildItem -LiteralPath (Join-Path $package 'cache') -Filter 'mix-output-*') { throw 'Temporary export soundtracks remain.' }
    [pscustomobject]@{ Seconds=$Seconds; AdditionalExportMiB=[math]::Round($extra,2); AdditionalEditorMiB=[math]::Round($editorExtra,2); Baseline=$baseline; Audio=$audio; EditorBaseline=$editorBaseline; Editor=$gui; Artifacts=$root } | ConvertTo-Json -Depth 5 | Tee-Object -FilePath (Join-Path $root 'result.json')
} catch {
    throw "$_ Artifacts: $root"
}
