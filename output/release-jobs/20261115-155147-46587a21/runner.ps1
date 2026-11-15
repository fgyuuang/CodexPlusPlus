#Requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Start', 'Status', 'Worker', 'Redeploy')][string]$Action = 'Status',
    [string]$Job,
    [string]$ExpectedUiText = '',
    [string]$DeployTo = '',
    [string]$NodePath = '',
    [string]$CargoPath = '',
    [ValidateRange(1, 16)][int]$Jobs = 2
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows) { throw 'This release workflow requires Windows and PowerShell 7.' }
$repoRoot = Split-Path -Parent $PSScriptRoot
$names = @('codex-plus-plus-manager.exe', 'codex-plus-plus.exe', 'codex-plus-mobile-relay.exe')
$script:distSnapshot = @()

function Write-Json($Path, $Value) {
    $temporary = "$Path.$PID.tmp"
    [IO.File]::WriteAllText($temporary, ($Value | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false))
    for ($attempt = 0; $attempt -lt 25; $attempt++) {
        try { [IO.File]::Move($temporary, $Path, $true); return }
        catch [IO.IOException], [UnauthorizedAccessException] {
            if ($attempt -eq 24) { throw }
            Start-Sleep -Milliseconds 40
        }
    }
}
function Read-Json($Path) {
    $stream = [IO.File]::Open($Path, 'Open', 'Read', ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $reader = [IO.StreamReader]::new($stream, [Text.Encoding]::UTF8)
    try { $reader.ReadToEnd() | ConvertFrom-Json -AsHashtable } finally { $reader.Dispose() }
}
function Assert-PlainPath($Path) {
    $cursor = [IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if ((Test-Path -LiteralPath $cursor) -and ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Refusing a reparse point: $cursor"
        }
        $cursor = [IO.Path]::GetDirectoryName($cursor)
    }
}
function Get-Snapshot([switch]$Dist) {
    Push-Location -LiteralPath $repoRoot
    try {
        if ($Dist) {
            $paths = @(Get-ChildItem -LiteralPath 'apps/codex-plus-manager/dist' -Recurse -File | ForEach-Object FullName)
        } else {
            $roots = @('crates', 'apps', 'assets', 'scripts', 'tools/rice-maid-pet', 'local-plugins', 'docs/images', '.cargo') | Where-Object { Test-Path -LiteralPath $_ }
            $paths = @(& $script:rgPath --files --hidden --no-ignore @roots -g '!**/node_modules/**' -g '!**/target/**' -g '!**/dist/**' -g '!**/gen/**' -g '!**/.git/**' -g '!**/.env')
            if ($LASTEXITCODE -gt 1) { throw 'Unable to enumerate build inputs.' }
            $paths += @('Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml', 'rustfmt.toml', '.rustfmt.toml') | Where-Object { Test-Path -LiteralPath $_ }
        }
        @($paths | Sort-Object -Unique | ForEach-Object {
            $absolute = if ([IO.Path]::IsPathRooted($_)) { [IO.Path]::GetFullPath($_) } else { [IO.Path]::GetFullPath((Join-Path $repoRoot $_)) }
            [IO.Path]::GetRelativePath($repoRoot, $absolute) + "`t" + (Get-FileHash -LiteralPath $absolute -Algorithm SHA256).Hash
        })
    } finally { Pop-Location }
}
function Assert-Inputs {
    if ((@(Get-Snapshot) -join "`n") -cne (@($request.inputs) -join "`n")) {
        throw 'Build inputs changed (including added/deleted files); refusing delivery. Review changes and start a new job.'
    }
    if ($script:distSnapshot.Count -and (@(Get-Snapshot -Dist) -join "`n") -cne ($script:distSnapshot -join "`n")) {
        throw 'Frontend outputs changed during build; refusing delivery.'
    }
}
function Read-Status($Directory) {
    $value = Read-Json (Join-Path $Directory 'status.json')
    $value['jobDirectory'] = $Directory
    $value['isAlive'] = $false
    $processFile = Join-Path $Directory 'process.json'
    if (Test-Path -LiteralPath $processFile) {
        $identity = Read-Json $processFile
        try {
            $instance = Get-Process -Id $identity.pid -ErrorAction Stop
            $value.isAlive = $instance.StartTime.ToUniversalTime().Ticks -eq ([datetime]$identity.startedAt).ToUniversalTime().Ticks
        } catch { $value.isAlive = $false }
    }
    if ($value.state -in @('queued', 'running') -and -not $value.isAlive) {
        if ((Test-Path -LiteralPath $processFile) -or ((Get-Date).ToUniversalTime() - [datetime]$value.createdAt).TotalSeconds -gt 15) {
            $value.state = 'interrupted'
            $value.error = 'Worker is no longer running. Inspect logs; this job did not complete delivery.'
        }
    }
    $value
}
function Save-State {
    $script:state.updatedAt = (Get-Date).ToUniversalTime().ToString('o')
    Write-Json (Join-Path $jobDirectory 'status.json') $script:state
}
function Invoke-Step($Name, $Program, [string[]]$Arguments, $WorkingDirectory = $repoRoot) {
    $script:state.stage = $Name
    $log = Join-Path $jobDirectory "$Name.log"
    $script:state.currentLog = $log
    Save-State
    $started = (Get-Date).ToUniversalTime()
    Push-Location -LiteralPath $WorkingDirectory
    try { $global:LASTEXITCODE = 0; & $Program @Arguments *> $log; $code = $LASTEXITCODE }
    finally { Pop-Location }
    $script:state.steps += @{ name=$Name; exitCode=$code; startedAt=$started.ToString('o'); endedAt=(Get-Date).ToUniversalTime().ToString('o'); log=$log }
    Save-State
    if ($code -ne 0) { throw "Step '$Name' failed with exit code $code. See $log" }
}
function Assert-NoCompiler {
    $active = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('cargo.exe', 'rustc.exe', 'link.exe', 'tauri.exe') })
    if ($active.Count) { throw 'Another compiler is still active. Refusing a concurrent build/delivery; inspect processes first.' }
}
function Get-RuntimeDependencySnapshot {
    $manifest = [IO.File]::ReadAllText((Join-Path $repoRoot 'Cargo.toml'), [Text.Encoding]::UTF8)
    # 只处理当前显式启用的共享语音库；绝不把 target/release 中任意 DLL 打包。
    if ($manifest -notmatch '(?m)^sherpa-onnx\s*=\s*\{[^\r\n]*features\s*=\s*\[[^\r\n]*"shared"') { return @() }
    $lockText = [IO.File]::ReadAllText((Join-Path $repoRoot 'Cargo.lock'), [Text.Encoding]::UTF8)
    if ($lockText -notmatch '(?ms)^\[\[package\]\]\s*name\s*=\s*"sherpa-onnx-sys"\s*version\s*=\s*"([^"\r\n]+)"') {
        throw 'Shared sherpa-onnx is enabled but its locked native-library version cannot be determined.'
    }
    $version = $Matches[1]
    if ($version -notmatch '^\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$') { throw 'Invalid sherpa-onnx native-library version.' }
    $releaseDirectory = Join-Path $repoRoot 'target/release'
    $cacheDirectory = Join-Path $repoRoot 'target/sherpa-onnx-prebuilt'
    $candidates = if ($env:SHERPA_ONNX_LIB_DIR) {
        @([IO.Path]::GetFullPath($env:SHERPA_ONNX_LIB_DIR))
    } elseif (Test-Path -LiteralPath $cacheDirectory -PathType Container) {
        @(Get-ChildItem -LiteralPath $cacheDirectory -Directory -Filter "sherpa-onnx-v$version-win-*-shared-MT-Release-lib" |
            Sort-Object Name | ForEach-Object { Join-Path $_.FullName 'lib' })
    } else { @() }
    $required = @('sherpa-onnx-c-api.dll', 'onnxruntime.dll')
    foreach ($directory in $candidates) {
        Assert-PlainPath $directory
        if (-not (Test-Path -LiteralPath $directory -PathType Container)) { continue }
        $libraries = @(Get-ChildItem -LiteralPath $directory -File -Filter '*.dll' | Sort-Object Name)
        if (-not $libraries.Count) { continue }
        if (@($required | Where-Object { $_ -notin $libraries.Name }).Count) { continue }
        $snapshot = @()
        $matchesRelease = $true
        foreach ($library in $libraries) {
            if ($library.Name -notmatch '^[A-Za-z0-9_.-]+\.dll$' -or $library.Name -match '^(?:api-ms-|ext-ms-|kernel32\.|kernelbase\.|ntdll\.|ucrtbase\.|vcruntime\d|msvcp\d|msvcr\d)') {
                throw "Refusing a Windows/system runtime DLL in the sherpa-onnx dependency bundle: $($library.Name)"
            }
            Assert-PlainPath $library.FullName
            $built = Join-Path $releaseDirectory $library.Name
            if (-not (Test-Path -LiteralPath $built -PathType Leaf) -or $library.Length -eq 0) { $matchesRelease = $false; break }
            $hash = (Get-FileHash -LiteralPath $library.FullName -Algorithm SHA256).Hash
            if ((Get-FileHash -LiteralPath $built -Algorithm SHA256).Hash -ne $hash) { $matchesRelease = $false; break }
            $snapshot += @{ name=$library.Name; sha256=$hash; source=$library.FullName; version=$version }
        }
        if ($matchesRelease) { return $snapshot }
    }
    throw 'Required shared sherpa-onnx runtime DLLs are missing or do not match the versioned native build bundle; refusing delivery.'
}
function Publish-Artifacts {
    $targets = @(@{ path=(Join-Path $repoRoot 'bin'); backup=(Join-Path $jobDirectory 'backup-bin') })
    if ($request.deployTo) { $targets += @{ path=$request.deployTo; backup=(Join-Path $request.deployTo "backups/release-$($request.id)") } }
    $pending = @()
    foreach ($target in $targets) {
        Assert-PlainPath $target.path
        Assert-PlainPath $target.backup
        [IO.Directory]::CreateDirectory($target.path) | Out-Null
        [IO.Directory]::CreateDirectory((Join-Path $target.backup 'staged')) | Out-Null
        foreach ($artifact in $script:state.artifacts) {
            $destination = Join-Path $target.path $artifact.name
            Assert-PlainPath $destination
            $oldHash = $null
            $backup = Join-Path $target.backup $artifact.name
            if (Test-Path -LiteralPath $destination) {
                $oldHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
                Copy-Item -LiteralPath $destination -Destination $backup
                if ((Get-FileHash -LiteralPath $backup).Hash -ne $oldHash) { throw 'Backup hash mismatch.' }
            }
            $staged = Join-Path $target.backup "staged/$($artifact.name)"
            Copy-Item -LiteralPath (Join-Path $jobDirectory "artifacts/$($artifact.name)") -Destination $staged
            if ((Get-FileHash -LiteralPath $staged).Hash -ne $artifact.sha256) { throw 'Staging hash mismatch.' }
            $pending += @{ path=$destination; backup=$backup; staged=$staged; oldHash=$oldHash; sha256=$artifact.sha256 }
        }
    }
    $script:state.delivery = $pending
    Save-State
    if (-not $request.ContainsKey('sourceJob')) { Assert-Inputs }
    $changed = @()
    try {
        foreach ($item in $pending) {
            $currentHash = if (Test-Path -LiteralPath $item.path) { (Get-FileHash -LiteralPath $item.path).Hash } else { $null }
            if ($currentHash -ne $item.oldHash) { throw "Destination changed after backup: $($item.path)" }
            if ($currentHash -ne $item.sha256) {
                if ($item.oldHash) { [IO.File]::Replace($item.staged, $item.path, "$($item.backup).replaced", $true) }
                else { [IO.File]::Move($item.staged, $item.path) }
                $changed += $item
            }
            if ((Get-FileHash -LiteralPath $item.path).Hash -ne $item.sha256) { throw "Published hash mismatch: $($item.path)" }
        }
    } catch {
        $failure = $_.Exception.Message
        foreach ($item in $changed) {
            if ($item.oldHash) {
                try {
                    Copy-Item -LiteralPath $item.backup -Destination "$($item.staged).restore"
                    [IO.File]::Replace("$($item.staged).restore", $item.path, "$($item.backup).rollback-replaced", $true)
                    if ((Get-FileHash -LiteralPath $item.path).Hash -ne $item.oldHash) { throw 'Restored hash mismatch.' }
                } catch { $failure += " Rollback failed for $($item.path): $($_.Exception.Message)" }
            } else { $failure += " Newly created file retained for review: $($item.path)." }
        }
        throw $failure
    }
}

