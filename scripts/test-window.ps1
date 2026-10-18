param([switch]$Release)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$binary = if ($Release) { 'target/release/cam.exe' } else { 'target/debug/cam.exe' }
$testDir = Join-Path $env:TEMP ('cam-window-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testDir | Out-Null
$form = New-Object Windows.Forms.Form
$form.Text = 'Cam window capture verification'
$form.Size = New-Object Drawing.Size(400,300)
$form.StartPosition = 'Manual'
$form.Location = New-Object Drawing.Point(120,120)
$form.BackColor = [Drawing.Color]::Blue
$script:captureProcess = $null
$script:tick = 0
$timer = New-Object Windows.Forms.Timer
$timer.Interval = 100
$timer.Add_Tick({
    $script:tick++
    if ($script:tick -eq 12) { $form.BackColor = [Drawing.Color]::Red; $form.Refresh() }
    if ($script:tick -eq 15) { $form.Location = New-Object Drawing.Point(350,180) }
    if (($script:captureProcess -and $script:captureProcess.HasExited) -or $script:tick -gt 100) { $timer.Stop(); $form.Close() }
})
$form.Add_Shown({
    $script:captureProcess = Start-Process -FilePath (Join-Path $PWD $binary) -ArgumentList @('record','--window',$form.Handle.ToInt64().ToString(),'--duration','2','--fps','30','--no-system-audio','--dir',('"' + $testDir + '"'),'--name','Window') -PassThru -WindowStyle Hidden -RedirectStandardOutput (Join-Path $testDir 'stdout.log') -RedirectStandardError (Join-Path $testDir 'stderr.log')
    $timer.Start()
})
[void]$form.ShowDialog()
$form.Dispose()
$timer.Dispose()
if (-not $script:captureProcess.HasExited) { Stop-Process -Id $script:captureProcess.Id; throw "Window recording timed out: $testDir" }
if ($script:captureProcess.ExitCode -ne 0) { throw "Window recording failed: $testDir" }
& ffmpeg -v error -i (Join-Path $testDir 'Window.cam/recording/screen.mov') -vf 'fps=5' -y (Join-Path $testDir 'frame%d.png')
if ($LASTEXITCODE -ne 0) { throw 'Independent decoding failed.' }
$blue = 0; $red = 0; $black = 0
foreach ($file in Get-ChildItem $testDir -Filter 'frame*.png') {
    $bitmap = [Drawing.Bitmap]::FromFile($file.FullName)
    $pixel = $bitmap.GetPixel(200,150)
    $bitmap.Dispose()
    if ($pixel.B -gt 200 -and $pixel.R -lt 30) { $blue++ }
    if ($pixel.R -gt 200 -and $pixel.B -lt 30) { $red++ }
    if ($pixel.R -lt 10 -and $pixel.G -lt 10 -and $pixel.B -lt 10) { $black++ }
}
if ($blue -lt 1 -or $red -lt 1 -or $black -gt 0) { throw "Window frames incorrect: blue=$blue red=$red black=$black; artifacts: $testDir" }
Write-Output "Window movement/content test passed: blue=$blue red=$red black=$black; artifacts: $testDir"
