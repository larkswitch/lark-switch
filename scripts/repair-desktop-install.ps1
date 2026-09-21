[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$release = Join-Path $repo 'target\release'
$install = Join-Path $env:LOCALAPPDATA 'Lark Profile Console'
$bin = Join-Path $env:LOCALAPPDATA 'LarkProfileConsole\Lark Profile Console\data\bin'
$desktop = Join-Path $release 'lark-profile-console.exe'
$bytes = [System.IO.File]::ReadAllBytes($desktop)
$text = [System.Text.Encoding]::ASCII.GetString($bytes)
$assets = @(Get-ChildItem -LiteralPath (Join-Path $repo 'apps\desktop\dist\assets') -File | Where-Object { $_.Extension -in '.js','.css' })
if (!$assets.Count) { throw 'No frontend assets; run the Tauri build first.' }
foreach ($asset in $assets) {
    if (!$text.Contains($asset.Name)) { throw "Desktop does not embed $($asset.Name); do not deploy a cargo-only build." }
}
$entries = @()
foreach ($name in @('lark-profile-console.exe','lark-cli.exe','lpcctl.exe','larkswitch.exe')) {
    $entries += [pscustomobject]@{ Source=(Join-Path $release $name); Target=(Join-Path $install $name) }
}
foreach ($name in @('lark-cli.exe','lpcctl.exe','larkswitch.exe')) {
    $entries += [pscustomobject]@{ Source=(Join-Path $release $name); Target=(Join-Path $bin $name) }
}
foreach ($entry in $entries) {
    if (!(Test-Path -LiteralPath $entry.Source)) { throw "Missing artifact: $($entry.Source)" }
}
$suffix = '.bak-' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff')
foreach ($entry in $entries) {
    New-Item -ItemType Directory -Path (Split-Path $entry.Target -Parent) -Force | Out-Null
    if (Test-Path -LiteralPath $entry.Target) { Copy-Item -LiteralPath $entry.Target -Destination ($entry.Target + $suffix) }
}
$installedExe = Join-Path $install 'lark-profile-console.exe'
Get-Process -Name lark-profile-console -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $installedExe } | Stop-Process -Force -ErrorAction SilentlyContinue
# Stop only this application's WebView children after stopping the owner.
$profile = Join-Path $env:LOCALAPPDATA 'dev.larkswitch.desktop\EBWebView'
$profileArg = '--user-data-dir="' + $profile + '"'
Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
    Where-Object { $_.CommandLine.Contains($profileArg) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 2
foreach ($entry in $entries) {
    Copy-Item -LiteralPath $entry.Source -Destination $entry.Target -Force
    if ((Get-FileHash -LiteralPath $entry.Source).Hash -ne (Get-FileHash -LiteralPath $entry.Target).Hash) {
        throw "Installed hash mismatch: $($entry.Target)"
    }
}
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'Lark Profile Console.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $installedExe
$shortcut.WorkingDirectory = $install
$shortcut.Arguments = ''
$shortcut.IconLocation = "$installedExe,0"
$shortcut.Description = 'larkswitch — 飞书账号管理'
$shortcut.Save()
Start-Process -FilePath $installedExe -WindowStyle Normal
[pscustomobject]@{ FilesVerified=$entries.Count; BackupSuffix=$suffix; Shortcut=$shortcutPath; StartupRequested=$true }
