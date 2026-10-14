param(
    [string]$Output = 'dist',
    [switch]$SourceOnly
)
$ErrorActionPreference = 'Stop'
$projectTrend = $PSScriptRoot
Set-Location -LiteralPath $projectTrend
if (Test-Path -LiteralPath '.rust\cargo\bin\cargo.exe') {
    $env:CARGO_HOME = Join-Path $projectTrend '.rust\cargo'
    $env:RUSTUP_HOME = Join-Path $projectTrend '.rust\rustup'
    $env:PATH = "$env:CARGO_HOME\bin;$env:PATH"
}
# Avoid personal build paths in Rust panic strings; statically link the MSVC runtime.
$env:CARGO_ENCODED_RUSTFLAGS = '--remap-path-prefix' + [char]31 + ($projectTrend + '=/project')
if ($env:OS -eq 'Windows_NT') {
    $env:CARGO_ENCODED_RUSTFLAGS += [char]31 + '-C' + [char]31 + 'target-feature=+crt-static'
}
# Dependency sources live under CARGO_HOME, by default in the user profile.
$cargoHomeTrend = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
$env:CARGO_ENCODED_RUSTFLAGS += [char]31 + '--remap-path-prefix' + [char]31 + ($cargoHomeTrend + '=/cargo')
$publicTrend = @(
    '.gitignore', 'Cargo.toml', 'Cargo.lock', 'build.ps1', 'build.rs',
    'README.md', 'README_OPERATORE.txt', 'CONTRIBUTING.md', 'AGENTS.md', 'CLAUDE.md', 'MAINTAINING.md', 'THIRD_PARTY_LICENSES.txt',
    'src/main.rs', 'src/archive.rs', 'src/gui.rs', 'src/demo.rs', 'src/tests.rs',
    'assets/icon.png', 'assets/icon.ico', 'docs/images/demo-daily.png', 'docs/images/demo-weekly.png',
    '.github/pull_request_template.md', '.github/ISSUE_TEMPLATE/bug_report.yml',
    '.github/ISSUE_TEMPLATE/feature_request.yml', '.github/ISSUE_TEMPLATE/config.yml'
)
if (Test-Path -LiteralPath 'LICENSE' -PathType Leaf) { $publicTrend += 'LICENSE' }
# Validate every ancestor, including docs/assets and destination parents.
function Assert-PlainPathTrend([string]$Path) {
    $walkTrend = [IO.Path]::GetFullPath($Path)
    while ($walkTrend) {
        if (Test-Path -LiteralPath $walkTrend) {
            $itemTrend = Get-Item -LiteralPath $walkTrend -Force
            if ($itemTrend.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw 'Redirected paths are not allowed in release packages'
            }
        }
        $walkTrend = [IO.Path]::GetDirectoryName($walkTrend)
    }
}
foreach ($nameTrend in $publicTrend) {
    $sourceTrend = Join-Path $projectTrend $nameTrend
    Assert-PlainPathTrend $sourceTrend
    if (-not (Test-Path -LiteralPath $sourceTrend -PathType Leaf)) {
        throw "Source missing: $nameTrend"
    }
}
$finalTrend = [IO.Path]::GetFullPath((Join-Path $projectTrend $Output))
if (-not $finalTrend.StartsWith($projectTrend.TrimEnd('\','/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Output must be a directory inside the project'
}
# The destination keeps only the latest release. Only these items are replaced;
# anything else (e.g. preferences written next to the executable) stops the build.
$releaseTrend = @('VesselTrendManager', 'VesselTrendManager_Rust_Windows_x64.zip', 'VesselTrendManager_Rust_Windows_x64.sha256',
    'vessel-trend-manager-source.zip', 'vessel-trend-manager-source.sha256')
$packageItemsTrend = @('VesselTrendManager.exe', 'README_OPERATORE.txt', 'THIRD_PARTY_LICENSES.txt', 'THIRD_PARTY_NOTICES.txt', 'LICENSE', 'licenses')
function Assert-OnlyReleaseTrend {
    Assert-PlainPathTrend $finalTrend
    if (-not (Test-Path -LiteralPath $finalTrend)) { return }
    $unknownTrend = @(Get-ChildItem -LiteralPath $finalTrend -Force | Where-Object { $releaseTrend -notcontains $_.Name })
    $oldPackageTrend = Join-Path $finalTrend 'VesselTrendManager'
    if (Test-Path -LiteralPath $oldPackageTrend) {
        $unknownTrend += @(Get-ChildItem -LiteralPath $oldPackageTrend -Force | Where-Object { $packageItemsTrend -notcontains $_.Name })
        $unknownTrend += @(Get-ChildItem -LiteralPath $oldPackageTrend -Recurse -Force | Where-Object {
            ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $_.Extension -in '.s3db','.json','.csv','.xlsx','.png','.tmp'
        })
    }
    if ($unknownTrend) {
        throw "Unexpected items in ${Output}: move them elsewhere first. No file is deleted. $(($unknownTrend.Name | Select-Object -Unique) -join ', ')"
    }
}
Assert-OnlyReleaseTrend
# Build into a staging folder; the destination changes only after a complete build.
$outputTrend = $finalTrend + '.new'
Assert-PlainPathTrend $outputTrend
if (Test-Path -LiteralPath $outputTrend) {
    throw "Staging folder $Output.new already exists: check and remove it. No file is deleted."
}

function Invoke-CargoTrend {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "Cargo did not complete: $args" }
}
if (-not $SourceOnly) {
    if ($env:OS -ne 'Windows_NT' -or -not [Environment]::Is64BitProcess) { throw 'Build the Windows package from Windows x64.' }
    Invoke-CargoTrend fmt --all '--' --check
    Invoke-CargoTrend clippy --all-targets --locked --offline '--' -D warnings
    Invoke-CargoTrend test --locked --offline
    Invoke-CargoTrend build --release --locked --offline
}
# Check again after compilation; creating the output must not replace an existing directory.
Assert-PlainPathTrend $outputTrend
New-Item -ItemType Directory -Path $outputTrend | Out-Null
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
function Write-ZipTrend([string]$Destination, [object[]]$Entries) {
    $streamTrend = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew)
    $zipTrend = [IO.Compression.ZipArchive]::new($streamTrend, [IO.Compression.ZipArchiveMode]::Create)
    try {
        foreach ($entryTrend in $Entries) {
            Assert-PlainPathTrend $entryTrend.Path
            [IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zipTrend, $entryTrend.Path, $entryTrend.Name.Replace('\','/'), [IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally { $zipTrend.Dispose(); $streamTrend.Dispose() }
    $hashTrend = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText([IO.Path]::ChangeExtension($Destination, '.sha256'), "$hashTrend  $([IO.Path]::GetFileName($Destination))`n", [Text.Encoding]::ASCII)
}
if (-not $SourceOnly) {
    $packageTrend = Join-Path $outputTrend 'VesselTrendManager'
    New-Item -ItemType Directory -Path $packageTrend | Out-Null
    Copy-Item -LiteralPath 'target\release\VesselTrendManager.exe','README_OPERATORE.txt','THIRD_PARTY_LICENSES.txt' -Destination $packageTrend
    if (Test-Path -LiteralPath 'LICENSE') { Copy-Item -LiteralPath 'LICENSE' -Destination $packageTrend }
    $metadataTextTrend = & cargo metadata --locked --offline --format-version 1 --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata not available' }
    $metadataTrend = $metadataTextTrend | ConvertFrom-Json
    $treeTrend = & cargo tree --locked --offline --target x86_64-pc-windows-msvc --edges normal,build --prefix none --format '{p}'
    if ($LASTEXITCODE -ne 0) { throw 'Dependency tree not available' }
    $usedTrend = [Collections.Generic.HashSet[string]]::new()
    foreach ($lineTrend in $treeTrend) {
        if ($lineTrend -match '^([^\s]+) v([^\s]+)') { [void]$usedTrend.Add($Matches[1] + '@' + $Matches[2]) }
    }
    $supplementalTrend = Get-Content -LiteralPath 'THIRD_PARTY_LICENSES.txt' -Raw
    $noticesTrend = [Collections.Generic.List[string]]::new()
    $noticesTrend.Add('Third-party components. Their original license files are included in licenses/.')
    foreach ($dependencyTrend in $metadataTrend.packages) {
        if ($dependencyTrend.name -eq 'vessel-trend-manager' -or -not $usedTrend.Contains($dependencyTrend.name + '@' + $dependencyTrend.version)) { continue }
        $noticesTrend.Add("$($dependencyTrend.name) $($dependencyTrend.version) | $($dependencyTrend.license)")
        $baseTrend = Split-Path -Parent $dependencyTrend.manifest_path
        $licenseFilesTrend = @(Get-ChildItem -LiteralPath $baseTrend -Recurse -File | Where-Object {
            $_.Name -match '^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)' -or
            ($dependencyTrend.name -eq 'epaint_default_fonts' -and $_.Extension -eq '.txt')
        })
        $markerTrend = '(?m)^Package: ' + [regex]::Escape($dependencyTrend.name + '@' + $dependencyTrend.version) + '\r?$'
        if ($licenseFilesTrend.Count -eq 0 -and $supplementalTrend -notmatch $markerTrend) { throw "Licenses to verify: $($dependencyTrend.name)" }
        foreach ($licenseTrend in $licenseFilesTrend) {
            $relativeTrend = $licenseTrend.FullName.Substring($baseTrend.Length).TrimStart('\','/')
            $targetTrend = Join-Path $packageTrend ("licenses\$($dependencyTrend.name)-$($dependencyTrend.version)\" + $relativeTrend)
            New-Item -ItemType Directory -Path (Split-Path -Parent $targetTrend) -Force | Out-Null
            Copy-Item -LiteralPath $licenseTrend.FullName -Destination $targetTrend
        }
    }
    $rustRootTrend = (& rustc --print sysroot).Trim()
    foreach ($nameTrend in @('LICENSE-APACHE','LICENSE-MIT','COPYRIGHT.html')) {
        $licenseTrend = Join-Path $rustRootTrend ('share\doc\rust\' + $nameTrend)
        if (Test-Path -LiteralPath $licenseTrend) {
            $targetTrend = Join-Path $packageTrend 'licenses\Rust'
            New-Item -ItemType Directory -Path $targetTrend -Force | Out-Null
            Copy-Item -LiteralPath $licenseTrend -Destination $targetTrend
        }
    }
    $rustLicensesTrend = Join-Path $rustRootTrend 'share\doc\rust\licenses'
    if (Test-Path -LiteralPath $rustLicensesTrend) {
        $targetTrend = Join-Path $packageTrend 'licenses\Rust'
        New-Item -ItemType Directory -Path $targetTrend -Force | Out-Null
        Get-ChildItem -LiteralPath $rustLicensesTrend -File | Copy-Item -Destination $targetTrend
    }
    $noticesTrend.Add('SQLite is bundled with rusqlite/libsqlite3-sys. SQLite: https://www.sqlite.org/copyright.html (public domain).')
    [IO.File]::WriteAllLines((Join-Path $packageTrend 'THIRD_PARTY_NOTICES.txt'), $noticesTrend, [Text.Encoding]::UTF8)
    $binariesTrend = @(Get-ChildItem -LiteralPath $packageTrend -Recurse -File)
    if ($binariesTrend | Where-Object { $_.Extension -in '.s3db','.xlsx' -or $_.Name -eq 'preferenze.json' }) { throw 'Unexpected local data in the package' }
    $entriesTrend = @($binariesTrend | ForEach-Object { @{Path=$_.FullName; Name=$_.FullName.Substring($outputTrend.Length + 1)} })
    Write-ZipTrend (Join-Path $outputTrend 'VesselTrendManager_Rust_Windows_x64.zip') $entriesTrend
}
$sourcesTrend = @($publicTrend | ForEach-Object { @{Path=(Join-Path $projectTrend $_); Name=('vessel-trend-manager/' + $_)} })
Write-ZipTrend (Join-Path $outputTrend 'vessel-trend-manager-source.zip') $sourcesTrend
# Replace the previous release with the new one, checking the destination again.
Assert-OnlyReleaseTrend
New-Item -ItemType Directory -Path $finalTrend -Force | Out-Null
foreach ($nameTrend in $releaseTrend) {
    $oldTrend = Join-Path $finalTrend $nameTrend
    if (Test-Path -LiteralPath $oldTrend) { Remove-Item -LiteralPath $oldTrend -Recurse -Force }
}
Get-ChildItem -LiteralPath $outputTrend -Force | Move-Item -Destination $finalTrend
Remove-Item -LiteralPath $outputTrend
Write-Output "Packages completed in $Output; $($publicTrend.Count) allowed sources."
