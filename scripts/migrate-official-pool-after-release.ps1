#Requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Job)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($Job -notmatch '^\d{8}-\d{6}-[a-f0-9]{8}$') { throw 'Invalid release job ID' }
$repo = Split-Path -Parent $PSScriptRoot
$directory = Join-Path $repo "output/release-jobs/$Job"
$statusPath = Join-Path $directory 'status.json'
$receipt = Join-Path $directory 'official-pool-migration.json'
if (Test-Path -LiteralPath $receipt) { throw 'Migration receipt already exists; inspect before retrying' }
$result = [ordered]@{ state='waiting_for_release'; job=$Job; agentsChanged=$false; loginCopied=$false }
function Save-Receipt {
    [IO.File]::WriteAllText($receipt, ($result | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
}
Save-Receipt
try {
    $deadline = (Get-Date).AddMinutes(90)
    while ($true) {
        $release = [IO.File]::ReadAllText($statusPath) | ConvertFrom-Json
        if ($release.state -eq 'succeeded') { break }
        if ($release.state -in @('failed','interrupted')) { throw "Release is $($release.state); no migration performed" }
        if ((Get-Date) -gt $deadline) { throw 'Release wait timed out; no migration performed' }
        Start-Sleep -Seconds 15
    }
    $exe = Join-Path $repo 'bin/codex-plus-plus-manager.exe'
    $expected = @($release.artifacts | Where-Object name -eq 'codex-plus-plus-manager.exe')
    if ($expected.Count -ne 1 -or (Get-FileHash -LiteralPath $exe).Hash -ne $expected[0].sha256) { throw 'Release binary hash mismatch' }
    $state = if ($env:CODEX_PLUS_STATE_DIR) { $env:CODEX_PLUS_STATE_DIR } else { Join-Path $env:USERPROFILE '.codex-session-delete' }
    $protected = @((Join-Path $env:USERPROFILE '.codex/auth.json'), (Join-Path $env:USERPROFILE '.codex/config.toml'), (Join-Path $state 'official-accounts.json'), (Join-Path $state 'official-account-secrets.json'))
    $before = @{}; foreach ($p in $protected) { $before[$p] = if (Test-Path -LiteralPath $p) { (Get-FileHash -LiteralPath $p).Hash } else { 'absent' } }
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.ArgumentList.Add('--codex-plus-migrate-official-pool')
    $process = [Diagnostics.Process]::Start($start)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(60000)) { $process.Kill(); throw 'Owned migration command timed out; inspect state before retrying' }
    $output = $stdout.GetAwaiter().GetResult()
    $errorText = $stderr.GetAwaiter().GetResult()
    if ($process.ExitCode -ne 0) { throw "Migration command failed: $errorText" }
    $commandResult = $output | ConvertFrom-Json
    if ($commandResult.state -ne 'succeeded') { throw 'No successful migration result' }
    $unchanged = $true
    foreach ($p in $protected) {
        $after = if (Test-Path -LiteralPath $p) { (Get-FileHash -LiteralPath $p).Hash } else { 'absent' }
        if ($after -ne $before[$p]) { $unchanged = $false }
    }
    $result.state = 'succeeded'
    $result['added'] = $commandResult.added
    $result['protectedFilesUnchanged'] = $unchanged
    $result['runtimeAcceptance'] = 'pending'
} catch {
    $result.state = 'failed'
    $result['error'] = $_.Exception.Message
} finally {
    $result['completedAt'] = (Get-Date).ToString('o')
    Save-Receipt
}
