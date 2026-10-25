#Requires -Version 7.0
# 只使用隔离目录中的假 Node/Cargo；不编译产品、不触碰真实安装目录。
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$testRoot = Join-Path (Split-Path -Parent $PSScriptRoot) ('output/release-workflow-tests/' + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($testRoot) | Out-Null
$names = @('codex-plus-plus-manager.exe', 'codex-plus-plus.exe', 'codex-plus-mobile-relay.exe')
function Assert($Condition, $Message) { if (-not $Condition) { throw $Message } }
function Write-Text($Path, $Text) {
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($Path)) | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function Read-StatusFile($Task) { Get-Content -LiteralPath (Join-Path $Task.jobDirectory 'status.json') -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable }
function Wait-Until($Condition, $Message) {
    $deadline = (Get-Date).AddSeconds(25)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "Timed out: $Message" }
        Start-Sleep -Milliseconds 100
    }
}
function New-Fixture($Mode) {
    $root = Join-Path $testRoot "$Mode repo with spaces"
    foreach ($directory in @('scripts', 'crates/core', 'apps/codex-plus-manager/src', 'tools', 'bin', 'runtime with spaces')) {
        [IO.Directory]::CreateDirectory((Join-Path $root $directory)) | Out-Null
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'release.ps1') -Destination (Join-Path $root 'scripts/release.ps1')
    foreach ($path in @('Cargo.toml', 'Cargo.lock', 'crates/core/lib.rs', 'scripts/assemble-renderer-inject.mjs', 'apps/codex-plus-manager/src/a.test.ts')) {
        Write-Text (Join-Path $root $path) 'fixture source'
    }
    Write-Text (Join-Path $root 'mode.txt') $Mode
    foreach ($name in $names) {
        Write-Text (Join-Path $root "bin/$name") 'old artifact'
        Write-Text (Join-Path $root "runtime with spaces/$name") 'old artifact'
    }
    Write-Text (Join-Path $root 'tools/node.ps1') @'
$root = Split-Path -Parent $PSScriptRoot
if ($args -contains '-v') { 'v24.0.0'; exit 0 }
if ($args[0] -like '*vite.js') {
    $dist = Join-Path $root 'apps/codex-plus-manager/dist'
    [IO.Directory]::CreateDirectory((Join-Path $dist 'assets')) | Out-Null
    [IO.File]::WriteAllText((Join-Path $dist 'index.html'), '<html>fixture</html>')
    $text = if ((Get-Content -LiteralPath (Join-Path $root 'mode.txt') -Raw) -eq 'ui-missing') { 'wrong text' } else { 'workflow-ui-proof' }
    [IO.File]::WriteAllText((Join-Path $dist 'assets/index-fixture.js'), $text)
}
exit 0
'@
    Write-Text (Join-Path $root 'tools/cargo.ps1') @'
$root = Split-Path -Parent $PSScriptRoot
if ($args -contains '--release') {
    [IO.File]::WriteAllText((Join-Path $root 'release-started.txt'), 'started')
    Start-Sleep -Seconds 3
    if ((Get-Content -LiteralPath (Join-Path $root 'mode.txt') -Raw) -eq 'cargo-failure') { 'simulated cargo failure'; exit 17 }
    $release = Join-Path $root 'target/release'
    [IO.Directory]::CreateDirectory($release) | Out-Null
    foreach ($name in @('codex-plus-plus-manager.exe', 'codex-plus-plus.exe', 'codex-plus-mobile-relay.exe')) {
        [IO.File]::WriteAllText((Join-Path $release $name), "new $name")
    }
    if ((Get-Content -LiteralPath (Join-Path $root 'mode.txt') -Raw) -eq 'stale-manager') {
        [IO.File]::SetLastWriteTimeUtc((Join-Path $release 'codex-plus-plus-manager.exe'), [datetime]::UtcNow.AddDays(-1))
    }
}
exit 0
'@
    @{ root=$root; script=(Join-Path $root 'scripts/release.ps1'); runtime=(Join-Path $root 'runtime with spaces') }
}
function Start-Fixture($Fixture, [switch]$BinOnly) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $options = @{ Action='Start'; NodePath=(Join-Path $Fixture.root 'tools/node.ps1'); CargoPath=(Join-Path $Fixture.root 'tools/cargo.ps1'); ExpectedUiText='workflow-ui-proof' }
    if (-not $BinOnly) { $options.DeployTo = $Fixture.runtime }
    $output = & $Fixture.script @options
    $task = ($output -join "`n") | ConvertFrom-Json -AsHashtable
    Assert ($watch.Elapsed.TotalSeconds -lt 3) 'Start must return without waiting for the 3-second fake release.'
    $task
}
function Assert-OldArtifacts($Fixture) {
    foreach ($directory in @('bin', 'runtime with spaces')) {
        foreach ($name in $names) { Assert ((Get-Content -LiteralPath (Join-Path $Fixture.root "$directory/$name") -Raw) -eq 'old artifact') 'Failure must not deliver artifacts.' }
    }
}

