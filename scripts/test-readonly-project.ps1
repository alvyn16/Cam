param(
    [Parameter(Mandatory=$true)][string]$Project,
    [switch]$Release
)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Join-Path $env:TEMP ('cam-readonly-final-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$package = Join-Path $root 'Readonly.cam'
Copy-Item -LiteralPath (Resolve-Path -LiteralPath $Project).Path -Destination $package -Recurse
$profile = if ($Release) { 'release' } else { 'debug' }
$exe = Join-Path $root 'cam.exe'
Copy-Item -LiteralPath "target/$profile/cam.exe" -Destination $exe
function Manifest {
    @(Get-ChildItem -LiteralPath $package -Recurse -File | Sort-Object FullName | ForEach-Object {
        "$($_.FullName.Substring($package.Length)) $((Get-FileHash -LiteralPath $_.FullName).Hash)"
    }) -join "`n"
}
$before = Manifest
$acl = Get-Acl -LiteralPath $package
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$rights = [Security.AccessControl.FileSystemRights]'WriteData,AppendData,WriteExtendedAttributes,WriteAttributes,DeleteSubdirectoriesAndFiles,Delete'
$rule = [Security.AccessControl.FileSystemAccessRule]::new($identity,$rights,'ContainerInherit,ObjectInherit','None','Deny')
$denied = Get-Acl -LiteralPath $package
$denied.AddAccessRule($rule)
$oldNative = $env:CAM_NATIVE_ENCODER
$oldEncoder = $env:CAM_ENCODER
try {
    Set-Acl -LiteralPath $package -AclObject $denied
    $blocked = $false
    try { [IO.File]::WriteAllText((Join-Path $package 'write-probe'),'unexpected') } catch [UnauthorizedAccessException] { $blocked = $true }
    if (-not $blocked) { throw 'Read-only negative control failed.' }
    [void][IO.File]::ReadAllText((Join-Path $package 'project.json'))
    $env:CAM_NATIVE_ENCODER='off'; $env:CAM_ENCODER='software'
    & $exe snapshot $package --tab audio --at 2 -o (Join-Path $root 'preview.png') *> (Join-Path $root 'preview.log')
    if ($LASTEXITCODE -ne 0) { throw 'Read-only preview failed.' }
    $output = Join-Path $root 'fallback.mp4'
    & $exe export $package --height 360 --fps 10 -o $output *> (Join-Path $root 'export.log')
    if ($LASTEXITCODE -ne 0) { throw 'Read-only export failed.' }
    & ffmpeg -v error -nostdin -i $output -f null NUL
    if ($LASTEXITCODE -ne 0) { throw 'Independent decoding failed.' }
    if ((Manifest) -cne $before) { throw 'Source files changed.' }
    @{Status='PASS'; WritesDenied=$blocked; SourceUnchanged=$true; SourceFiles=@(Get-ChildItem -LiteralPath $package -Recurse -File).Count; SHA256=(Get-FileHash $exe).Hash; Artifacts=$root} | ConvertTo-Json | Tee-Object -FilePath (Join-Path $root 'result.json')
} finally {
    Set-Acl -LiteralPath $package -AclObject $acl
    $env:CAM_NATIVE_ENCODER=$oldNative; $env:CAM_ENCODER=$oldEncoder
}
