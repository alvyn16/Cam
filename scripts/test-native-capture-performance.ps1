param([Parameter(Mandatory=$true)][string]$Executable,[ValidateRange(1,10)][int]$Runs=3,[string]$Label='candidate')
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
$root=Join-Path $env:TEMP ('cam-capture-profile-'+[guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root|Out-Null
$previousCapture=$env:CAM_GPU_CAPTURE
$previousEncoder=$env:CAM_ENCODER
$results=@()
try {
    $env:CAM_GPU_CAPTURE='require'
    $env:CAM_ENCODER=$null
    for($i=1;$i -le $Runs;$i++) {
        $argsLine='record --area 0,0,1920,1080 --duration 8 --fps 60 --no-system-audio --dir "'+$root+'" --name Capture'+$i
        $p=Start-Process -FilePath $exe -ArgumentList $argsLine -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $root "$i.stdout.log") -RedirectStandardError (Join-Path $root "$i.stderr.log")
        # Retain the process handle so final CPU time is readable after exit.
        $null=$p.Handle
        $peak=0L
        $watch=[Diagnostics.Stopwatch]::StartNew()
        do {
            try {$p.Refresh();$peak=[Math]::Max($peak,$p.PrivateMemorySize64)} catch {}
            if($watch.Elapsed.TotalSeconds -gt 40){$p.Kill();throw 'Native capture timed out'}
            Start-Sleep -Milliseconds 50
        } while(!$p.HasExited)
        $p.WaitForExit()
        if($p.ExitCode -ne 0){throw 'Native capture failed'}
        $cpu=$p.TotalProcessorTime.TotalSeconds
        $p.Dispose()
        if(!(Select-String -LiteralPath (Join-Path $root "$i.stderr.log") -Pattern 'Cam GPU capture:' -Quiet)){throw 'GPU route was not selected'}
        $video=Join-Path $root "Capture$i.cam\recording\screen.mov"
        $probe=& ffprobe -v error -count_frames -show_entries stream=width,height,avg_frame_rate,nb_read_frames,duration -of json $video | ConvertFrom-Json
        if($LASTEXITCODE -ne 0){throw 'Capture probe failed'}
        $stream=$probe.streams[0]
        if($stream.width -ne 1920 -or $stream.height -ne 1080 -or [int]$stream.nb_read_frames -lt 475 -or [int]$stream.nb_read_frames -gt 490){throw 'Unexpected capture dimensions/frame count'}
        & ffmpeg -v error -nostdin -i $video -f null - 2> (Join-Path $root "$i.decode.log")
        if($LASTEXITCODE -ne 0){throw 'Full independent capture decode failed'}
        $results+=@{cpu_seconds=$cpu;peak_private_mib=[Math]::Round($peak/1MB,2);video=$stream}
    }
} finally {$env:CAM_GPU_CAPTURE=$previousCapture;$env:CAM_ENCODER=$previousEncoder}
@{label=$Label;exe_sha256=(Get-FileHash $exe).Hash;runs=$results;artifacts=$root}|ConvertTo-Json -Depth 6|Tee-Object -FilePath (Join-Path $root 'result.json')