$fixture = New-Fixture 'success'
$task = Start-Fixture $fixture
$duplicateRejected = $false
try { Start-Fixture $fixture | Out-Null } catch { $duplicateRejected = $_.Exception.Message -match 'lock|queued/running' }
Assert $duplicateRejected 'A duplicate release must be rejected.'
$live = (& $fixture.script -Action Status -Job $task.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($live.state -in @('queued', 'running') -and $live.isAlive) 'Status must report the live detached worker.'
Wait-Until { (Read-StatusFile $task).state -in @('succeeded', 'failed') } 'successful release'
$result = Read-StatusFile $task
Assert ($result.state -eq 'succeeded') ("Fake release failed: " + $result.error)
foreach ($item in $result.delivery) {
    Assert ((Get-FileHash -LiteralPath $item.path).Hash -eq $item.sha256) 'All delivered hashes must match.'
    Assert ((Get-Content -LiteralPath $item.backup -Raw) -eq 'old artifact') 'Old artifacts must be backed up.'
}
Assert (-not $result.restartPerformed) 'The worker must not restart applications.'
$replayRejected = $false
try { & $fixture.script -Action Worker -Job $task.jobDirectory } catch { $replayRejected = $_.Exception.Message -match 'Only a new queued job' }
Assert $replayRejected 'Completed jobs must not replay and overwrite their original backups.'
Write-Output 'PASS detached return, spaces, status, duplicate lock, checks/build/delivery, backups and hashes'

$fixture = New-Fixture 'bin-only'
$task = Start-Fixture $fixture -BinOnly
Wait-Until { (Read-StatusFile $task).state -in @('succeeded', 'failed') } 'bin-only release'
$result = Read-StatusFile $task
Assert ($result.state -eq 'succeeded' -and $result.delivery.Count -eq 3) 'Default delivery must only publish three bin artifacts.'
foreach ($name in $names) { Assert ((Get-Content -LiteralPath (Join-Path $fixture.runtime $name) -Raw) -eq 'old artifact') 'Default must not change runtime files.' }
Write-Output 'PASS default bin-only delivery preserves runtime'

foreach ($mode in @('cargo-failure', 'source-change', 'dist-change', 'stale-manager', 'ui-missing')) {
    $fixture = New-Fixture $mode
    $task = Start-Fixture $fixture
    if ($mode -in @('source-change', 'dist-change')) {
        Wait-Until { Test-Path -LiteralPath (Join-Path $fixture.root 'release-started.txt') } 'fake release start'
        $changedPath = if ($mode -eq 'source-change') { 'crates/core/added.rs' } else { 'apps/codex-plus-manager/dist/assets/index-fixture.js' }
        Write-Text (Join-Path $fixture.root $changedPath) 'changed during build'
    }
    Wait-Until { (Read-StatusFile $task).state -in @('succeeded', 'failed') } $mode
    $result = Read-StatusFile $task
    Assert ($result.state -eq 'failed') "$mode must fail closed."
    $expectedError = switch ($mode) { 'cargo-failure' { 'exit code 17' } 'source-change' { 'Build inputs changed' } 'dist-change' { 'Frontend outputs changed' } 'stale-manager' { 'Manager predates' } 'ui-missing' { 'Expected UI text' } }
    Assert ($result.error -match $expectedError) "$mode must preserve the specific failure."
    Assert-OldArtifacts $fixture
    if ($mode -eq 'ui-missing') { Assert (-not (Test-Path -LiteralPath (Join-Path $fixture.root 'release-started.txt'))) 'Missing UI must stop before Rust release.' }
    Write-Output "PASS $mode stops delivery"
}
$result.state = 'running'
$statusPath = Join-Path $task.jobDirectory 'status.json'
[IO.File]::WriteAllText($statusPath, ($result | ConvertTo-Json -Depth 12))
$stale = (& $fixture.script -Action Status -Job $task.id) -join "`n" | ConvertFrom-Json -AsHashtable
# 最后一个 worker 可能刚写完状态；等待其退出后再验证 PID/启动时间识别。
Wait-Until { -not (Get-Process -Id $task.pid -ErrorAction SilentlyContinue) } 'fixture worker exit'
$stale = (& $fixture.script -Action Status -Job $task.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($stale.state -eq 'interrupted') 'A stale running state must not be called successful.'
Write-Output "PASS stale worker detection; retained fixtures: $testRoot"
