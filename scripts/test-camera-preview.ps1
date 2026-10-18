param([switch]$Release, [switch]$HidePreview, [switch]$Disabled)
$ErrorActionPreference='Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CamCameraPreviewProbe {
    public delegate bool Visitor(IntPtr hwnd, IntPtr data);
    [DllImport("user32.dll")] public static extern bool EnumWindows(Visitor visitor, IntPtr data);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, System.Text.StringBuilder text, int count);
    public static IntPtr FindCamera(uint process) {
        IntPtr result=IntPtr.Zero;
        EnumWindows((hwnd, data) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            if (owner == process) {
                var title=new System.Text.StringBuilder(256); GetWindowText(hwnd,title,256);
                if (title.ToString().Contains("camera preview")) { result=hwnd; return false; }
            }
            return true;
        },IntPtr.Zero);
        return result;
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowDisplayAffinity(IntPtr hwnd, out uint affinity);
    [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr hwnd, int index);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr wp, IntPtr lp);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
}
'@
$root=Join-Path $env:TEMP ('cam-camera-preview-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$profile=if ($Release) { 'release' } else { 'debug' }
$exe=Join-Path $root 'cam.exe'
Copy-Item -LiteralPath "target/$profile/cam.exe" -Destination $exe
$keys=@('CAM_GUI_SELFTEST','CAM_SELFTEST_REAL','CAM_SELFTEST_CAMERA','CAM_SELFTEST_SYSTEM','CAM_SELFTEST_CAMERA_PREVIEW','CAM_SELFTEST_HIDE_CAMERA_PREVIEW','CAM_OUTPUT_DIR','CAM_NATIVE_ENCODER')
$saved=@{}; foreach ($key in $keys) { $saved[$key]=[Environment]::GetEnvironmentVariable($key,'Process') }
$process=$null
try {
    $env:CAM_GUI_SELFTEST='1'; $env:CAM_SELFTEST_REAL='1'; $env:CAM_SELFTEST_CAMERA='1'; $env:CAM_SELFTEST_SYSTEM='1'
    $env:CAM_SELFTEST_CAMERA_PREVIEW=if ($Disabled) { $null } else { '1' }
    $env:CAM_SELFTEST_HIDE_CAMERA_PREVIEW=if ($HidePreview) { '1' } else { $null }
    $env:CAM_OUTPUT_DIR=$root; $env:CAM_NATIVE_ENCODER='require'
    $process=Start-Process -FilePath $exe -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'stdout.log') -RedirectStandardError (Join-Path $root 'gui.log')
    $clock=[Diagnostics.Stopwatch]::StartNew(); $found=$false; $closed=$false; $window=[IntPtr]::Zero
    while (-not $process.HasExited) {
        if ($clock.Elapsed.TotalSeconds -gt 120) { throw 'Camera preview GUI timed out.' }
        $candidate=[CamCameraPreviewProbe]::FindCamera([uint32]$process.Id)
        if ($candidate -ne [IntPtr]::Zero) {
            [uint32]$owner=0; [void][CamCameraPreviewProbe]::GetWindowThreadProcessId($candidate,[ref]$owner)
            if ($owner -eq $process.Id) {
                if ($Disabled) { throw 'Preview visible with toggle disabled.' }
                $window=$candidate; $found=$true
                [uint32]$affinity=0
                if (-not [CamCameraPreviewProbe]::GetWindowDisplayAffinity($window,[ref]$affinity) -or $affinity -ne 0x11) { throw 'Camera monitor is not excluded from capture.' }
                $style=[CamCameraPreviewProbe]::GetWindowLong($window,-20)
                if (($style -band 0x08000080) -ne 0x08000080) { throw "Camera monitor must not activate or enter the taskbar (style=$style)." }
                if ($HidePreview -and -not $closed) {
                    # Let several actual frames render before exercising the normal close event.
                    Start-Sleep -Milliseconds 1000
                    if (-not [CamCameraPreviewProbe]::PostMessage($window,0x10,[IntPtr]::Zero,[IntPtr]::Zero)) { throw 'Could not close camera monitor.' }
                    $closed=$true
                }
            }
        }
        Start-Sleep -Milliseconds 50
    }
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { Get-Content (Join-Path $root 'gui.log') -Tail 20; throw 'GUI selftest failed.' }
    if (-not $Disabled -and -not $found) { throw 'No floating camera monitor appeared.' }
    if ($window -ne [IntPtr]::Zero -and [CamCameraPreviewProbe]::IsWindow($window)) { throw 'Camera window survived application exit.' }
    $media=@(Get-ChildItem -LiteralPath (Join-Path $root 'Selftest.cam/recording') -File | Where-Object { $_.Extension -in @('.mov','.wav') })
    $media+=Get-Item -LiteralPath (Join-Path $root 'Selftest.mp4')
    if ($media.Count -ne 5) { throw 'Missing camera, screen, audio or exported media.' }
    foreach ($file in $media) { & ffmpeg -v error -nostdin -i $file.FullName -f null NUL; if ($LASTEXITCODE -ne 0) { throw "Decode failed: $($file.Name)" } }
    @{Status='PASS'; PreviewEnabled=(-not $Disabled); HiddenDuringRecording=[bool]$HidePreview; MediaFilesDecoded=$media.Count; SHA256=(Get-FileHash $exe).Hash; Artifacts=$root} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'result.json')
} finally {
    if ($process -and -not $process.HasExited) { $process.Kill($true); $process.WaitForExit() }
    foreach ($key in $keys) { [Environment]::SetEnvironmentVariable($key,$saved[$key],'Process') }
}
