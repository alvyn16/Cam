param(
    [Parameter(Mandatory=$true)][string]$Baseline,
    [Parameter(Mandatory=$true)][string]$Candidate,
    [Parameter(Mandatory=$true)][string]$Project,
    [int]$Seconds=8,
    [string]$Area='0,0,1920,1080'
)
$ErrorActionPreference='Stop'
$baselineExe=(Resolve-Path $Baseline).Path
$candidateExe=(Resolve-Path $Candidate).Path
$projectPath=(Resolve-Path $Project).Path
$root=Join-Path $env:TEMP ('cam-highres-performance-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root|Out-Null
$results=@()
$previousCapture=$env:CAM_GPU_CAPTURE
$previousPreview=$env:CAM_GPU_PREVIEW
try {
    foreach($case in @('baseline','candidate')) {
        $exe=if($case -eq 'baseline'){$baselineExe}else{$candidateExe}
        $env:CAM_GPU_CAPTURE=if($case -eq 'candidate'){'require'}else{'off'}
        $argsLine='record --area '+$Area+' --duration '+$Seconds+' --fps 60 --no-system-audio --dir "'+$root+'" --name '+$case
        $process=Start-Process $exe -ArgumentList $argsLine -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root "$case.stdout.log") -RedirectStandardError (Join-Path $root "$case.stderr.log")
        $known=@{};$known[$process.Id]=$process
        $cpu=@{};$peak=0L
        $timer=[Diagnostics.Stopwatch]::StartNew()
        do {
            foreach($child in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($process.Id)")) {
                if(!$known.ContainsKey([int]$child.ProcessId)) { try { $known[[int]$child.ProcessId]=Get-Process -Id $child.ProcessId -ErrorAction Stop } catch {} }
            }
            $memory=0L
            foreach($entry in $known.GetEnumerator()) {
                try { $entry.Value.Refresh();$cpu[$entry.Key]=$entry.Value.TotalProcessorTime.TotalSeconds;$memory+=$entry.Value.PrivateMemorySize64 } catch {}
            }
            $peak=[Math]::Max($peak,$memory)
            if($timer.Elapsed.TotalSeconds -gt ($Seconds+45)){ $process.Kill();throw 'Capture benchmark timed out' }
            Start-Sleep -Milliseconds 100
            $process.Refresh()
        } while(!$process.HasExited)
        if($process.ExitCode -ne 0){throw "$case recording failed"}
        $video=Join-Path $root "$case.cam\recording\screen.mov"
        $probe=& ffprobe -v error -count_frames -show_entries stream=codec_name,width,height,nb_read_frames,duration -of json $video|ConvertFrom-Json
        if($LASTEXITCODE -ne 0){throw 'Recording probe failed'}
        & ffmpeg -v error -nostdin -i $video -f null - 2> (Join-Path $root "$case.decode.log")
        if($LASTEXITCODE -ne 0){throw 'Recording failed full independent decode'}
        if($case -eq 'candidate' -and (Get-Content (Join-Path $root "$case.stderr.log") -Raw) -notmatch 'Cam GPU capture:'){throw 'Candidate did not use direct GPU capture'}
        $stream=$probe.streams[0]
        $results+=@{case=$case;cpu_seconds=($cpu.Values|Measure-Object -Sum).Sum;peak_private_mib=[Math]::Round($peak/1MB,2);wall_seconds=$timer.Elapsed.TotalSeconds;video=$stream}
        $env:CAM_GPU_PREVIEW=if($case -eq 'candidate'){'require'}else{'off'}
        & $exe bench-preview $projectPath --scrub --width 1280 --height 720 > (Join-Path $root "$case.scrub.json") 2> (Join-Path $root "$case.scrub.log")
        if($LASTEXITCODE -ne 0){throw "$case 4K preview benchmark failed"}
        $scrub=Get-Content (Join-Path $root "$case.scrub.json") -Raw|ConvertFrom-Json
        if($scrub.final_frame_mse -gt 100 -or $scrub.p95_ui_ms -gt 50 -or $scrub.worst_ui_ms -gt 200 -or $scrub.final_settle_ms -gt 3000){throw "$case preview quality/responsiveness budget failed"}
    }
} finally { $env:CAM_GPU_CAPTURE=$previousCapture;$env:CAM_GPU_PREVIEW=$previousPreview }
@{status='PASS';capture=$results;baseline_scrub=(Get-Content (Join-Path $root 'baseline.scrub.json') -Raw|ConvertFrom-Json);candidate_scrub=(Get-Content (Join-Path $root 'candidate.scrub.json') -Raw|ConvertFrom-Json);candidate_sha256=(Get-FileHash $candidateExe).Hash;artifacts=$root}|
    ConvertTo-Json -Depth 8|Tee-Object -FilePath (Join-Path $root 'result.json')
