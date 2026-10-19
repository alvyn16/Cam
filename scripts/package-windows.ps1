param(
    [string]$CamExe = (Join-Path $PSScriptRoot '..\target\release\cam.exe'),
    [string]$OutputDir = (Join-Path $PSScriptRoot '..\dist'),
    [string]$VCRuntimeDir
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$exe = (Resolve-Path -LiteralPath $CamExe).Path
$version = ((Get-Content (Join-Path $repo 'Cargo.toml') | Select-String '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value)
$commit = (& git -C $repo rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { throw 'Cannot identify source commit.' }
$output = [IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Path $output -Force | Out-Null
$build = Join-Path $env:TEMP ('cam-package-' + [guid]::NewGuid().ToString('N'))
$payload = Join-Path $build 'payload'
New-Item -ItemType Directory -Path $payload -Force | Out-Null
Copy-Item -LiteralPath $exe -Destination (Join-Path $payload 'cam.exe')
if (!$VCRuntimeDir) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (!(Test-Path -LiteralPath $vswhere)) { throw 'Specify -VCRuntimeDir with the Visual Studio x64 release CRT directory.' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $VCRuntimeDir = Get-ChildItem (Join-Path $vs 'VC\Redist\MSVC') -Directory | Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\Microsoft.VC143.CRT' } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
foreach ($dll in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
    if (!$VCRuntimeDir -or !(Test-Path -LiteralPath (Join-Path $VCRuntimeDir $dll))) { throw "Missing release C++ runtime: $dll" }
}
Copy-Item (Join-Path $VCRuntimeDir '*.dll') $payload
Copy-Item (Join-Path $repo 'packaging\Install.ps1'),(Join-Path $repo 'packaging\Uninstall.ps1'),(Join-Path $repo 'packaging\runtime.json') $payload
Copy-Item (Join-Path $repo 'LICENSE-MIT'),(Join-Path $repo 'LICENSE-APACHE'),(Join-Path $repo 'THIRD_PARTY_NOTICES.md') $payload
Copy-Item (Join-Path $repo 'assets\fonts\OFL.txt') (Join-Path $payload 'LICENSE-Inter.txt')
$noise = Join-Path $env:USERPROFILE '.cargo\registry\src'
$noiseLicense = @(Get-ChildItem -LiteralPath $noise -Directory | ForEach-Object { Join-Path $_.FullName 'nnnoiseless-0.5.2\COPYING' } | Where-Object { Test-Path -LiteralPath $_ })
if ($noiseLicense.Count -eq 0) { throw 'nnnoiseless license missing; fetch Cargo dependencies first.' }
Copy-Item -LiteralPath $noiseLicense[0] -Destination (Join-Path $payload 'LICENSE-RNNoise.txt')
$dlls = @(Get-ChildItem $payload -Filter '*.dll' | ForEach-Object { @{name=$_.Name;sha256=(Get-FileHash $_.FullName).Hash} })
@{version=$version;commit=$commit;cam_sha256=(Get-FileHash $exe).Hash;runtime_dlls=$dlls} | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $payload 'package.json') -Encoding UTF8
@'
@echo off
setlocal
set "PSModulePath=%SystemRoot%\System32\WindowsPowerShell\v1.0\Modules"
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0Install.ps1" %*
exit /b %errorlevel%
'@ | Set-Content (Join-Path $payload 'Install.cmd') -Encoding ASCII
$names = @(Get-ChildItem -LiteralPath $payload -File | Select-Object -ExpandProperty Name)
$setup = Join-Path $output "Cam-$version-Windows-x64-Setup.exe"
if (Test-Path -LiteralPath $setup) { throw "Output already exists: $setup" }
$definitions = for ($i = 0; $i -lt $names.Count; $i++) { "FILE$i=`"$($names[$i])`"" }
$references = for ($i = 0; $i -lt $names.Count; $i++) { "%FILE$i%=" }
$sed = @"
[Version]
Class=IEXPRESS
SEDVersion=3
[Options]
PackagePurpose=InstallApp
ShowInstallProgramWindow=0
HideExtractAnimation=1
UseLongFileName=1
InsideCompressed=0
CAB_FixedSize=0
CAB_ResvCodeSigning=0
RebootMode=N
InstallPrompt=
DisplayLicense=
FinishMessage=
TargetName=$setup
FriendlyName=Cam Setup
AppLaunched=cmd.exe /d /c .\Install.cmd
PostInstallCmd=<None>
AdminQuietInstCmd=cmd.exe /d /c .\Install.cmd -Quiet
UserQuietInstCmd=cmd.exe /d /c .\Install.cmd -Quiet
SourceFiles=SourceFiles
[Strings]
$($definitions -join "`r`n")
[SourceFiles]
SourceFiles0=$payload\
[SourceFiles0]
$($references -join "`r`n")
"@
$sedPath = Join-Path $build 'setup.sed'
$sed | Set-Content $sedPath -Encoding ASCII
$process = Start-Process -FilePath "$env:SystemRoot\System32\iexpress.exe" -ArgumentList '/N /Q setup.sed' -WorkingDirectory $build -WindowStyle Hidden -PassThru
if (!$process.WaitForExit(120000)) { $process.Kill(); throw "IExpress timed out. Build files: $build" }
if ($process.ExitCode -ne 0 -or !(Test-Path -LiteralPath $setup)) { throw "IExpress failed. Build files: $build" }
$archive = Join-Path $output "Cam-$version-Windows-x64-Setup.zip"
Compress-Archive -Path (Join-Path $payload '*') -DestinationPath $archive
foreach ($artifact in @($setup,$archive)) {
    ((Get-FileHash $artifact).Hash.ToLowerInvariant() + '  ' + (Split-Path $artifact -Leaf)) | Set-Content ($artifact + '.sha256') -Encoding ASCII
}
@{setup=$setup;zip=$archive;payload=$payload;version=$version;commit=$commit;cam_sha256=(Get-FileHash $exe).Hash} |
    ConvertTo-Json | Tee-Object -FilePath (Join-Path $output 'package-result.json')
