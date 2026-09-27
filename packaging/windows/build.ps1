#Requires -Version 7

param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[a-p]{32}$', Options = 'None')]
    [string] $ExtensionId,

    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $root 'out\windows'
}
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path $OutputDirectory).Path

$target = 'x86_64-pc-windows-msvc'
& rustup target add $target
if ($LASTEXITCODE -ne 0) {
    throw "rustup target add $target failed with exit code $LASTEXITCODE"
}
& cargo build --manifest-path (Join-Path $root 'native\Cargo.toml') --release --locked --target $target -p pervue-host
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with exit code $LASTEXITCODE"
}

$hostSource = Join-Path $root "native\target\$target\release\pervue-host.exe"
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
    $stagedHost = Join-Path $stage 'pervue-host.exe'
    Copy-Item -LiteralPath $hostSource -Destination $stagedHost

    # Let the exact built host validate the extension ID and construct the
    # allowlist. Verify its complete identity before changing the one field
    # Windows intentionally makes relative to the manifest.
    $manifestText = (& $hostSource --print-manifest $ExtensionId | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw "pervue-host --print-manifest failed with exit code $LASTEXITCODE"
    }
    $manifest = $manifestText | ConvertFrom-Json
    $expectedOrigin = "chrome-extension://$ExtensionId/"
    $generatedHost = [IO.Path]::GetFullPath([string]$manifest.path)
    if ($manifest.name -ne 'com.pervue.host' -or
        $manifest.type -ne 'stdio' -or
        @($manifest.allowed_origins).Count -ne 1 -or
        @($manifest.allowed_origins)[0] -cne $expectedOrigin -or
        $generatedHost -ine [IO.Path]::GetFullPath($hostSource)) {
        throw 'Unexpected Native Messaging manifest from pervue-host --print-manifest.'
    }
    $manifest.path = 'pervue-host.exe'

    $utf8 = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText(
        (Join-Path $stage 'com.pervue.host.json'),
        ($manifest | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
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
        foreach ($version in @('7', '6')) {
            foreach ($candidate in @(
                (Join-Path $programFilesX86 "Inno Setup $version\ISCC.exe"),
                (Join-Path $programFiles "Inno Setup $version\ISCC.exe")
            )) {
                if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                    $iscc = $candidate
                    break
                }
            }
            if ($iscc) { break }
        }
    }
    if (-not $iscc) {
        throw 'Inno Setup 6.3 or newer compiler (ISCC.exe) is required.'
    }

    $innoVersionText = (& $iscc --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $innoVersionText -notmatch '(\d+)\.(\d+)(?:\.(\d+))?') {
        throw "Could not determine Inno Setup version from: $innoVersionText"
    }
    $innoVersion = [Version]::new(
        [int]$Matches[1],
        [int]$Matches[2],
        $(if ($Matches[3]) { [int]$Matches[3] } else { 0 })
    )
    if ($innoVersion -lt [Version]'6.3.0') {
        throw "Inno Setup 6.3 or newer is required; found $innoVersion."
    }

    $iss = Join-Path $PSScriptRoot 'Pervue.iss'
    $compilerArgs = @(
        "/DStageDir=$stage",
        "/DOutputDir=$OutputDirectory",
        "/DHostVersion=$hostVersion",
        "/DPackageVersion=$packageVersion",
        "/DSourceCommitShort=$sourceCommitShort"
    )

    $signCommand = [Environment]::GetEnvironmentVariable('PERVUE_WINDOWS_SIGN_COMMAND')
    if ($signCommand) {
        if (-not $signCommand.Contains('$f')) {
            throw 'PERVUE_WINDOWS_SIGN_COMMAND must contain Inno Setup''s $f filename placeholder.'
        }
        $compilerArgs += '/DPervueSignTool=1'
        $compilerArgs += "--signtool=pervue=$signCommand"
    }

    & $iscc @compilerArgs $iss
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
