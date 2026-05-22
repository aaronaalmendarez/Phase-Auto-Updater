$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

& (Join-Path $PSScriptRoot "build-windows.ps1")

$ToolDir = Join-Path $Root ".tools\wix"
$WixExe = Join-Path $ToolDir "wix.exe"
if (!(Test-Path $WixExe)) {
    New-Item -ItemType Directory -Force $ToolDir | Out-Null
    dotnet tool install --tool-path $ToolDir wix --version 5.0.2
}

$CargoToml = Get-Content -Raw (Join-Path $Root "Cargo.toml")
if ($CargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw "Could not read package version from Cargo.toml"
}
$PackageVersion = $Matches[1]

$DistDir = Join-Path $Root "dist\windows"
$WxsPath = Join-Path $Root "installer\windows\PhaseAutoUpdater.wxs"
$MsiPath = Join-Path $DistDir "PhaseAutoUpdater-$PackageVersion.msi"
$SetupPath = Join-Path $DistDir "PhaseAnimatorSetup.exe"

& $WixExe build $WxsPath `
    -arch x64 `
    -d "SourceDir=$DistDir" `
    -d "PackageVersion=$PackageVersion" `
    -out $MsiPath

if (!(Test-Path $MsiPath)) {
    throw "MSI was not created"
}

$env:PHASE_MSI_PATH = $MsiPath
cargo build --release --bin phase-setup --features embed-msi
& (Join-Path $Root "target\release\phase-setup.exe") --smoke-test
Copy-Item -Force (Join-Path $Root "target\release\phase-setup.exe") $SetupPath

if (!(Test-Path $SetupPath)) {
    throw "Setup executable was not created"
}

Write-Host "Built $MsiPath"
Write-Host "Built $SetupPath"
Write-Host "Optional start-on-login install: msiexec /i `"$MsiPath`" START_ON_LOGIN=1"
