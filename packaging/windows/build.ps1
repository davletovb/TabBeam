param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[a-p]{32}$')]
    [string] $ExtensionId,

    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $root 'out\windows'
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path $OutputDirectory).Path

& cargo build --manifest-path (Join-Path $root 'native\Cargo.toml') --release --locked -p pervue-host
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$hostSource = Join-Path $root 'native\target\release\pervue-host.exe'
if (-not (Test-Path -LiteralPath $hostSource -PathType Leaf)) {
    throw "Built host not found: $hostSource"
}

$hostVersion = (& $hostSource --version).Trim()
if ($LASTEXITCODE -ne 0 -or $hostVersion -notmatch '^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$') {
    throw "Unsupported host version: $hostVersion"
}
$packageVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3])"

$sourceCommit = (& git -C $root rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sourceCommit -notmatch '^[0-9a-f]{40}$') {
    throw "Could not determine a full source commit"
}
$sourceCommitShort = $sourceCommit.Substring(0, 12)

$stage = Join-Path ([IO.Path]::GetTempPath()) ("pervue-windows-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Copy-Item -LiteralPath $hostSource -Destination (Join-Path $stage 'pervue-host.exe')

    # Let the host validate the extension ID and construct the allowlist. Chrome
    # explicitly permits a host path relative to the manifest on Windows, so
    # the installed manifest is independent of the user's profile directory.
    $manifestText = (& $hostSource --print-manifest $ExtensionId | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw "pervue-host --print-manifest failed with exit code $LASTEXITCODE"
    }
    $manifest = $manifestText | ConvertFrom-Json
    $manifest.path = 'pervue-host.exe'
    $manifestJson = $manifest | ConvertTo-Json -Depth 8
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText(
        (Join-Path $stage 'com.pervue.host.json'),
        $manifestJson + [Environment]::NewLine,
        $utf8
    )

    $buildInfo = [ordered]@{
        version = $hostVersion
        package_version = $packageVersion
        architecture = 'windows-x86_64'
        extension_id = $ExtensionId
        source_commit = $sourceCommit
    } | ConvertTo-Json
    [IO.File]::WriteAllText(
        (Join-Path $stage 'build-info.json'),
        $buildInfo + [Environment]::NewLine,
        $utf8
    )

    $iscc = $null
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) {
        $iscc = $command.Source
    }
    if (-not $iscc) {
        $programFilesX86 = [Environment]::GetFolderPath('ProgramFilesX86')
        $programFiles = [Environment]::GetFolderPath('ProgramFiles')
        foreach ($candidate in @(
            (Join-Path $programFilesX86 'Inno Setup 6\ISCC.exe'),
            (Join-Path $programFiles 'Inno Setup 6\ISCC.exe')
        )) {
            if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                $iscc = $candidate
                break
            }
        }
    }
    if (-not $iscc) {
        throw 'Inno Setup 6 compiler (ISCC.exe) is required to build the Windows companion.'
    }

    $iss = Join-Path $PSScriptRoot 'Pervue.iss'
    $defines = @(
        "/DStageDir=$stage",
        "/DOutputDir=$OutputDirectory",
        "/DExtensionId=$ExtensionId",
        "/DHostVersion=$hostVersion",
        "/DPackageVersion=$packageVersion",
        "/DSourceCommitShort=$sourceCommitShort"
    )
    & $iscc @defines $iss
    if ($LASTEXITCODE -ne 0) {
        throw "Inno Setup failed with exit code $LASTEXITCODE"
    }

    $installer = Get-ChildItem -LiteralPath $OutputDirectory -Filter "Pervue-$hostVersion-$sourceCommitShort-windows-x64.exe" |
        Select-Object -First 1
    if (-not $installer) {
        throw 'Inno Setup completed but the expected installer was not produced.'
    }
    Write-Output $installer.FullName
}
finally {
    Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
}
