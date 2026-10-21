#Requires -Version 5.1
<#! Install the CLI Native Messaging bridge for the current Windows user. #>
[CmdletBinding()]
param(
    [string]$HostPath = (Join-Path $PSScriptRoot '..\monica-browser-host.exe'),
    [string]$EdgeExtensionId,
    [string]$ChromeExtensionId,
    [switch]$Uninstall,
    [switch]$Diagnose
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$hostName = 'app.monica.cli'
$registrations = @('HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\app.monica.cli', 'HKCU:\Software\Google\Chrome\NativeMessagingHosts\app.monica.cli')
function Read-HostMetadata([string]$Executable) {
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable; $start.Arguments = '--diagnose'
    $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true; $start.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::Start($start)
    try {
        if (-not $process.WaitForExit(5000)) { $process.Kill(); throw 'Host diagnostic timed out.' }
        $text = $process.StandardOutput.ReadToEnd()
        if ($process.ExitCode -ne 0 -or $text.Length -gt 16384) { throw 'Host diagnostic failed.' }
        $info = $text | ConvertFrom-Json
        if ($info.host -ne $hostName -or $info.protocol -ne 2 -or [version]$info.version -lt [version]'1.0.101' -or $info.integration_revision -lt 2 -or $info.capabilities -notcontains 'mutation_receipts' -or $info.capabilities -notcontains 'snapshots') { throw 'Update the CLI and browser helper together: incompatible host.' }
        return $info
    } finally { $process.Dispose() }
}
if ($Diagnose) {
    if ($Uninstall) { throw 'Diagnose cannot be combined with Uninstall.' }
    $binary = $null; $binaryState = 'host_missing'
    if (Test-Path -LiteralPath $HostPath -PathType Leaf) {
        try { $binary = Read-HostMetadata (Get-Item -LiteralPath $HostPath).FullName; $binaryState = 'compatible' }
        catch { $binaryState = 'host_incompatible_or_unstartable' }
    }
    $checks = for ($index = 0; $index -lt 2; $index++) {
        $state = 'not_registered'; $manifestFile = $null
        $expectedId = if ($index -eq 0) { $EdgeExtensionId } else { $ChromeExtensionId }
        if ($expectedId -and $expectedId -cnotmatch '^[a-p]{32}$') { throw 'Invalid Chromium extension ID.' }
        if (Test-Path -LiteralPath $registrations[$index]) {
            try {
                $key = Get-Item -LiteralPath $registrations[$index]
                $manifestFile = $key.GetValue('', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $registered = [System.IO.File]::ReadAllText($manifestFile) | ConvertFrom-Json
                $state = 'registered'
                if ($registered.name -ne $hostName -or $registered.type -ne 'stdio') { $state = 'invalid_manifest' }
                elseif (-not (Test-Path -LiteralPath $registered.path -PathType Leaf)) { $state = 'registered_host_missing' }
                elseif ($expectedId -and $registered.allowed_origins -notcontains "chrome-extension://$expectedId/") { $state = 'extension_origin_not_allowed' }
                elseif ($binary -and $registered.path -ne (Get-Item -LiteralPath $HostPath).FullName) { $state = 'different_host_registered' }
            } catch { $state = 'manifest_unreadable' }
        }
        @{ browser = if ($index -eq 0) { 'Edge' } else { 'Chrome' }; state = $state; manifest = $manifestFile }
    }
    @{ read_only = $true; host_state = $binaryState; host = $binary; browsers = @($checks) } | ConvertTo-Json -Depth 6
    return
}
if ($Uninstall) {
    foreach ($registration in $registrations) { if (Test-Path -LiteralPath $registration) { Remove-Item -LiteralPath $registration } }
    Write-Output 'Monica CLI browser integration unregistered. Databases are preserved.'
    return
}
$origins = @()
foreach ($extensionId in @($EdgeExtensionId, $ChromeExtensionId)) {
    if ($extensionId) {
        if ($extensionId -cnotmatch '^[a-p]{32}$') { throw 'Invalid Chromium extension ID.' }
        $origins += "chrome-extension://$extensionId/"
    }
}
if (-not $origins.Count) { throw 'Specify -EdgeExtensionId or -ChromeExtensionId from the extension settings.' }
$hostFile = Get-Item -LiteralPath $HostPath
if ($hostFile.PSIsContainer -or $hostFile.Name -ne 'monica-browser-host.exe') { throw 'HostPath must point to monica-browser-host.exe.' }
$null = Read-HostMetadata $hostFile.FullName
for ($index = 0; $index -lt 2; $index++) {
    if (($index -eq 0 -and $EdgeExtensionId) -or ($index -eq 1 -and $ChromeExtensionId)) {
        $browser = if ($index -eq 0) { 'edge' } else { 'chrome' }
        $extensionId = if ($index -eq 0) { $EdgeExtensionId } else { $ChromeExtensionId }
        $manifestPath = Join-Path $hostFile.DirectoryName "app.monica.cli.$browser.json"
        $manifest = @{ name = $hostName; description = 'Monica CLI human database management'; path = $hostFile.FullName; type = 'stdio'; allowed_origins = @("chrome-extension://$extensionId/") } | ConvertTo-Json -Depth 4
        [System.IO.File]::WriteAllText($manifestPath, $manifest, [System.Text.UTF8Encoding]::new($false))
        New-Item -Path $registrations[$index] -Force | Out-Null
        Set-Item -LiteralPath $registrations[$index] -Value $manifestPath
    }
}
Write-Output 'Monica CLI browser integration installed. Reopen CLI content management in the extension.'