function Save-Artifacts($SourceDirectory) {
    $archive = Join-Path $jobDirectory 'artifacts'
    Assert-PlainPath $archive
    [IO.Directory]::CreateDirectory($archive) | Out-Null
    foreach ($artifact in $script:state.artifacts) {
        $source = Join-Path $SourceDirectory $artifact.name
        Assert-PlainPath $source
        $file = Get-Item -LiteralPath $source
        if ($file.Length -ne $artifact.length -or (Get-FileHash -LiteralPath $source).Hash -ne $artifact.sha256) { throw "Source artifact mismatch: $($artifact.name)" }
        $destination = Join-Path $archive $artifact.name
        Copy-Item -LiteralPath $source -Destination $destination
        if ((Get-FileHash -LiteralPath $destination).Hash -ne $artifact.sha256) { throw 'Archive hash mismatch.' }
    }
}

function Invoke-Redeploy {
    if ($Job -notmatch '^\d{8}-\d{6}-[a-f0-9]{8}$') { throw 'Redeploy requires a successful build -Job identifier.' }
    $latest = Join-Path $jobsRoot 'latest.json'
    if (Test-Path -LiteralPath $latest) {
        if ((Read-Status (Join-Path $jobsRoot (Read-Json $latest).id)).state -in @('queued', 'running')) { throw 'A release job is already queued/running.' }
    }
    Assert-NoCompiler
    $sourceDirectory = Join-Path $jobsRoot $Job
    Assert-PlainPath $sourceDirectory
    foreach ($file in @('status.json', 'request.json')) { Assert-PlainPath (Join-Path $sourceDirectory $file) }
    $original = Read-Json (Join-Path $sourceDirectory 'status.json')
    $sourceRequest = Read-Json (Join-Path $sourceDirectory 'request.json')
    $buildDirectory = $sourceDirectory
    $buildJob = $Job
    if ($sourceRequest.ContainsKey('sourceJob')) {
        if ($original.id -ne $Job -or $sourceRequest.id -ne $Job -or $sourceRequest.repoRoot -ne $repoRoot -or $original.state -ne 'succeeded' -or $original.stage -ne 'complete' -or $sourceRequest.sourceJob -notmatch '^\d{8}-\d{6}-[a-f0-9]{8}$') { throw 'Invalid redeploy source record.' }
        $buildJob = $sourceRequest.sourceJob
        $buildDirectory = Join-Path $jobsRoot $buildJob
        foreach ($file in @('status.json', 'request.json')) { Assert-PlainPath (Join-Path $buildDirectory $file) }
        if ((Get-FileHash -LiteralPath (Join-Path $buildDirectory 'status.json')).Hash -ne $sourceRequest.sourceStatusHash -or (Get-FileHash -LiteralPath (Join-Path $buildDirectory 'request.json')).Hash -ne $sourceRequest.sourceRequestHash) { throw 'Original build evidence changed.' }
        $original = Read-Json (Join-Path $buildDirectory 'status.json')
        $sourceRequest = Read-Json (Join-Path $buildDirectory 'request.json')
    }
    $required = @('renderer-check','rust-fmt','rust-check','test-codex-plus-core','test-codex-plus-data','test-codex-plus-manager','typescript','frontend-tests','vite','release')
    if ($original.id -ne $buildJob -or $sourceRequest.id -ne $buildJob -or $sourceRequest.repoRoot -ne $repoRoot -or $sourceRequest.ContainsKey('sourceJob') -or $original.state -ne 'succeeded' -or $original.stage -ne 'complete' -or @($original.steps).Count -ne $required.Count -or @($original.steps | Where-Object { $_.exitCode -ne 0 }).Count -or @($required | Where-Object { $_ -notin $original.steps.name }).Count) { throw 'Source must be a complete successful build from this repository.' }
    $platform = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    if ($sourceRequest.ContainsKey('platform') -and $sourceRequest.platform -ne $platform) { throw 'Source platform mismatch.' }
    $runtime = @($original.runtimeDependencies)
    $expectedNames = @($names) + @($runtime | ForEach-Object { $_.name })
    if (@($original.artifacts).Count -ne $expectedNames.Count -or @($expectedNames | Select-Object -Unique).Count -ne $expectedNames.Count -or @($original.artifacts.name | Select-Object -Unique).Count -ne $expectedNames.Count) { throw 'Incomplete or duplicate artifact manifest.' }
    foreach ($artifact in $original.artifacts) {
        if ($artifact.name -notin $expectedNames -or $artifact.name -notmatch '^[A-Za-z0-9_.-]+\.(exe|dll)$' -or $artifact.sha256 -notmatch '^[A-Fa-f0-9]{64}$' -or $artifact.length -le 0) { throw 'Invalid artifact manifest.' }
        if ($artifact.name -like '*.dll' -and @($runtime | Where-Object { $_.name -eq $artifact.name -and $_.sha256 -eq $artifact.sha256 }).Count -ne 1) { throw 'Runtime provenance mismatch.' }
    }
    if ($DeployTo) {
        $DeployTo = (Get-Item -LiteralPath $DeployTo).FullName.TrimEnd('\')
        Assert-PlainPath $DeployTo
        if ($DeployTo -eq (Join-Path $repoRoot 'bin')) { throw 'bin is already delivered by default; omit -DeployTo.' }
        foreach ($name in $names) { if (-not (Test-Path -LiteralPath (Join-Path $DeployTo $name) -PathType Leaf)) { throw "Not an existing runtime directory: missing $name" } }
    }
    $sourceArtifacts = Join-Path $sourceDirectory 'artifacts'
    $legacy = $Job -eq $buildJob -and -not $sourceRequest.ContainsKey('platform') -and -not (Test-Path -LiteralPath $sourceArtifacts)
    if ($legacy) { $sourceArtifacts = Join-Path $repoRoot 'bin' }
    Assert-PlainPath $sourceArtifacts
    if (-not (Test-Path -LiteralPath $sourceArtifacts -PathType Container)) { throw 'Source archive is missing.' }
    if (-not $sourceRequest.ContainsKey('platform')) {
        # 旧记录没有架构字段，核对真实 PE；绝不按文件名猜架构。
        foreach ($name in $names) {
            $file = Join-Path $sourceArtifacts $name
            Assert-PlainPath $file
            $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($file))
            try {
                if ($reader.ReadUInt16() -ne 0x5A4D) { throw 'Invalid PE artifact.' }
                $reader.BaseStream.Position = 0x3c; $offset = $reader.ReadUInt32()
                $reader.BaseStream.Position = $offset
                if ($reader.ReadUInt32() -ne 0x4550) { throw 'Invalid PE signature.' }
                $machine = $reader.ReadUInt16()
                if (($platform -eq 'X64' -and $machine -ne 0x8664) -or ($platform -eq 'Arm64' -and $machine -ne 0xAA64) -or $platform -notin @('X64','Arm64')) { throw 'Source platform mismatch.' }
            } finally { $reader.Dispose() }
        }
    }
    $id = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $jobDirectory = Join-Path $jobsRoot $id
    [IO.Directory]::CreateDirectory($jobDirectory) | Out-Null
    $request = @{ id=$id; repoRoot=$repoRoot; sourceJob=$buildJob; copiedFromJob=$Job; deployTo=$DeployTo; platform=$platform; legacyCapture=$legacy; sourceStatusHash=(Get-FileHash -LiteralPath (Join-Path $buildDirectory 'status.json')).Hash; sourceRequestHash=(Get-FileHash -LiteralPath (Join-Path $buildDirectory 'request.json')).Hash }
    Write-Json (Join-Path $jobDirectory 'request.json') $request
    $script:state = @{ id=$id; state='running'; stage='archive-validation'; createdAt=(Get-Date).ToUniversalTime().ToString('o'); updatedAt=''; error=$null; steps=@(); artifacts=$original.artifacts; runtimeDependencies=$runtime; delivery=@(); sourceJob=$buildJob; restartPerformed=$false }
    Save-State
    Write-Json (Join-Path $jobDirectory 'process.json') @{ pid=$PID; startedAt=(Get-Process -Id $PID).StartTime.ToUniversalTime().ToString('o') }
    Write-Json $latest @{ id=$id }
    try {
        Save-Artifacts $sourceArtifacts
        Copy-Item -LiteralPath (Join-Path $buildDirectory 'status.json') -Destination (Join-Path $jobDirectory 'source-status.json')
        Copy-Item -LiteralPath (Join-Path $buildDirectory 'request.json') -Destination (Join-Path $jobDirectory 'source-request.json')
        $script:state.stage = 'delivery'; Save-State
        Publish-Artifacts
        $script:state['runningInstances'] = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in $names } | Select-Object ProcessId, Name, CreationDate, ExecutablePath)
        $script:state.state = 'succeeded'; $script:state.stage = 'complete'
    } catch { $script:state.state='failed'; $script:state.error=$_.Exception.Message }
    Save-State
    [IO.File]::WriteAllText((Join-Path $jobDirectory 'summary.md'), "# Redeploy $id`nSource build: $buildJob`nArchive job: $Job`nState: $($script:state.state)`nError: $($script:state.error)`nNo Cargo/Node invocation. No application restart. This delivers the source build, not current source changes.`n")
    $script:state | ConvertTo-Json -Depth 12
    if ($script:state.state -ne 'succeeded') { throw "Redeploy failed: $($script:state.error)" }
}

