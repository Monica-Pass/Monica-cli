#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$HostPath,
    [string]$TemporaryRoot = [System.IO.Path]::GetTempPath()
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
function Assert($Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
$installer = Join-Path $PSScriptRoot 'install-browser.ps1'
$source = (Get-Item -LiteralPath $HostPath).FullName
$identity = [guid]::NewGuid().ToString('N')
$temporaryParent = [System.IO.Path]::GetFullPath($TemporaryRoot).TrimEnd('\', '/')
$temporary = Join-Path $temporaryParent "monica-browser-installer-$identity"
$registryParent = 'Registry::HKEY_CURRENT_USER\Software\MonicaCliInstallerTests'
$registryRoot = "$registryParent\$identity"
$edge = 'HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\app.monica.cli'
$chrome = 'HKCU:\Software\Google\Chrome\NativeMessagingHosts\app.monica.cli'
$firefox = 'HKCU:\Software\Mozilla\NativeMessagingHosts\app.monica.cli'
$firefoxId = 'monica@monica-pass.github.io'
$edgeId = 'a' * 32; $chromeId = 'b' * 32
New-Item -ItemType Directory -Path $temporary | Out-Null
New-Item -Path $registryRoot -Force | Out-Null
try {
    # This process sees a private registry subtree as HKCU. Real browser keys
    # are never read, overwritten or removed, even while other tests run.
    Remove-PSDrive -Name HKCU
    New-PSDrive -Name HKCU -PSProvider Registry -Root "HKEY_CURRENT_USER\Software\MonicaCliInstallerTests\$identity" | Out-Null
    $binary = Join-Path $temporary 'monica-browser-host.exe'
    Copy-Item -LiteralPath $source -Destination $binary
    [System.IO.File]::WriteAllBytes((Join-Path $temporary 'monica-pass.portable'), [byte[]]@())
    $before = (& $installer -HostPath $binary -Diagnose) | ConvertFrom-Json
    Assert ($before.host_state -eq 'compatible' -and $before.host.state -eq 'setup_required') 'Private portable helper diagnostic failed'
    Assert (@($before.browsers | Where-Object { $_.state -ne 'not_registered' }).Count -eq 0) 'Unexpected test registrations'
    foreach ($badId in @('other@example.com', 'Monica@monica-pass.github.io', 'monica@monica-pass.github.io/')) {
        $rejected = $false
        try { & $installer -HostPath $binary -FirefoxExtensionId $badId | Out-Null } catch { $rejected = $true }
        Assert $rejected 'Invalid Firefox add-on ID accepted'
        Assert (-not (Test-Path -LiteralPath $firefox)) 'Rejected ID wrote a Firefox registration'
    }
    & $installer -HostPath $binary -EdgeExtensionId $edgeId -ChromeExtensionId $chromeId | Out-Null
    $edgeManifest = (Get-Item -LiteralPath $edge).GetValue('')
    $chromeManifest = (Get-Item -LiteralPath $chrome).GetValue('')
    $edgeBytes = [System.IO.File]::ReadAllText($edgeManifest)
    $chromeBytes = [System.IO.File]::ReadAllText($chromeManifest)
    & $installer -HostPath $binary -FirefoxExtensionId $firefoxId | Out-Null
    $firefoxManifest = (Get-Item -LiteralPath $firefox).GetValue('')
    Assert ($firefoxManifest -ne $edgeManifest -and $firefoxManifest -ne $chromeManifest) 'Browser manifests share a file'
    $manifest = [System.IO.File]::ReadAllText($firefoxManifest) | ConvertFrom-Json
    Assert (@($manifest.allowed_extensions).Count -eq 1 -and $manifest.allowed_extensions[0] -ceq $firefoxId) 'Firefox allowlist is not exact'
    Assert ($manifest.PSObject.Properties.Name -notcontains 'allowed_origins') 'Firefox manifest contains Chromium origins'
    Assert ([System.IO.File]::ReadAllText($edgeManifest) -ceq $edgeBytes) 'Firefox install changed Edge'
    Assert ([System.IO.File]::ReadAllText($chromeManifest) -ceq $chromeBytes) 'Firefox install changed Chrome'
    $diagnosis = (& $installer -HostPath $binary -FirefoxExtensionId $firefoxId -Diagnose) | ConvertFrom-Json
    Assert ($diagnosis.read_only -and @($diagnosis.browsers | Where-Object { $_.state -ne 'registered' }).Count -eq 0) 'Installed browser diagnostic failed'
    $manifest.allowed_extensions = @('other@example.com')
    $manifest | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $firefoxManifest -Encoding UTF8
    $diagnosis = (& $installer -HostPath $binary -Diagnose) | ConvertFrom-Json
    Assert (($diagnosis.browsers | Where-Object { $_.browser -eq 'Firefox' }).state -eq 'extension_id_not_allowed') 'Bad manifest was not diagnosed'
    & $installer -HostPath $binary -FirefoxExtensionId $firefoxId | Out-Null
    $firefoxBytes = [System.IO.File]::ReadAllText($firefoxManifest)
    & $installer -HostPath $binary -EdgeExtensionId ('c' * 32) | Out-Null
    Assert ([System.IO.File]::ReadAllText($firefoxManifest) -ceq $firefoxBytes) 'Edge update changed Firefox'
    Assert ([System.IO.File]::ReadAllText($chromeManifest) -ceq $chromeBytes) 'Edge update changed Chrome'
    & $installer -FirefoxExtensionId $firefoxId -Uninstall | Out-Null
    Assert (-not (Test-Path -LiteralPath $firefox)) 'Scoped Firefox uninstall failed'
    Assert ((Test-Path -LiteralPath $edge) -and (Test-Path -LiteralPath $chrome)) 'Firefox uninstall removed Chromium registrations'
    & $installer -Uninstall | Out-Null
    Assert (-not ((Test-Path -LiteralPath $edge) -or (Test-Path -LiteralPath $chrome) -or (Test-Path -LiteralPath $firefox))) 'All-browser uninstall failed'
    Write-Output 'Isolated Edge/Chrome/Firefox registration, diagnostics, ID rejection and scoped uninstall passed.'
} finally {
    Remove-PSDrive -Name HKCU -ErrorAction SilentlyContinue
    New-PSDrive -Name HKCU -PSProvider Registry -Root HKEY_CURRENT_USER | Out-Null
    Assert ($registryRoot -ceq "$registryParent\$identity") 'Unexpected registry cleanup target'
    if (Test-Path -LiteralPath $registryRoot) { Remove-Item -LiteralPath $registryRoot -Recurse -Force }
    $resolved = [System.IO.Path]::GetFullPath($temporary)
    Assert ($resolved -ceq (Join-Path $temporaryParent "monica-browser-installer-$identity")) 'Unexpected file cleanup target'
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
