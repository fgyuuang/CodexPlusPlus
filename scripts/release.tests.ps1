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
    if ($Mode -like 'sherpa-*') {
        Write-Text (Join-Path $root 'Cargo.toml') 'sherpa-onnx = { version = "1.13.8", default-features = false, features = ["shared"] }'
        Write-Text (Join-Path $root 'Cargo.lock') "[[package]]`nname = `"sherpa-onnx-sys`"`nversion = `"1.13.8`"`n"
    }
    Write-Text (Join-Path $root 'mode.txt') $Mode
    foreach ($name in $names) {
        Write-Text (Join-Path $root "bin/$name") 'old artifact'
        Write-Text (Join-Path $root "runtime with spaces/$name") 'old artifact'
    }
    if ($Mode -like 'sherpa-*') {
        foreach ($name in @('sherpa-onnx-c-api.dll', 'onnxruntime.dll')) {
            Write-Text (Join-Path $root "bin/$name") 'old runtime library'
            Write-Text (Join-Path $root "runtime with spaces/$name") 'old runtime library'
        }
    }
    Write-Text (Join-Path $root 'tools/node.ps1') @'
$root = Split-Path -Parent $PSScriptRoot
[IO.File]::AppendAllText((Join-Path $root 'tool-calls.txt'), "node`n")
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
[IO.File]::AppendAllText((Join-Path $root 'tool-calls.txt'), "cargo`n")
if ($args[0] -eq 'check' -and $args -notcontains '--all-targets') { 'test targets were not checked before execution'; exit 18 }
if ($args -contains '--release') {
    [IO.File]::WriteAllText((Join-Path $root 'release-started.txt'), 'started')
    Start-Sleep -Seconds 3
    if ((Get-Content -LiteralPath (Join-Path $root 'mode.txt') -Raw) -eq 'cargo-failure') { 'simulated cargo failure'; exit 17 }
    $release = Join-Path $root 'target/release'
    [IO.Directory]::CreateDirectory($release) | Out-Null
    foreach ($name in @('codex-plus-plus-manager.exe', 'codex-plus-plus.exe', 'codex-plus-mobile-relay.exe')) {
        [IO.File]::WriteAllText((Join-Path $release $name), "new $name")
    }
    $mode = Get-Content -LiteralPath (Join-Path $root 'mode.txt') -Raw
    if ($mode -like 'sherpa-*') {
        $libraryDirectory = Join-Path $root 'target/sherpa-onnx-prebuilt/sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib/lib'
        [IO.Directory]::CreateDirectory($libraryDirectory) | Out-Null
        foreach ($name in @('sherpa-onnx-c-api.dll', 'onnxruntime.dll')) {
            [IO.File]::WriteAllText((Join-Path $libraryDirectory $name), "new $name")
            if ($mode -eq 'sherpa-missing' -and $name -eq 'onnxruntime.dll') { continue }
            [IO.File]::WriteAllText((Join-Path $release $name), "new $name")
        }
        [IO.File]::WriteAllText((Join-Path $release 'unrelated.dll'), 'must not be distributed')
        if ($mode -eq 'sherpa-mismatch') { [IO.File]::WriteAllText((Join-Path $release 'onnxruntime.dll'), 'different library') }
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
    Assert ($task.state -eq 'queued' -and (Get-Process -Id $task.pid -ErrorAction SilentlyContinue)) 'Start must return a live detached worker.'
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

$fixture = New-Fixture 'sherpa-success'
$task = Start-Fixture $fixture
Wait-Until { (Read-StatusFile $task).state -in @('succeeded', 'failed') } 'shared runtime delivery'
$result = Read-StatusFile $task
Assert ($result.state -eq 'succeeded' -and $result.artifacts.Count -eq 5 -and $result.delivery.Count -eq 10) 'Shared speech runtime must accompany the three executables.'
Assert ($result.runtimeDependencies.Count -eq 2) 'Native runtime sources must be recorded.'
foreach ($item in $result.delivery) {
    Assert ((Get-FileHash -LiteralPath $item.path).Hash -eq $item.sha256) 'Runtime and executable delivery hashes must match.'
    Assert (Test-Path -LiteralPath $item.backup -PathType Leaf) 'Old DLLs must be backed up.'
}
foreach ($directory in @('bin', 'runtime with spaces')) {
    Assert (-not (Test-Path -LiteralPath (Join-Path $fixture.root "$directory/unrelated.dll"))) 'Do not distribute arbitrary DLLs from target/release.'
}
Write-Output 'PASS shared speech runtime provenance, backup, delivery and hashes'

# 源码/target 已变化仍只能交付原成功快照；工具调用次数必须不变。
Wait-Until { -not (Get-Process -Id $task.pid -ErrorAction SilentlyContinue) } 'source worker exit'
$sourceTask = $task
$sourceStatus = Get-Content -LiteralPath (Join-Path $task.jobDirectory 'status.json') -Raw
$sourceRequest = Get-Content -LiteralPath (Join-Path $task.jobDirectory 'request.json') -Raw
$toolHash = (Get-FileHash -LiteralPath (Join-Path $fixture.root 'tool-calls.txt')).Hash
Write-Text (Join-Path $fixture.root 'crates/core/lib.rs') 'later source changes'
foreach ($name in $names) { Write-Text (Join-Path $fixture.root "target/release/$name") 'later build' }
$redeployed = (& $fixture.script -Action Redeploy -Job $task.id -DeployTo $fixture.runtime -NodePath 'missing-node' -CargoPath 'missing-cargo') -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($redeployed.state -eq 'succeeded' -and $redeployed.sourceJob -eq $task.id -and $redeployed.id -ne $task.id) 'Redeploy must create a separate successful job.'
Assert ($redeployed.artifacts.Count -eq 5 -and $redeployed.delivery.Count -eq 10) 'Redeploy must include runtime DLLs.'
foreach ($item in $redeployed.delivery) { Assert ((Get-FileHash -LiteralPath $item.path).Hash -eq $item.sha256) 'Redeployed hash mismatch.' }
Assert ((Get-FileHash -LiteralPath (Join-Path $fixture.root 'tool-calls.txt')).Hash -eq $toolHash) 'Redeploy must never invoke Node/Cargo.'
Assert ((Get-Content -LiteralPath (Join-Path $task.jobDirectory 'status.json') -Raw) -ceq $sourceStatus) 'Source evidence must remain unchanged.'
Write-Output 'PASS redeploy archived EXEs/DLLs despite changed source/target with zero tool calls'

$lockFile = [IO.File]::Open((Join-Path $fixture.root 'output/release-jobs/active.lock'), 'Open', 'ReadWrite', 'None')
try {
    $rejected = $false
    try { & $fixture.script -Action Redeploy -Job $task.id | Out-Null } catch { $rejected = $_.Exception.Message -match 'lock' }
    Assert $rejected 'Redeploy must respect the workspace lock.'
} finally { $lockFile.Dispose() }

foreach ($mode in @('failed-source','missing-step','duplicate','unsafe-name','platform','foreign-repo','damaged','missing')) {
    $status = $sourceStatus | ConvertFrom-Json -AsHashtable
    $req = $sourceRequest | ConvertFrom-Json -AsHashtable
    $file = Join-Path $task.jobDirectory 'artifacts/onnxruntime.dll'
    $bytes = [IO.File]::ReadAllBytes($file)
    switch ($mode) {
        'failed-source' { $status.state = 'failed' }
        'missing-step' { $status.steps = @($status.steps | Where-Object name -ne 'typescript') }
        'duplicate' { $status.artifacts[0].name = $status.artifacts[1].name }
        'unsafe-name' { $status.artifacts[0].name = '../escape.exe' }
        'platform' { $req.platform = 'wrong-architecture' }
        'foreign-repo' { $req.repoRoot = 'C:\another-repository' }
        'damaged' { Write-Text $file 'damaged' }
        'missing' { Move-Item -LiteralPath $file -Destination "$file.held" }
    }
    Write-Text (Join-Path $task.jobDirectory 'status.json') ($status | ConvertTo-Json -Depth 12)
    Write-Text (Join-Path $task.jobDirectory 'request.json') ($req | ConvertTo-Json -Depth 12)
    $before = @($redeployed.delivery | ForEach-Object { (Get-FileHash -LiteralPath $_.path).Hash }) -join ','
    $rejected = $false
    try { & $fixture.script -Action Redeploy -Job $task.id -DeployTo $fixture.runtime | Out-Null } catch { $rejected = $true }
    Assert $rejected "$mode must reject redeploy."
    Assert ((@($redeployed.delivery | ForEach-Object { (Get-FileHash -LiteralPath $_.path).Hash }) -join ',') -eq $before) "$mode must preserve targets."
    if ($mode -eq 'missing') { Move-Item -LiteralPath "$file.held" -Destination $file } else { [IO.File]::WriteAllBytes($file, $bytes) }
    Write-Text (Join-Path $task.jobDirectory 'status.json') $sourceStatus
    Write-Text (Join-Path $task.jobDirectory 'request.json') $sourceRequest
}
Write-Output 'PASS redeploy rejects failed/incomplete/unsafe/foreign/damaged source and respects lock'

# 可读但不允许 Replace 的第二个运行目录文件，让前面的替换成功后触发回滚。
foreach ($item in $redeployed.delivery) { Write-Text $item.path 'rollback baseline' }
$held = [IO.File]::Open((Join-Path $fixture.runtime $names[1]), 'Open', 'Read', 'Read')
try {
    $rejected = $false
    try { & $fixture.script -Action Redeploy -Job $task.id -DeployTo $fixture.runtime | Out-Null } catch { $rejected = $_.Exception.Message -match 'Redeploy failed' }
    Assert $rejected 'Locked destination must fail delivery.'
    foreach ($item in $redeployed.delivery) { Assert ((Get-Content -LiteralPath $item.path -Raw) -eq 'rollback baseline') 'Partial delivery must restore old files.' }
} finally { $held.Dispose() }
$binResult = (& $fixture.script -Action Redeploy -Job $task.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($binResult.state -eq 'succeeded' -and $binResult.delivery.Count -eq 5) 'Redeploy defaults to bin only.'
foreach ($name in $names) { Assert ((Get-Content -LiteralPath (Join-Path $fixture.runtime $name) -Raw) -eq 'rollback baseline') 'Default redeploy must preserve runtime.' }
Assert ((Get-FileHash -LiteralPath (Join-Path $fixture.root 'tool-calls.txt')).Hash -eq $toolHash) 'All redeploy paths must avoid build tools.'
Write-Output 'PASS redeploy rollback after partial replacement and default bin-only delivery'
$reused = (& $fixture.script -Action Redeploy -Job $binResult.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($reused.state -eq 'succeeded' -and $reused.sourceJob -eq $task.id) 'Captured archive must be reusable with original build evidence.'

# 旧任务没有封存与平台字段：只允许 bin 完整匹配原清单并通过 PE 架构核对。
$legacyStatus = $sourceStatus | ConvertFrom-Json -AsHashtable
$legacyRequest = $sourceRequest | ConvertFrom-Json -AsHashtable
$legacyRequest.Remove('platform')
foreach ($artifact in $legacyStatus.artifacts) {
    if ($artifact.name -like '*.exe') {
        $pe = [byte[]]::new(128)
        [BitConverter]::GetBytes([uint16]0x5A4D).CopyTo($pe, 0)
        [BitConverter]::GetBytes([uint32]64).CopyTo($pe, 60)
        [BitConverter]::GetBytes([uint32]0x4550).CopyTo($pe, 64)
        $machine = if ([Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -eq 'Arm64') { 0xAA64 } else { 0x8664 }
        [BitConverter]::GetBytes([uint16]$machine).CopyTo($pe, 68)
        $file = Join-Path $fixture.root "bin/$($artifact.name)"
        [IO.File]::WriteAllBytes($file, $pe)
        $artifact.sha256 = (Get-FileHash -LiteralPath $file).Hash
        $artifact.length = $pe.Length
    }
}
Write-Text (Join-Path $task.jobDirectory 'status.json') ($legacyStatus | ConvertTo-Json -Depth 12)
Write-Text (Join-Path $task.jobDirectory 'request.json') ($legacyRequest | ConvertTo-Json -Depth 12)
$archive = [IO.Path]::GetFullPath((Join-Path $task.jobDirectory 'artifacts'))
Assert ($archive.StartsWith([IO.Path]::GetFullPath($testRoot) + '\')) 'Archive move must remain inside test workspace.'
Move-Item -LiteralPath $archive -Destination "$archive.held"
$legacyResult = (& $fixture.script -Action Redeploy -Job $task.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($legacyResult.state -eq 'succeeded') 'Matching legacy bin must be captured and delivered.'
Write-Text (Join-Path $fixture.root 'bin/onnxruntime.dll') 'newer unrelated runtime'
$legacyReused = (& $fixture.script -Action Redeploy -Job $legacyResult.id) -join "`n" | ConvertFrom-Json -AsHashtable
Assert ($legacyReused.state -eq 'succeeded') 'Legacy capture must remain usable after bin changes.'
$captured = [IO.Path]::GetFullPath((Join-Path $fixture.root "output/release-jobs/$($legacyResult.id)/artifacts"))
Assert ($captured.StartsWith([IO.Path]::GetFullPath($testRoot) + '\')) 'Captured archive must remain inside test workspace.'
Move-Item -LiteralPath $captured -Destination "$captured.held"
$rejected = $false
try { & $fixture.script -Action Redeploy -Job $legacyResult.id | Out-Null } catch { $rejected = $_.Exception.Message -match 'Source archive is missing' }
Assert $rejected 'A missing captured archive must not fall back to legacy bin.'
Move-Item -LiteralPath "$captured.held" -Destination $captured
Write-Text (Join-Path $fixture.root 'bin/onnxruntime.dll') 'newer unrelated runtime'
$rejected = $false
try { & $fixture.script -Action Redeploy -Job $task.id | Out-Null } catch { $rejected = $_.Exception.Message -match 'Source artifact mismatch' }
Assert $rejected 'Legacy mismatch must never fall back to target/release.'
Assert ((Get-FileHash -LiteralPath (Join-Path $fixture.root 'tool-calls.txt')).Hash -eq $toolHash) 'Legacy capture must not invoke build tools.'
Write-Output 'PASS legacy capture checks PE architecture and full manifest without target fallback'

foreach ($mode in @('sherpa-missing', 'sherpa-mismatch')) {
    $fixture = New-Fixture $mode
    $task = Start-Fixture $fixture
    Wait-Until { (Read-StatusFile $task).state -in @('succeeded', 'failed') } $mode
    $result = Read-StatusFile $task
    Assert ($result.state -eq 'failed' -and $result.error -match 'runtime DLLs are missing or do not match') "$mode must stop delivery."
    Assert-OldArtifacts $fixture
    foreach ($directory in @('bin', 'runtime with spaces')) {
        foreach ($name in @('sherpa-onnx-c-api.dll', 'onnxruntime.dll')) {
            Assert ((Get-Content -LiteralPath (Join-Path $fixture.root "$directory/$name") -Raw) -eq 'old runtime library') 'Failed runtime validation must preserve old DLLs.'
        }
    }
    Write-Output "PASS $mode fails before executable or DLL delivery"
}

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
