[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $Arguments
)

$ErrorActionPreference = 'Stop'

$resolved = @(
    (Get-Command larkswitch.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source),
    (Get-Command lpcctl.exe -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Source)
) | Where-Object { $_ }

$candidates = @(
    $resolved +
    @(
        (Join-Path $env:LOCALAPPDATA 'Lark Profile Console\larkswitch.exe'),
        (Join-Path $env:LOCALAPPDATA 'Lark Profile Console\lpcctl.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Lark Profile Console\larkswitch.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Lark Profile Console\lpcctl.exe')
    )
) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) } |
    Select-Object -Unique

$executable = $candidates | Select-Object -First 1
if (-not $executable) {
    throw 'larkswitch/lpcctl was not found. Run `larkswitch setup` first.'
}

& $executable @Arguments
exit $LASTEXITCODE
