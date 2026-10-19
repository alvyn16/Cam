param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Cam'),
    [switch]$Quiet,
    [switch]$NoLaunch,
    [switch]$NoShortcuts,
    [switch]$NoRegistration,
    [string]$RuntimeArchive
)
$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$target = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')
if ($target -eq [IO.Path]::GetPathRoot($target).TrimEnd('\')) { throw 'Choose an application folder, not a drive root.' }
$work = Join-Path $env:TEMP ('Cam-Setup-' + [guid]::NewGuid().ToString('N'))
$stage = $null
$backup = $null
$form = $null
$client = $null
$cancelled = $false
function Assert-ChildPath([string]$parent, [string]$child) {
    $prefix = [IO.Path]::GetFullPath($parent).TrimEnd('\') + '\'
    if (![IO.Path]::GetFullPath($child).StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe installation path.' }
}
Assert-ChildPath $env:TEMP $work
function Set-ProgressText([string]$text) {
    Write-Output $text
    if ($form) { $label.Text = $text; [Windows.Forms.Application]::DoEvents() }
}
try {
    if (!$Quiet) {
        Add-Type -AssemblyName System.Windows.Forms
        Add-Type -AssemblyName System.Drawing
        $form = New-Object Windows.Forms.Form
        $form.Text = 'Cam Setup'
        $form.ClientSize = New-Object Drawing.Size(470,170)
        $form.StartPosition = 'CenterScreen'
        $form.FormBorderStyle = 'FixedDialog'
        $form.MaximizeBox = $false
        $form.BackColor = [Drawing.Color]::FromArgb(248,249,251)
        $form.Font = New-Object Drawing.Font('Segoe UI',10)
        $label = New-Object Windows.Forms.Label
        $label.SetBounds(24,20,422,72)
        $bar = New-Object Windows.Forms.ProgressBar
        $bar.SetBounds(24,105,422,18)
        $bar.Style = 'Marquee'
        $form.Controls.AddRange(@($label,$bar))
        $form.Add_FormClosing({ $script:cancelled = $true })
        $form.Show()
    }
    $manifest = Get-Content (Join-Path $PSScriptRoot 'runtime.json') -Raw | ConvertFrom-Json
    $package = Get-Content (Join-Path $PSScriptRoot 'package.json') -Raw | ConvertFrom-Json
    if ((Get-FileHash (Join-Path $PSScriptRoot 'cam.exe')).Hash -ne $package.cam_sha256) { throw 'Cam package checksum does not match.' }
    foreach ($dll in $package.runtime_dlls) {
        if ([IO.Path]::GetFileName($dll.name) -ne $dll.name -or (Get-FileHash (Join-Path $PSScriptRoot $dll.name)).Hash -ne $dll.sha256) { throw 'C++ runtime package checksum does not match.' }
    }
    if (Test-Path -LiteralPath $target) {
        if ((Get-Item -LiteralPath $target).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Choose an installation folder that is not a link.' }
        $marker = Join-Path $target 'cam-install.json'
        if (!(Test-Path -LiteralPath $marker) -or (Get-Content $marker -Raw | ConvertFrom-Json).product -ne 'Cam') {
            throw "The destination already exists and is not a Cam Setup installation: $target"
        }
        if (Get-Process cam -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $target 'cam.exe') }) {
            throw 'Close Cam before installing this update, then run setup again.'
        }
    }
    New-Item -ItemType Directory -Path $work | Out-Null
    $archive = Join-Path $work 'ffmpeg.zip'
    if ($RuntimeArchive) {
        Copy-Item -LiteralPath $RuntimeArchive -Destination $archive
    } else {
        Set-ProgressText 'Downloading media tools (about 110 MB). Internet is required only for setup.'
        $client = New-Object Net.WebClient
        $client.Headers['User-Agent'] = 'Cam-Setup/' + $package.version
        $download = $client.DownloadFileTaskAsync([uri]$manifest.url, $archive)
        $timer = [Diagnostics.Stopwatch]::StartNew()
        while (!$download.IsCompleted) {
            if ($form) { [Windows.Forms.Application]::DoEvents() }
            if ($cancelled -or $timer.Elapsed.TotalMinutes -gt 10) { $client.CancelAsync(); throw 'Download cancelled or timed out. Run setup again to retry.' }
            Start-Sleep -Milliseconds 100
        }
        $download.GetAwaiter().GetResult()
    }
    if ($cancelled) { throw 'Installation cancelled.' }
    Set-ProgressText 'Verifying the download and preparing Cam...'
    if ((Get-FileHash $archive).Hash -ne $manifest.sha256) { throw 'Media tools checksum failed. Nothing was installed. Run setup again.' }
    Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $work 'runtime')
    $runtimeRoot = Join-Path $work ('runtime\ffmpeg-' + $manifest.version)
    foreach ($tool in @('ffmpeg','ffprobe')) {
        $toolPath = Join-Path $runtimeRoot "bin\$tool.exe"
        if (!(Test-Path -LiteralPath $toolPath)) { throw "Media download is missing $tool.exe" }
        & $toolPath -version *> $null
        if ($LASTEXITCODE -ne 0) { throw "$tool could not start on this computer." }
    }
    if ($cancelled) { throw 'Installation cancelled.' }
    if ($form) { $form.ControlBox = $false }
    Set-ProgressText "Installing Cam to $target"
    $parent = Split-Path $target -Parent
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $stage = Join-Path $parent ('.Cam-install-' + [guid]::NewGuid().ToString('N'))
    Assert-ChildPath $parent $stage
    Assert-ChildPath $parent $target
    New-Item -ItemType Directory -Path $stage | Out-Null
    # Preserve files a user added inside an existing managed installation.
    if (Test-Path -LiteralPath $target) {
        Get-ChildItem -LiteralPath $target -Force | Copy-Item -Destination $stage -Recurse -Force
    }
    Copy-Item (Join-Path $PSScriptRoot 'cam.exe') $stage
    foreach ($dll in $package.runtime_dlls) { Copy-Item (Join-Path $PSScriptRoot $dll.name) $stage }
    Copy-Item (Join-Path $runtimeRoot 'bin\ffmpeg.exe'),(Join-Path $runtimeRoot 'bin\ffprobe.exe') $stage
    Copy-Item (Join-Path $PSScriptRoot 'Uninstall.ps1') $stage
    $licenses = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Path $licenses -Force | Out-Null
    Copy-Item (Join-Path $PSScriptRoot 'LICENSE*'),(Join-Path $PSScriptRoot 'THIRD_PARTY_NOTICES.md') $licenses
    Copy-Item (Join-Path $runtimeRoot 'LICENSE') (Join-Path $licenses 'FFmpeg-LICENSE.txt')
    Copy-Item (Join-Path $runtimeRoot 'README.txt') (Join-Path $licenses 'FFmpeg-README.txt')
    @{product='Cam';version=$package.version;commit=$package.commit;cam_sha256=$package.cam_sha256;runtime=$manifest;runtime_dlls=$package.runtime_dlls;installed_utc=[DateTime]::UtcNow.ToString('o')} |
        ConvertTo-Json -Depth 6 | Set-Content (Join-Path $stage 'cam-install.json') -Encoding UTF8
    if (Test-Path -LiteralPath $target) {
        $backup = Join-Path $parent ('.Cam-previous-' + [guid]::NewGuid().ToString('N'))
        Assert-ChildPath $parent $backup
        Move-Item -LiteralPath $target -Destination $backup
    }
    try { Move-Item -LiteralPath $stage -Destination $target; $stage = $null }
    catch { if ($backup) { Move-Item -LiteralPath $backup -Destination $target; $backup = $null }; throw }
    if (!$NoShortcuts) {
        $shell = New-Object -ComObject WScript.Shell
        foreach ($folder in @([Environment]::GetFolderPath('Desktop'),[Environment]::GetFolderPath('Programs'))) {
            $shortcut = $shell.CreateShortcut((Join-Path $folder 'Cam.lnk'))
            $shortcut.TargetPath = Join-Path $target 'cam.exe'
            $shortcut.WorkingDirectory = $target
            $shortcut.IconLocation = (Join-Path $target 'cam.exe') + ',0'
            $shortcut.Save()
        }
    }
    if (!$NoRegistration) {
        $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Cam'
        New-Item $key -Force | Out-Null
        $uninstall = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $target 'Uninstall.ps1') + '"'
        $values = @{DisplayName='Cam';DisplayVersion=$package.version;Publisher='alvyn16';InstallLocation=$target;DisplayIcon=(Join-Path $target 'cam.exe');UninstallString=$uninstall;URLInfoAbout='https://github.com/alvyn16/Cam'}
        foreach ($entry in $values.GetEnumerator()) { New-ItemProperty $key -Name $entry.Key -Value $entry.Value -PropertyType String -Force | Out-Null }
    }
    # Only remove the uniquely named backup that this invocation created.
    if ($backup) { Remove-Item -LiteralPath $backup -Recurse -Force; $backup = $null }
    Set-ProgressText 'Cam is ready. You can open it from your desktop or Start menu.'
    if (!$Quiet) { [Windows.Forms.MessageBox]::Show('Cam is installed and ready to use.','Cam Setup','OK','Information') | Out-Null }
    if (!$NoLaunch) { Start-Process -FilePath (Join-Path $target 'cam.exe') -WorkingDirectory $target }
} catch {
    if (!$Quiet) { [Windows.Forms.MessageBox]::Show($_.Exception.Message,'Cam Setup','OK','Error') | Out-Null }
    Write-Error $_
    exit 1
} finally {
    if ($client) { $client.Dispose() }
    if ($form) { $form.Dispose() }
    # Both directories are generated by this invocation, never user-supplied.
    if ($stage -and (Test-Path -LiteralPath $stage)) { Remove-Item -LiteralPath $stage -Recurse -Force }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
