param(
    [Parameter(Mandatory=$true)][string]$PackageResult,
    [Parameter(Mandatory=$true)][string]$RuntimeArchive,
    [switch]$Download,
    [switch]$Integration,
    [string]$HighResolutionProject
)
$ErrorActionPreference = 'Stop'
$package = Get-Content $PackageResult -Raw | ConvertFrom-Json
$root = Join-Path $env:TEMP ('cam-setup-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$extracted = Join-Path $root 'extracted setup'
New-Item -ItemType Directory -Path $extracted | Out-Null
$process = Start-Process -FilePath $package.setup -ArgumentList ('/Q /C /T:"' + $extracted + '"') -WindowStyle Hidden -PassThru
if (!$process.WaitForExit(60000) -or $process.ExitCode -ne 0) { throw 'Setup self-extraction failed.' }
foreach ($name in @('cam.exe','Install.ps1','Uninstall.ps1','runtime.json','package.json','Install.cmd','LICENSE-RNNoise.txt')) {
    if (!(Test-Path -LiteralPath (Join-Path $extracted $name))) { throw "Setup payload missing $name" }
}
if ((Get-FileHash (Join-Path $extracted 'cam.exe')).Hash -ne $package.cam_sha256) { throw 'Extracted executable hash mismatch.' }
foreach ($dll in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
    if (!(Test-Path -LiteralPath (Join-Path $extracted $dll))) { throw "C++ runtime missing from setup: $dll" }
}
$target = Join-Path $root 'Programs with spaces\Cam'
$ps = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$installArgs = @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $extracted 'Install.ps1'),'-Quiet','-NoLaunch','-NoShortcuts','-NoRegistration','-InstallDir',$target)
if ($Integration) {
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Cam'
    $links = @([Environment]::GetFolderPath('Desktop'),[Environment]::GetFolderPath('Programs')) | ForEach-Object { Join-Path $_ 'Cam.lnk' }
    if ((Test-Path $key) -or @($links | Where-Object { Test-Path -LiteralPath $_ }).Count) { throw 'Integration test requires no existing Cam registration/shortcuts.' }
    $installArgs = @($installArgs | Where-Object { $_ -ne '-NoShortcuts' -and $_ -ne '-NoRegistration' })
}
if (!$Download) { $installArgs += @('-RuntimeArchive',$RuntimeArchive) }
$scriptArgs = @($installArgs | Select-Object -Skip 5)
& (Join-Path $extracted 'Install.cmd') @scriptArgs > (Join-Path $root 'install.log') 2>&1
if ($LASTEXITCODE -ne 0) { throw "Install failed: $root\install.log" }
if ($Integration) {
    if ((Get-ItemProperty $key).InstallLocation -ne $target) { throw 'Installed apps entry points to the wrong folder.' }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($link in $links) { if ($shell.CreateShortcut($link).TargetPath -ne (Join-Path $target 'cam.exe')) { throw 'Shortcut target is incorrect.' } }
}
$before = (Get-FileHash (Join-Path $target 'cam.exe')).Hash
# An upgrade preserves additional user files and replaces only managed content.
Set-Content (Join-Path $target 'keep-me.txt') 'user data'
$installArgs = @('-NoProfile','-ExecutionPolicy','Bypass','-File',(Join-Path $extracted 'Install.ps1'),'-Quiet','-NoLaunch','-NoShortcuts','-NoRegistration','-InstallDir',$target,'-RuntimeArchive',$RuntimeArchive)
& $ps @installArgs > (Join-Path $root 'upgrade.log') 2>&1
if ($LASTEXITCODE -ne 0 -or (Get-Content (Join-Path $target 'keep-me.txt')) -ne 'user data') { throw 'Upgrade failed or removed user files.' }
# Corrupt downloads must leave the existing installation unchanged.
$bad = Join-Path $root 'corrupt.zip'
Set-Content $bad 'not an archive'
$installArgs[-1] = $bad
& $ps @installArgs > (Join-Path $root 'corrupt.log') 2>&1
if ($LASTEXITCODE -eq 0 -or (Get-FileHash (Join-Path $target 'cam.exe')).Hash -ne $before) { throw 'Bad download damaged the existing installation.' }
$previousPath = $env:PATH
$previousFfmpeg = $env:CAM_FFMPEG
$previousFfprobe = $env:CAM_FFPROBE
$previousEncoder = $env:CAM_ENCODER
$previousGpuCapture = $env:CAM_GPU_CAPTURE
$previousGpuPreview = $env:CAM_GPU_PREVIEW
function Invoke-CamCheck([string[]]$Arguments, [string]$Stdout, [string]$Stderr) {
    # PowerShell does not reliably wait for Windows GUI-subsystem executables
    # invoked with &: use the owned process's actual exit code, not a stale
    # LASTEXITCODE left by the intentional corrupt-install check above.
    $quoted = foreach ($argument in $Arguments) {
        if ($argument.Contains('"') -or $argument.EndsWith('\')) {
            throw 'Unexpected quoting in a package-test argument.'
        }
        '"' + $argument + '"'
    }
    $process = Start-Process -FilePath $cam -ArgumentList $quoted -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $Stdout -RedirectStandardError $Stderr
    $null = $process.Handle # Cache the owned handle before a fast child exits (Windows PowerShell 5.1).
    try {
        if (!$process.WaitForExit(60000)) {
            $process.Kill()
            $process.WaitForExit()
            throw "Installed Cam command timed out; see $Stderr"
        }
        if ($process.ExitCode -ne 0) { throw "Installed Cam exited $($process.ExitCode); see $Stderr" }
    } finally { $process.Dispose() }
}
if ($HighResolutionProject) { $HighResolutionProject = (Resolve-Path -LiteralPath $HighResolutionProject).Path }
try {
    $env:CAM_FFMPEG = $null
    $env:CAM_FFPROBE = $null
    $env:CAM_ENCODER = 'software'
    Set-Location $root
    $cam = Join-Path $target 'cam.exe'
    $gui = Start-Process -FilePath $cam -WorkingDirectory $root -PassThru -RedirectStandardOutput (Join-Path $root 'gui.stdout.log') -RedirectStandardError (Join-Path $root 'gui.stderr.log')
    try {
        # Wait for winit to create the GUI window before inspecting its modules.
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        do { Start-Sleep -Milliseconds 100; $gui.Refresh() } while (!$gui.HasExited -and !$gui.MainWindowHandle -and [DateTime]::UtcNow -lt $deadline)
        if ($gui.HasExited -or !$gui.MainWindowHandle) { throw 'Installed GUI did not open its window.' }
        # Winit creates the HWND before asynchronous GPU/device startup ends.
        Start-Sleep -Seconds 3
        $gui.Refresh()
        # Probe from a detached helper so the test never detaches the caller's
        # own terminal. A GUI process must have no console to attach to.
        $probePath = Join-Path $root 'console-probe.ps1'
        @'
param([uint32]$GuiId, [string]$OutputFile, [string]$CamExe, [string]$TestRoot)
Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public static class CamSetupConsoleProbe { [DllImport("kernel32.dll",SetLastError=true)] public static extern bool AttachConsole(uint id); [DllImport("kernel32.dll")] public static extern bool FreeConsole(); }'
[void][CamSetupConsoleProbe]::FreeConsole()
$attached = [CamSetupConsoleProbe]::AttachConsole($GuiId)
$code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
if ($attached) { [void][CamSetupConsoleProbe]::FreeConsole() }
$record = Start-Process -FilePath $CamExe -ArgumentList @('record','--area','100,100,320,200','--duration','1','--fps','10','--no-system-audio','--dir',('"{0}"' -f $TestRoot),'--name','Detached') -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $TestRoot 'detached.stdout.log') -RedirectStandardError (Join-Path $TestRoot 'detached.stderr.log')
$null = $record.Handle
if (!$record.WaitForExit(30000)) { $record.Kill(); throw 'Detached recording timed out.' }
[IO.File]::WriteAllText($OutputFile, (@{attached=$attached;error_code=$code;detached_record_exit=$record.ExitCode} | ConvertTo-Json))
'@ | Set-Content -LiteralPath $probePath -Encoding UTF8
        $consolePath = Join-Path $root 'console-probe.json'
        $helper = Start-Process -FilePath $ps -ArgumentList @('-NoProfile','-File',('"{0}"' -f $probePath),'-GuiId',$gui.Id,'-OutputFile',('"{0}"' -f $consolePath),'-CamExe',('"{0}"' -f $cam),'-TestRoot',('"{0}"' -f $root)) -WindowStyle Hidden -PassThru
        if (!$helper.WaitForExit(45000)) { $helper.Kill(); throw 'GUI console probe timed out.' }
        if ($helper.ExitCode -ne 0) { throw 'GUI console probe failed.' }
        $console = Get-Content -LiteralPath $consolePath -Raw | ConvertFrom-Json
        if ($console.attached -or $console.error_code -ne 6) { throw 'Installed GUI unexpectedly has a console window.' }
        if ($console.detached_record_exit -ne 0) { throw 'Detached CLI recording failed; see detached.stderr.log.' }
        foreach ($dll in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
            $module = $gui.Modules | Where-Object ModuleName -eq $dll
            if (!$module -or $module.FileName -ne (Join-Path $target $dll)) { throw "GUI did not load its app-local $dll" }
        }
    } finally {
        if (!$gui.HasExited) { $null = $gui.CloseMainWindow(); if (!$gui.WaitForExit(15000)) { $gui.Kill(); throw 'Installed GUI did not close cleanly.' } }
    }
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    if ($HighResolutionProject) {
        # Exercise the native codecs with the packaged app-local C++ runtime.
        $env:CAM_GPU_PREVIEW = 'require'
        Invoke-CamCheck @('bench-preview',$HighResolutionProject,'--scrub','--width','1280','--height','720') (Join-Path $root 'native-preview.json') (Join-Path $root 'native-preview.log')
        $preview = Get-Content (Join-Path $root 'native-preview.json') -Raw | ConvertFrom-Json
        if ($null -eq $preview.final_frame_mse -or $preview.final_frame_mse -ge 100) { throw 'Installed native preview failed pixel comparison.' }
        $env:CAM_ENCODER = $null
        $env:CAM_GPU_CAPTURE = 'require'
        Invoke-CamCheck @('record','--area','100,100,320,200','--duration','1','--fps','10','--no-system-audio','--dir',$root,'--name','NativeRecorded') (Join-Path $root 'native-record.stdout.log') (Join-Path $root 'native-record.log')
        if (!(Select-String -LiteralPath (Join-Path $root 'native-record.log') -Pattern 'GPU' -Quiet)) { throw 'Installed native GPU capture was not selected.' }
        & (Join-Path $target 'ffmpeg.exe') -v error -nostdin -i (Join-Path $root 'NativeRecorded.cam\recording\screen.mov') -f null - 2> (Join-Path $root 'native-record-decode.log')
        if ($LASTEXITCODE -ne 0) { throw 'Installed native recording failed independent decoding.' }
        $env:CAM_GPU_CAPTURE = $previousGpuCapture
        $env:CAM_GPU_PREVIEW = $previousGpuPreview
        $env:CAM_ENCODER = 'software'
    }
    Invoke-CamCheck @('record','--area','100,100,320,200','--duration','1','--fps','10','--no-system-audio','--dir',$root,'--name','Recorded') (Join-Path $root 'record.stdout.log') (Join-Path $root 'record.log')
    & (Join-Path $target 'ffmpeg.exe') -v error -nostdin -i (Join-Path $root 'Recorded.cam\recording\screen.mov') -f null - 2> (Join-Path $root 'record-decode.log')
    if ($LASTEXITCODE -ne 0) { throw 'Installed screen recording failed independent decoding.' }
    $project = Join-Path $root 'Media.cam'
    Invoke-CamCheck @('synth',$project,'--duration','1','--width','160','--height','90') (Join-Path $root 'synth.stdout.log') (Join-Path $root 'synth.log')
    & (Join-Path $target 'ffmpeg.exe') -v error -f lavfi -i 'sine=frequency=1234:duration=1' -ar 48000 -ac 2 (Join-Path $project 'recording\mic.wav')
    if ($LASTEXITCODE -ne 0) { throw 'Installed audio encoding failed.' }
    $metaPath = Join-Path $project 'recording\meta.json'
    $meta = Get-Content $metaPath -Raw | ConvertFrom-Json
    $meta | Add-Member -NotePropertyName mic -NotePropertyValue @{file='mic.wav';track=0} -Force
    $meta | ConvertTo-Json -Depth 12 | Set-Content $metaPath -Encoding UTF8
    $output = Join-Path $root 'Export.mp4'
    Invoke-CamCheck @('export',$project,'--output',$output,'--height','96','--fps','10','--quality','web') (Join-Path $root 'export.stdout.log') (Join-Path $root 'export.log')
    & (Join-Path $target 'ffmpeg.exe') -v error -nostdin -i $output -f null - 2> (Join-Path $root 'decode.log')
    if ($LASTEXITCODE -ne 0) { throw 'Installed export failed full decoding.' }
    $probe = & (Join-Path $target 'ffprobe.exe') -v error -show_streams -of json $output | ConvertFrom-Json
    if (!($probe.streams | Where-Object codec_name -eq 'h264') -or !($probe.streams | Where-Object codec_name -eq 'aac')) { throw 'Export is missing expected video or audio.' }
} finally {
    $env:PATH = $previousPath
    $env:CAM_FFMPEG = $previousFfmpeg
    $env:CAM_FFPROBE = $previousFfprobe
    $env:CAM_ENCODER = $previousEncoder
    $env:CAM_GPU_CAPTURE = $previousGpuCapture
    $env:CAM_GPU_PREVIEW = $previousGpuPreview
    Set-Location (Split-Path $PSScriptRoot -Parent)
}
& $ps -NoProfile -ExecutionPolicy Bypass -File (Join-Path $target 'Uninstall.ps1') -Quiet > (Join-Path $root 'uninstall.log') 2>&1
if ($LASTEXITCODE -ne 0 -or (Test-Path (Join-Path $target 'cam.exe')) -or (Get-Content (Join-Path $target 'keep-me.txt')) -ne 'user data') { throw 'Uninstall failed to preserve user files.' }
if ($Integration -and ((Test-Path $key) -or @($links | Where-Object { Test-Path -LiteralPath $_ }).Count)) { throw 'Uninstall left registration/shortcuts behind.' }
@{status='PASS';checks=@('self-extraction','Windows PowerShell 5.1 installation','update preserves user files','corrupt download preserves installation','GUI has no console window','detached CLI recording without a console','GUI loads app-local C++ runtime','real screen recording without tools on PATH','H.264/AAC export without tools on PATH','full independent decode','uninstall preserves user files');download_tested=[bool]$Download;integration_tested=[bool]$Integration;artifacts=$root} |
    ForEach-Object { $_.high_resolution_tested = [bool]$HighResolutionProject; $_ } |
    ConvertTo-Json -Depth 4 | Tee-Object -FilePath (Join-Path $root 'result.json')
