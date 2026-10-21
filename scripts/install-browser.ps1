#Requires -Version 5.1
<#! Install the CLI Native Messaging bridge for the current Windows user. #>
[CmdletBinding()]
param(
    [string]$HostPath = (Join-Path $PSScriptRoot '..\monica-browser-host.exe'),
    [string]$EdgeExtensionId,
    [string]$ChromeExtensionId,
    [string]$FirefoxExtensionId,
    [switch]$Uninstall,
    [switch]$Diagnose
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$hostName = 'app.monica.cli'
$firefoxId = 'monica@monica-pass.github.io'
$browsers = @(
    @{ name = 'Edge'; file = 'edge'; id = $EdgeExtensionId; key = 'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\app.monica.cli' },
    @{ name = 'Chrome'; file = 'chrome'; id = $ChromeExtensionId; key = 'HKCU:\Software\Google\Chrome\NativeMessagingHosts\app.monica.cli' },
    @{ name = 'Firefox'; file = 'firefox'; id = $FirefoxExtensionId; key = 'HKCU:\Software\Mozilla\NativeMessagingHosts\app.monica.cli' }
)
foreach ($browser in $browsers) {
    if (-not $browser.id) { continue }
    if ($browser.name -eq 'Firefox') {
        if ($browser.id -cne $firefoxId) { throw 'Firefox requires the exact Monica add-on ID: monica@monica-pass.github.io.' }
    } elseif ($browser.id -cnotmatch '^[a-p]{32}$') { throw 'Invalid Chromium extension ID.' }
}
function Supports-Firefox($Info) {
    return ($null -ne $Info -and $Info.PSObject.Properties.Name -contains 'firefox_extension_id' -and $Info.firefox_extension_id -ceq $firefoxId)
}
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
        if ($FirefoxExtensionId -and -not (Supports-Firefox $info)) { throw 'Update the browser helper for Firefox support.' }
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
    $checks = foreach ($browser in $browsers) {
        $state = 'not_registered'; $manifestFile = $null
        if (Test-Path -LiteralPath $browser.key) {
            try {
                $key = Get-Item -LiteralPath $browser.key
                $manifestFile = $key.GetValue('', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                $registered = [System.IO.File]::ReadAllText($manifestFile) | ConvertFrom-Json
                $state = 'registered'
                if ($registered.name -ne $hostName -or $registered.type -ne 'stdio') { $state = 'invalid_manifest' }
                elseif (-not (Test-Path -LiteralPath $registered.path -PathType Leaf)) { $state = 'registered_host_missing' }
                elseif ($browser.name -eq 'Firefox') {
                    if ($registered.PSObject.Properties.Name -contains 'allowed_origins' -or $registered.PSObject.Properties.Name -notcontains 'allowed_extensions' -or @($registered.allowed_extensions).Count -ne 1 -or $registered.allowed_extensions[0] -cne $firefoxId) { $state = 'extension_id_not_allowed' }
                    elseif ($binary -and -not (Supports-Firefox $binary)) { $state = 'host_does_not_support_firefox' }
                }
                elseif ($browser.id -and $registered.allowed_origins -notcontains "chrome-extension://$($browser.id)/") { $state = 'extension_origin_not_allowed' }
                if ($state -eq 'registered' -and $binary -and $registered.path -ne (Get-Item -LiteralPath $HostPath).FullName) { $state = 'different_host_registered' }
            } catch { $state = 'manifest_unreadable' }
        }
        @{ browser = $browser.name; state = $state; manifest = $manifestFile }
    }
    @{ read_only = $true; host_state = $binaryState; host = $binary; browsers = @($checks) } | ConvertTo-Json -Depth 6
    return
}
$selected = @($browsers | Where-Object { $_.id })
if ($Uninstall) {
    # No IDs retains the original all-browser uninstall. IDs scope removal.
    if (-not $selected.Count) { $selected = $browsers }
    foreach ($browser in $selected) { if (Test-Path -LiteralPath $browser.key) { Remove-Item -LiteralPath $browser.key } }
    Write-Output 'Selected Monica CLI browser registrations removed. Databases are preserved.'
    return
}
if (-not $selected.Count) { throw 'Specify -EdgeExtensionId, -ChromeExtensionId or -FirefoxExtensionId from the extension settings.' }
$hostFile = Get-Item -LiteralPath $HostPath
if ($hostFile.PSIsContainer -or $hostFile.Name -ne 'monica-browser-host.exe') { throw 'HostPath must point to monica-browser-host.exe.' }
$null = Read-HostMetadata $hostFile.FullName
foreach ($browser in $selected) {
    $manifestPath = Join-Path $hostFile.DirectoryName "app.monica.cli.$($browser.file).json"
    $manifest = @{ name = $hostName; description = 'Monica CLI human database management'; path = $hostFile.FullName; type = 'stdio' }
    if ($browser.name -eq 'Firefox') { $manifest.allowed_extensions = @($firefoxId) }
    else { $manifest.allowed_origins = @("chrome-extension://$($browser.id)/") }
    [System.IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 4), [System.Text.UTF8Encoding]::new($false))
    New-Item -Path $browser.key -Force | Out-Null
    Set-Item -LiteralPath $browser.key -Value $manifestPath
}
Write-Output 'Monica CLI browser integration installed. Reopen CLI content management in the extension.'
