param([switch]$Hardware, [switch]$Gui)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
& cargo test -j 1 --all-targets --no-fail-fast -- --test-threads=1
if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
if ($Hardware) {
    & cargo build -j 1
    if ($LASTEXITCODE -ne 0) { throw 'Native worker build failed.' }
    & cargo test -j 1 --lib -- --ignored --test-threads=1 --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Native audio/GPU hardware checks failed.' }
    & cargo test -j 1 --test windows_live -- --ignored --test-threads=1 --nocapture
    if ($LASTEXITCODE -ne 0) { throw 'Live microphone/webcam/control test failed.' }
    & (Join-Path $PSScriptRoot 'test-window.ps1')
}
if ($Gui) {
    & cargo build -j 1
    if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
    $previousSelftest = $env:CAM_GUI_SELFTEST
    $previousOutput = $env:CAM_OUTPUT_DIR
    $testOutput = Join-Path $env:TEMP ('cam-gui-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $testOutput | Out-Null
    try {
        $env:CAM_GUI_SELFTEST = '1'
        $env:CAM_OUTPUT_DIR = $testOutput
        $process = Start-Process -FilePath (Join-Path $PWD 'target/debug/cam.exe') -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $testOutput 'stdout.log') -RedirectStandardError (Join-Path $testOutput 'stderr.log')
        if (-not $process.WaitForExit(180000)) {
            Stop-Process -Id $process.Id
            throw "GUI test timed out; logs: $testOutput"
        }
        $log = Get-Content (Join-Path $testOutput 'stderr.log') -Raw
        if ($process.ExitCode -ne 0 -or $log -notmatch 'selftest: PASS') { throw "GUI test failed; logs: $testOutput" }
        Write-Output "GUI test passed; artifacts: $testOutput"
    } finally {
        $env:CAM_GUI_SELFTEST = $previousSelftest
        $env:CAM_OUTPUT_DIR = $previousOutput
    }
}