if ($Action -eq 'Worker') {
    $jobDirectory = [IO.Path]::GetFullPath($Job)
    $request = Read-Json (Join-Path $jobDirectory 'request.json')
    $repoRoot = $request.repoRoot
    if ($jobDirectory -ne [IO.Path]::GetFullPath((Join-Path $repoRoot "output/release-jobs/$($request.id)"))) { throw 'Job directory mismatch.' }
}
$jobsRoot = Join-Path $repoRoot 'output/release-jobs'
Assert-PlainPath $jobsRoot
if ($Action -eq 'Status') {
    if (-not $Job) {
        if (-not (Test-Path -LiteralPath (Join-Path $jobsRoot 'latest.json'))) { @{ state='none'; message='No release job has been started.' } | ConvertTo-Json; exit 0 }
        $Job = (Read-Json (Join-Path $jobsRoot 'latest.json')).id
    }
    if ($Job -notmatch '^\d{8}-\d{6}-[a-f0-9]{8}$') { throw 'Invalid job identifier.' }
    Read-Status (Join-Path $jobsRoot $Job) | ConvertTo-Json -Depth 12
    exit 0
}
[IO.Directory]::CreateDirectory($jobsRoot) | Out-Null
$lock = $null
try {
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        try { $lock = [IO.File]::Open((Join-Path $jobsRoot 'active.lock'), 'OpenOrCreate', 'ReadWrite', 'None'); break }
        catch [IO.IOException] {
            if ($Action -in @('Start', 'Redeploy')) { throw 'A release job already owns the workspace lock. Use -Action Status.' }
            Start-Sleep -Milliseconds 100
        }
    }
    if (-not $lock) { throw 'Unable to acquire the release lock.' }
    if ($Action -eq 'Redeploy') { Invoke-Redeploy; exit 0 }
    if ($Action -eq 'Start') {
        $latest = Join-Path $jobsRoot 'latest.json'
        if (Test-Path -LiteralPath $latest) {
            $previous = Read-Status (Join-Path $jobsRoot (Read-Json $latest).id)
            if ($previous.state -in @('queued', 'running')) { throw 'A release job is already queued/running. Use -Action Status.' }
        }
        Assert-NoCompiler
        if ($DeployTo) {
            $DeployTo = (Get-Item -LiteralPath $DeployTo).FullName.TrimEnd('\')
            Assert-PlainPath $DeployTo
            if ($DeployTo -eq (Join-Path $repoRoot 'bin')) { throw 'bin is already delivered by default; omit -DeployTo.' }
            foreach ($name in $names) { if (-not (Test-Path -LiteralPath (Join-Path $DeployTo $name) -PathType Leaf)) { throw "Not an existing runtime directory: missing $name" } }
        }
        $script:rgPath = (Get-Command rg -CommandType Application | Select-Object -First 1).Source
        $cargo = if ($CargoPath) { (Get-Item -LiteralPath $CargoPath).FullName } else { (Get-Command cargo -CommandType Application | Select-Object -First 1).Source }
        $defaultNode = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        $candidates = if ($NodePath) { @($NodePath) } else {
            @($(if ($defaultNode) { $defaultNode.Source }), (Join-Path $env:USERPROFILE '.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node.exe')) | Where-Object { $_ }
        }
        $node = $null
        foreach ($candidate in $candidates) {
            if (-not (Test-Path -LiteralPath $candidate)) { continue }
            $version = & $candidate -v
            if ($LASTEXITCODE -eq 0 -and "$version" -match '^v(\d+)\.' -and [int]$Matches[1] -ge 24) { $node = (Get-Item -LiteralPath $candidate).FullName; break }
        }
        if (-not $node) { throw 'Node 24+ is required for TypeScript tests. Pass -NodePath; no dependencies were installed.' }
        $id = (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
        $jobDirectory = Join-Path $jobsRoot $id
        [IO.Directory]::CreateDirectory($jobDirectory) | Out-Null
        $request = @{ id=$id; repoRoot=$repoRoot; node=$node; cargo=$cargo; rg=$script:rgPath; jobs=$Jobs; expectedUiText=$ExpectedUiText; deployTo=$DeployTo; inputs=@(Get-Snapshot) }
        $request['platform'] = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
        Write-Json (Join-Path $jobDirectory 'request.json') $request
        $script:state = @{ id=$id; state='queued'; stage='queued'; createdAt=(Get-Date).ToUniversalTime().ToString('o'); updatedAt=''; error=$null; steps=@(); artifacts=@(); delivery=@(); restartPerformed=$false }
        Save-State
        Copy-Item -LiteralPath $PSCommandPath -Destination (Join-Path $jobDirectory 'runner.ps1')
        Write-Json $latest @{ id=$id }
        $worker = Join-Path $jobDirectory 'runner.ps1'
        try {
            $process = Start-Process -FilePath (Join-Path $PSHOME 'pwsh.exe') -WindowStyle Hidden -PassThru -ArgumentList @('-NoLogo', '-NoProfile', '-File', ('"' + $worker + '"'), '-Action', 'Worker', '-Job', ('"' + $jobDirectory + '"')) -RedirectStandardOutput (Join-Path $jobDirectory 'worker.out.log') -RedirectStandardError (Join-Path $jobDirectory 'worker.err.log')
            Write-Json (Join-Path $jobDirectory 'process.json') @{ pid=$process.Id; startedAt=$process.StartTime.ToUniversalTime().ToString('o') }
        } catch { $script:state.state='failed'; $script:state.error=$_.Exception.Message; Save-State; throw }
        @{ id=$id; state='queued'; pid=$process.Id; jobDirectory=$jobDirectory; message='Detached worker started. This is not a successful release; read Status later.' } | ConvertTo-Json
    } else {
        $script:state = Read-Json (Join-Path $jobDirectory 'status.json')
        if ($script:state.state -ne 'queued') { throw 'Only a new queued job may run. Start a new job instead of replaying old backups.' }
        $script:state.state = 'running'
        $script:rgPath = $request.rg
        $env:PATH = (Split-Path -Parent $request.node) + ';' + $env:PATH
        try {
            Assert-NoCompiler
            Assert-Inputs
            Invoke-Step 'renderer-check' $request.node @('scripts/assemble-renderer-inject.mjs', '--check')
            Invoke-Step 'rust-fmt' $request.cargo @('fmt', '--all', '--', '--check')
            Invoke-Step 'rust-check' $request.cargo @('check', '--workspace', '--all-targets', '-j', "$($request.jobs)")
            foreach ($package in @('codex-plus-core', 'codex-plus-data', 'codex-plus-manager')) {
                $selection = if ($package -eq 'codex-plus-manager') { '--lib' } else { '--tests' }
                Invoke-Step "test-$package" $request.cargo @('test', '-p', $package, $selection, '--no-fail-fast', '-j', "$($request.jobs)", '--', '--test-threads=1')
            }
            $frontend = Join-Path $repoRoot 'apps/codex-plus-manager'
            Invoke-Step 'typescript' $request.node @('node_modules/typescript/bin/tsc', '--noEmit', '-p', 'tsconfig.json') $frontend
            $testFiles = @(Get-ChildItem -LiteralPath (Join-Path $frontend 'src') -Filter '*.test.ts' -File | ForEach-Object FullName)
            if (-not $testFiles.Count) { throw 'No frontend tests found.' }
            Invoke-Step 'frontend-tests' $request.node (@('--test') + $testFiles) $frontend
            Assert-Inputs
            $viteStarted = (Get-Date).ToUniversalTime()
            Invoke-Step 'vite' $request.node @('node_modules/vite/bin/vite.js', 'build') $frontend
            $index = Get-Item -LiteralPath (Join-Path $frontend 'dist/index.html')
            if ($index.LastWriteTimeUtc -lt $viteStarted.AddSeconds(-1)) { throw 'Vite did not refresh dist/index.html.' }
            $bundles = @(Get-ChildItem -LiteralPath (Join-Path $frontend 'dist/assets') -Filter 'index-*.js' -File)
            if (-not $bundles.Count) { throw 'No Vite index bundle found.' }
            if ($request.expectedUiText -and -not @($bundles | Where-Object { [IO.File]::ReadAllText($_.FullName, [Text.Encoding]::UTF8).Contains($request.expectedUiText) }).Count) { throw 'Expected UI text is missing from dist; refusing release.' }
            $script:distSnapshot = @(Get-Snapshot -Dist)
            Write-Json (Join-Path $jobDirectory 'frontend.json') @{ indexModifiedAt=$index.LastWriteTimeUtc.ToString('o'); expectedUiText=$request.expectedUiText; files=$script:distSnapshot }
            Assert-Inputs
            Invoke-Step 'release' $request.cargo @('build', '--workspace', '--release', '-j', "$($request.jobs)")
            Assert-NoCompiler
            Assert-Inputs
            $script:state['runtimeDependencies'] = @(Get-RuntimeDependencySnapshot)
            $artifactNames = @($names) + @($script:state.runtimeDependencies | ForEach-Object { $_.name })
            foreach ($name in $artifactNames) {
                $artifact = Get-Item -LiteralPath (Join-Path $repoRoot "target/release/$name")
                if ($artifact.Length -eq 0) { throw "Empty release artifact: $name" }
                if ($name -eq $names[0] -and $artifact.LastWriteTimeUtc -lt $index.LastWriteTimeUtc) { throw 'Manager predates generated frontend; refusing delivery.' }
                $script:state.artifacts += @{ name=$name; sha256=(Get-FileHash -LiteralPath $artifact.FullName).Hash; lastWriteTime=$artifact.LastWriteTimeUtc.ToString('o'); length=$artifact.Length }
            }
            $script:state.stage = 'delivery'
            Save-State
            Save-Artifacts (Join-Path $repoRoot 'target/release')
            Publish-Artifacts
            $script:state['runningInstances'] = @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in $names } | Select-Object ProcessId, Name, CreationDate, ExecutablePath)
            $script:state.state = 'succeeded'
            $script:state.stage = 'complete'
        } catch { $script:state.state='failed'; $script:state.error=$_.Exception.Message }
        $summary = @("# Release $($request.id)", "State: $($script:state.state)", "Stage: $($script:state.stage)", "Error: $($script:state.error)", 'No application restart was performed.', '', 'Artifacts:')
        $summary += @($script:state.artifacts | ForEach-Object { "$($_.name)  $($_.sha256)" })
        $summary += @('', 'See status.json for step logs, delivery paths, backups and running instances.')
        [IO.File]::WriteAllText((Join-Path $jobDirectory 'summary.md'), ($summary -join "`n"), [Text.UTF8Encoding]::new($false))
        Save-State
        if ($script:state.state -ne 'succeeded') { exit 1 }
    }
} finally { if ($lock) { $lock.Dispose() } }
