#Requires -Version 5.1
<#
.SYNOPSIS
Install Monica CLI and its portable data directory for the current Windows user.
.EXAMPLE
.\scripts\install.ps1 -InstallDir D:\Apps\MonicaCLI
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\MonicaCLI'),
    [string]$Source,
    [switch]$NoPath,
    [switch]$NoShortcut
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This installer requires Windows.' }
if ([string]::IsNullOrWhiteSpace($Source)) {
    $Source = Join-Path $PSScriptRoot '..\target\release\monica-pass.exe'
}
if (-not [System.IO.Path]::IsPathRooted($InstallDir)) {
    throw 'InstallDir must be an absolute path.'
}
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir).TrimEnd('\', '/')
if ($InstallDir -eq [System.IO.Path]::GetPathRoot($InstallDir).TrimEnd('\', '/')) {
    throw 'Choose a dedicated application directory, not a drive root.'
}
$sourceFile = Get-Item -LiteralPath $Source
if ($sourceFile.PSIsContainer) { throw 'Source must be a monica-pass.exe file.' }
$sourcePath = $sourceFile.FullName
$executable = Join-Path $InstallDir 'monica-pass.exe'
$marker = Join-Path $InstallDir 'monica-pass.portable'
$dataDir = Join-Path $InstallDir 'data'

# A junction must not redirect an installation or its private data elsewhere.
$ancestor = $InstallDir
while ($ancestor) {
    if (Test-Path -LiteralPath $ancestor) {
        $item = Get-Item -LiteralPath $ancestor -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            throw 'The installation path must contain only ordinary directories.'
        }
    }
    $ancestor = [System.IO.Path]::GetDirectoryName($ancestor)
}
if (Test-Path -LiteralPath $InstallDir) {
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
        if (@(Get-ChildItem -LiteralPath $InstallDir -Force).Count -ne 0) {
            throw 'This nonempty directory is not a portable Monica installation. Choose a new directory.'
        }
    }
}
foreach ($existingPath in @($marker, $executable, $dataDir)) {
    if (Test-Path -LiteralPath $existingPath) {
        $item = Get-Item -LiteralPath $existingPath -Force
        if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
            throw 'Installation files and data must not be symbolic links or junctions.'
        }
        if (($existingPath -eq $dataDir) -ne $item.PSIsContainer) {
            throw 'An existing installation path has the wrong file type.'
        }
    }
}
if ((Test-Path -LiteralPath $marker) -and (Get-Item -LiteralPath $marker).Length -ne 0) {
    throw 'The portable marker must be an empty file.'
}

$running = Get-Process -Name 'monica-pass', 'monica', 'monicapass' -ErrorAction SilentlyContinue |
    Where-Object { [System.IO.Path]::GetDirectoryName($_.Path) -eq $InstallDir }
if ($running) { throw 'Close the installed Monica CLI before updating it.' }

function Set-PrivateDirectory([string]$Path) {
    $userSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $acl.SetOwner($userSid)
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @($userSid, [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'), [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))) {
        $rule = [System.Security.AccessControl.FileSystemAccessRule]::new(
            $sid, [System.Security.AccessControl.FileSystemRights]::FullControl,
            [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
            [System.Security.AccessControl.PropagationFlags]::None,
            [System.Security.AccessControl.AccessControlType]::Allow
        )
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Set-PrivateDirectory $InstallDir
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
Set-PrivateDirectory $dataDir

# Stage and validate the program before replacing the installed executable.
$staging = Join-Path $InstallDir ('.install-' + [guid]::NewGuid().ToString('N') + '.exe')
try {
    [System.IO.File]::Copy($sourcePath, $staging, $false)
    $version = & $staging --version
    if ($LASTEXITCODE -ne 0 -or $version -notmatch '^monica(?:-?pass)? [0-9]+\.') {
        throw 'The source executable did not pass its version check.'
    }
    if (Test-Path -LiteralPath $executable) {
        [System.IO.File]::Replace($staging, $executable, [NullString]::Value)
    } else {
        [System.IO.File]::Move($staging, $executable)
    }
} finally {
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging }
}
if (-not (Test-Path -LiteralPath $marker)) {
    [System.IO.File]::WriteAllBytes($marker, [byte[]]@())
}

# Executable aliases work in PowerShell, cmd, and noninteractive AI processes.
foreach ($aliasName in @('monica.exe', 'monicapass.exe')) {
    $aliasPath = Join-Path $InstallDir $aliasName
    if (Test-Path -LiteralPath $aliasPath) {
        $aliasItem = Get-Item -LiteralPath $aliasPath -Force
        if ($aliasItem.PSIsContainer -or ($aliasItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            throw 'An executable alias must be an ordinary file.'
        }
    }
    [System.IO.File]::Copy($executable, $aliasPath, $true)
}

if (-not $NoPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    $present = $entries | Where-Object {
        [Environment]::ExpandEnvironmentVariables($_).Trim('"').TrimEnd('\', '/') -ieq $InstallDir
    }
    if (-not $present) {
        $updatedPath = (@($InstallDir) + $entries) -join ';'
        [Environment]::SetEnvironmentVariable('Path', $updatedPath, 'User')
    }
    $env:Path = $InstallDir + ';' + $env:Path
}

$shortcutPath = $null
if (-not $NoShortcut) {
    $shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'Monica CLI.lnk'
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($shortcutPath)
    if ((Test-Path -LiteralPath $shortcutPath) -and $shortcut.TargetPath -ne $executable) {
        throw 'A shortcut for another Monica installation already exists. Rerun with -NoShortcut.'
    }
    $shortcut.TargetPath = $executable
    $shortcut.WorkingDirectory = $InstallDir
    $shortcut.Description = 'Monica CLI credential manager'
    $shortcut.Save()
}

[pscustomobject]@{
    Version = $version
    Executable = $executable
    DataDirectory = $dataDir
    Shortcut = $shortcutPath
    AddedToUserPath = -not $NoPath
}
if (-not $NoPath) { Write-Host 'Open a new terminal, then run: monica (also: monicapass, monica-pass)' }
