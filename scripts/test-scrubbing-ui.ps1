param([Parameter(Mandatory=$true)][string]$Project,[switch]$Release)
$ErrorActionPreference='Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class CamScrubUiProbe {
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int x,y; }
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left,top,right,bottom; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd,out Rect rect);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd,out uint owner);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd,System.Text.StringBuilder text,int count);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hwnd,IntPtr after,int x,int y,int width,int height,uint flags);
    [DllImport("user32.dll",SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr hwnd,uint message,UIntPtr w,IntPtr l,uint flags,uint timeout,out UIntPtr result);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd,uint message,IntPtr w,IntPtr l);
    public static void MouseMessage(IntPtr hwnd,uint message,int x,int y) { if(!PostMessage(hwnd,message,(IntPtr)(message==0x202?0:1),(IntPtr)((y<<16)|(x&65535)))) throw new Exception("Mouse message failed"); }
}
'@
$root=Join-Path $env:TEMP ('cam-scrub-ui-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$profile=if ($Release) {'release'} else {'debug'}
$exe=Join-Path $root 'cam.exe'; Copy-Item -LiteralPath "target/$profile/cam.exe" -Destination $exe
$package=Join-Path $root 'Scrub.cam'; Copy-Item -LiteralPath (Resolve-Path -LiteralPath $Project).Path -Destination $package -Recurse
$foreground=[CamScrubUiProbe]::GetForegroundWindow()
$oldDpi=[CamScrubUiProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
$savedTrace=$env:CAM_SCRUB_TRACE; $savedTraceFile=$env:CAM_SCRUB_TRACE_FILE; $process=$null
$window=[IntPtr]::Zero
try {
    $env:CAM_SCRUB_TRACE='1'
    $traceFile=Join-Path $root 'trace.log'; $env:CAM_SCRUB_TRACE_FILE=$traceFile
    $process=Start-Process -FilePath $exe -ArgumentList @('gui',('"{0}"' -f $package)) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'stdout.log') -RedirectStandardError (Join-Path $root 'gui.log')
    # Warm the initial GPU upload, inspector thumbnails and audio preparation;
    # measure scrubbing in an already-open editor rather than startup work.
    Start-Sleep -Seconds 12; $process.Refresh()
    if ($process.HasExited) { throw 'Scrub GUI exited early.' }
    $window=$process.MainWindowHandle
    # An occluded DXGI swapchain is throttled by Windows. Keep the owned test
    # window visible even when foreground activation is denied to automation.
    [void][CamScrubUiProbe]::SetWindowPos($window,[IntPtr](-1),0,0,0,0,0x13)
    [void][CamScrubUiProbe]::SetForegroundWindow($window)
    Start-Sleep -Milliseconds 500
    # Direct the same client mouse messages to the owned window. This avoids
    # desktop focus changes or physical pointer interference during the check.
    $rect=[CamScrubUiProbe+Rect]::new(); [void][CamScrubUiProbe]::GetClientRect($window,[ref]$rect)
    [uint32]$owner=0; [void][CamScrubUiProbe]::GetWindowThreadProcessId($window,[ref]$owner)
    $class=[Text.StringBuilder]::new(256); [void][CamScrubUiProbe]::GetClassName($window,$class,256)
    @{Window=$window.ToInt64();Process=$process.Id;Owner=$owner;Class=$class.ToString();Width=$rect.right;Height=$rect.bottom} | ConvertTo-Json | Set-Content (Join-Path $root 'window.json')
    if ($owner -ne $process.Id -or $rect.bottom -lt 500) { throw 'Probe did not select the owned editor window.' }
    $deadline=[DateTime]::UtcNow.AddSeconds(30)
    $ruler=$null
    while (-not $ruler -and [DateTime]::UtcNow -lt $deadline) {
        $line=Select-String -LiteralPath $traceFile -Pattern '^scrub-ruler: x=([0-9.-]+) y=([0-9.-]+) w=([0-9.-]+) h=([0-9.-]+) ppp=([0-9.-]+)' | Select-Object -Last 1
        if ($line) { $ruler=$line.Matches[0].Groups } else { Start-Sleep -Milliseconds 100 }
    }
    if (-not $ruler) { throw 'Editor did not report its timeline ruler layout.' }
    $scale=[double]$ruler[5].Value
    $rulerY=([double]$ruler[2].Value+[double]$ruler[4].Value/2)*$scale
    $samples=[Collections.Generic.List[double]]::new(); $timeouts=0; $seeks=0
    $errors=[Collections.Generic.List[int]]::new()
    $y=[int]$rulerY
    [CamScrubUiProbe]::MouseMessage($window,0x200,[int](40*$scale),$y)
    [CamScrubUiProbe]::MouseMessage($window,0x201,[int](40*$scale),$y)
        Start-Sleep -Milliseconds 50
        try {
            for ($i=0;$i -lt 120;$i++) {
                $logicalX=if ($i%2 -eq 0) {80+($i%11)*8} else {760-($i%11)*8}
                $x=[int]($logicalX*$scale)
                [CamScrubUiProbe]::MouseMessage($window,0x200,$x,$y)
                $timer=[Diagnostics.Stopwatch]::StartNew(); [UIntPtr]$reply=[UIntPtr]::Zero
                if ([CamScrubUiProbe]::SendMessageTimeout($window,0,[UIntPtr]::Zero,[IntPtr]::Zero,2,150,[ref]$reply) -eq [IntPtr]::Zero) { $timeouts++; $errors.Add([Runtime.InteropServices.Marshal]::GetLastWin32Error()) }
                $samples.Add($timer.Elapsed.TotalMilliseconds)
                Start-Sleep -Milliseconds 8
            }
        } finally {
            [CamScrubUiProbe]::MouseMessage($window,0x202,[int](40*$scale),$y)
        }
        Start-Sleep -Milliseconds 250
    [void][CamScrubUiProbe]::PostMessage($window,0x10,[IntPtr]::Zero,[IntPtr]::Zero)
    if (-not $process.WaitForExit(10000)) { throw 'Scrub window did not close promptly.' }
    if ($process.ExitCode -ne 0) { throw 'Scrub GUI exited with an error.' }
    $seeks=@(Select-String -LiteralPath $traceFile -Pattern '^scrub: seek_ms=').Count
    @{Samples=@($samples);Errors=@($errors);Seeks=$seeks} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $root 'message-samples.json')
    if ($seeks -lt 20) { throw "Input did not exercise timeline scrubbing: $seeks seeks." }
    if ($timeouts -gt 0) { throw "UI stalled during scrub: $timeouts message timeouts." }
    $sorted=@($samples | Sort-Object)
    @{Status='PASS';InputMode='PostedWindowMessages';ActualSeeks=$seeks;MessageTimeouts=$timeouts;P95MessageMs=$sorted[[int]($sorted.Count*0.95)];WorstMessageMs=$sorted[-1];SHA256=(Get-FileHash $exe).Hash;Artifacts=$root} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'result.json')
} finally {
    if ($process -and -not $process.HasExited) {
        [void][CamScrubUiProbe]::PostMessage($process.MainWindowHandle,0x10,[IntPtr]::Zero,[IntPtr]::Zero)
        if (-not $process.WaitForExit(10000)) { $process.Kill($true); $process.WaitForExit() }
    }
    [void][CamScrubUiProbe]::SetForegroundWindow($foreground)
    [void][CamScrubUiProbe]::SetThreadDpiAwarenessContext($oldDpi)
    $env:CAM_SCRUB_TRACE=$savedTrace
    $env:CAM_SCRUB_TRACE_FILE=$savedTraceFile
}
