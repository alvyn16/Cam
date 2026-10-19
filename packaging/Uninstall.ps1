param([switch]$Quiet)
$ErrorActionPreference = 'Stop'
$target = [IO.Path]::GetFullPath($PSScriptRoot).TrimEnd('\')
$marker = Join-Path $target 'cam-install.json'
if (!(Test-Path -LiteralPath $marker) -or (Get-Content $marker -Raw | ConvertFrom-Json).product -ne 'Cam') { throw 'This folder is not a Cam Setup installation.' }
$installed = Get-Content $marker -Raw | ConvertFrom-Json
if ($target -eq [IO.Path]::GetPathRoot($target).TrimEnd('\')) { throw 'Invalid installation folder.' }
if (!$Quiet) {
    Add-Type -AssemblyName System.Windows.Forms
    if ([Windows.Forms.MessageBox]::Show('Uninstall Cam? Your recordings and settings will be kept.','Cam','YesNo','Question') -ne 'Yes') { exit 0 }
}
if (Get-Process cam -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $target 'cam.exe') }) { throw 'Close Cam before uninstalling.' }
$shell = New-Object -ComObject WScript.Shell
foreach ($folder in @([Environment]::GetFolderPath('Desktop'),[Environment]::GetFolderPath('Programs'))) {
    $link = Join-Path $folder 'Cam.lnk'
    if ((Test-Path -LiteralPath $link) -and $shell.CreateShortcut($link).TargetPath -eq (Join-Path $target 'cam.exe')) { Remove-Item -LiteralPath $link -Force }
}
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Cam'
if ((Get-ItemProperty $key -ErrorAction SilentlyContinue).InstallLocation -eq $target) { Remove-Item -LiteralPath $key -Recurse }
# Delete only files owned by setup, preserving any additional user files.
foreach ($dll in $installed.runtime_dlls) {
    if ([IO.Path]::GetFileName($dll.name) -ne $dll.name -or !($dll.name -like '*.dll')) { throw 'Invalid runtime file name in installation marker.' }
    $owned = Join-Path $target $dll.name
    if (Test-Path -LiteralPath $owned) { Remove-Item -LiteralPath $owned -Force }
}
foreach ($name in @('cam.exe','ffmpeg.exe','ffprobe.exe','cam-install.json','Uninstall.ps1')) {
    $owned = Join-Path $target $name
    if (Test-Path -LiteralPath $owned) { Remove-Item -LiteralPath $owned -Force }
}
$licenses = Join-Path $target 'licenses'
if (Test-Path -LiteralPath $licenses) { Remove-Item -LiteralPath $licenses -Recurse -Force }
if (@(Get-ChildItem -LiteralPath $target -Force).Count -eq 0) { Remove-Item -LiteralPath $target }
