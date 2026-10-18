param(
    [ValidateRange(600,7200)][int]$Seconds = 3600,
    [ValidateRange(10,60)][int]$Fps = 10,
    [ValidateSet('h264','hevc')][string]$Codec = 'h264',
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Join-Path $env:TEMP ('cam-native-resources-' + [guid]::NewGuid().ToString('N'))
$package = Join-Path $root 'Resource.cam'
$recording = Join-Path $package 'recording'
New-Item -ItemType Directory -Path $recording -Force | Out-Null
$binary = if ($Release) { 'target/release/cam.exe' } else { 'target/debug/cam.exe' }
$exe = Join-Path $root 'cam.exe'
Copy-Item -LiteralPath (Join-Path $PWD $binary) -Destination $exe
# This isolates GPU resource use. Audio has its separate hour-long check.
& ffmpeg -v error -nostdin -f lavfi -i 'testsrc2=size=64x48:rate=1' -t $Seconds -an -c:v libx264 -preset ultrafast -threads 1 (Join-Path $recording 'screen.mov')
if ($LASTEXITCODE -ne 0) { throw 'Cannot create source footage.' }
@{
    version=1; kind='window'; duration_ms=($Seconds*1000.0); scale=1.0
    screen=@{file='screen.mov';width=64;height=48;fps=1.0;offset_ms=0.0}
    bounds=@{x=0.0;y=0.0;w=64.0;h=48.0}
} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $recording 'meta.json') -Encoding utf8NoBOM
@{name='Native resources';timeline=@{slices=@();zooms=@();layouts=@();masks=@();voiceovers=@()}} | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $package 'project.json') -Encoding utf8NoBOM
$savedNative = $env:CAM_NATIVE_ENCODER
$savedProfile = $env:CAM_NATIVE_PROFILE
$savedEncoder = $env:CAM_ENCODER
$process = $null
$workerProcess = $null
$memory = $null
$handles = $null
try {
    $env:CAM_NATIVE_ENCODER = 'require'
    $env:CAM_NATIVE_PROFILE = '1'
    $env:CAM_ENCODER = $null
    $output = Join-Path $root 'native.mp4'
    $stderr = Join-Path $root 'stderr.log'
    $process = Start-Process -FilePath $exe -ArgumentList @('export',('"'+$package+'"'),'--output',('"'+$output+'"'),'--height','400','--fps',$Fps,'--codec',$Codec,'--quality','social') -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'stdout.log') -RedirectStandardError $stderr
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $samples = [Collections.Generic.List[object]]::new()
    while (-not $process.HasExited) {
        $process.Refresh()
        if ($process.HasExited) { break }
        if (-not $workerProcess) {
            $currentLog = Get-Content -LiteralPath $stderr -Raw -ErrorAction SilentlyContinue
            if ($currentLog -match 'Cam native export worker: (\d+)') {
                $workerProcess = Get-Process -Id ([int]$Matches[1]) -ErrorAction SilentlyContinue
            }
        }
        $workerBytes = 0L
        $workerHandles = 0
        if ($workerProcess) {
            $workerProcess.Refresh()
            if (-not $workerProcess.HasExited) {
                $workerBytes = $workerProcess.PrivateMemorySize64
                $workerHandles = $workerProcess.HandleCount
            }
        }
        $samples.Add([pscustomobject]@{Seconds=$clock.Elapsed.TotalSeconds;PrivateBytes=($process.PrivateMemorySize64+$workerBytes);Handles=($process.HandleCount+$workerHandles);ParentPrivateBytes=$process.PrivateMemorySize64;WorkerPrivateBytes=$workerBytes;WorkerHandles=$workerHandles})
        if ($clock.Elapsed.TotalSeconds -gt 600) { throw 'Native export timed out.' }
        Start-Sleep -Milliseconds 200
    }
    $samples | Export-Csv (Join-Path $root 'memory.csv') -NoTypeInformation
    if ($process.ExitCode -ne 0) { throw 'Native export failed.' }
    $text = Get-Content -LiteralPath $stderr -Raw
    if ($text -notmatch 'Cam native GPU export:') { throw 'Native encoding was not selected.' }
    $gpu = @([regex]::Matches($text,'native-gpu-memory frames=(\d+) local=(\d+) budget=(\d+)') | ForEach-Object {
        [pscustomobject]@{Frames=[long]$_.Groups[1].Value;LocalBytes=[long]$_.Groups[2].Value;BudgetBytes=[long]$_.Groups[3].Value}
    })
    $gpu | Export-Csv (Join-Path $root 'gpu.csv') -NoTypeInformation
    $steadyGpu = @($gpu | Where-Object Frames -ge 1000)
    if ($steadyGpu.Count -lt 3) { throw 'Not enough direct DXGI resource samples.' }
    $gpuRange = $steadyGpu | Measure-Object LocalBytes -Minimum -Maximum
    if (($gpuRange.Maximum-$gpuRange.Minimum)/1MB -gt 128) { throw 'Direct DXGI memory grew beyond 128 MiB.' }
    # Exclude startup and normal sink/device teardown. Handle counts drop
    # during teardown; counting that drop as a leak is a measurement error.
    $last = [double]$samples[-1].Seconds
    $steady = @($samples | Where-Object {$_.Seconds -ge 5 -and $_.Seconds -lt $last-2 -and $_.WorkerPrivateBytes -gt 0})
    if ($steady.Count -ge 5) {
        $memory = $steady | Measure-Object PrivateBytes -Minimum -Maximum
        $handles = $steady | Measure-Object Handles -Minimum -Maximum
        if (($memory.Maximum-$memory.Minimum)/1MB -gt 128) { throw 'Native private memory grew beyond 128 MiB.' }
        if ($handles.Maximum-$handles.Minimum -gt 64) { throw 'Native handle count grew beyond 64.' }
    }
    $probe = & ffprobe -v error -show_entries format=duration:stream=codec_name,nb_frames,width,height -of json $output | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or [int]$probe.streams[0].nb_frames -ne $Seconds*$Fps -or [math]::Abs([double]$probe.format.duration-$Seconds) -gt 0.1) { throw 'Native output metadata is incorrect.' }
    & ffmpeg -v error -i $output -f null NUL
    if ($LASTEXITCODE -ne 0) { throw 'Native output failed full independent decoding.' }
    if (Get-ChildItem -LiteralPath $root -File | Where-Object Name -match '\.native-video\.|\.complete\.|\.mux-log\.') { throw 'Native staging files remain.' }
    if (Get-ChildItem -LiteralPath $root -Directory -Force | Where-Object Name -like '.cam-native-*') { throw 'Native worker staging remains.' }
    [pscustomobject]@{Frames=$Seconds*$Fps;VideoSeconds=$Seconds;ElapsedSeconds=$clock.Elapsed.TotalSeconds;GpuRangeMiB=($gpuRange.Maximum-$gpuRange.Minimum)/1MB;PrivateRangeMiB=if($memory){($memory.Maximum-$memory.Minimum)/1MB}else{$null};HandleRange=if($handles){$handles.Maximum-$handles.Minimum}else{$null};Artifacts=$root} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'result.json')
} catch {
    if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id }
    throw "$_ Artifacts: $root"
} finally {
    if ($process) { $process.Dispose() }
    if ($workerProcess) { $workerProcess.Dispose() }
    $env:CAM_NATIVE_ENCODER = $savedNative
    $env:CAM_NATIVE_PROFILE = $savedProfile
    $env:CAM_ENCODER = $savedEncoder
}
