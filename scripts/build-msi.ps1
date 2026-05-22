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

$ExtensionList = & $WixExe extension list
if ($ExtensionList -notmatch "WixToolset\.UI\.wixext") {
    & $WixExe extension add WixToolset.UI.wixext/5.0.2
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

function New-InstallerBitmap {
    param(
        [string] $Path,
        [int] $Width,
        [int] $Height,
        [int] $LogoSize,
        [int] $LogoX,
        [int] $LogoY,
        [bool] $WithText
    )

    Add-Type -AssemblyName System.Drawing

    $LogoPath = Join-Path $DistDir "PhaseAnimator.png"
    $Bitmap = New-Object System.Drawing.Bitmap $Width, $Height
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    $Graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $Graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $Graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit

    $Logo = [System.Drawing.Image]::FromFile($LogoPath)
    if ($Height -gt 100) {
        $Graphics.Clear([System.Drawing.Color]::FromArgb(248, 247, 252))
        $LeftPanel = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(19, 15, 36))
        $Divider = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(213, 205, 230))
        $Graphics.FillRectangle($LeftPanel, 0, 0, 164, $Height)
        $Graphics.FillRectangle($Divider, 164, 0, 1, $Height)
        $LeftPanel.Dispose()
        $Divider.Dispose()
    } else {
        $Graphics.Clear([System.Drawing.Color]::FromArgb(248, 247, 252))
        $Accent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(19, 15, 36))
        $Graphics.FillRectangle($Accent, 0, 54, $Width, 4)
        $Accent.Dispose()
    }

    $Graphics.DrawImage($Logo, $LogoX, $LogoY, $LogoSize, $LogoSize)

    if ($WithText) {
        $TitleFont = New-Object System.Drawing.Font "Segoe UI", 18, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
        $TextFont = New-Object System.Drawing.Font "Segoe UI", 10, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
        $TitleBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(32, 27, 49))
        $TextBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(92, 80, 119))
        $Graphics.DrawString("Phase Animator", $TitleFont, $TitleBrush, 160, 20)
        $Graphics.DrawString("Setup", $TextFont, $TextBrush, 162, 43)
        $TitleFont.Dispose()
        $TextFont.Dispose()
        $TitleBrush.Dispose()
        $TextBrush.Dispose()
    }

    $Logo.Dispose()
    $Graphics.Dispose()
    $Bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $Bitmap.Dispose()
}

New-InstallerBitmap `
    -Path (Join-Path $DistDir "WixUIDialogBmp.bmp") `
    -Width 493 `
    -Height 312 `
    -LogoSize 112 `
    -LogoX 26 `
    -LogoY 100 `
    -WithText $false

New-InstallerBitmap `
    -Path (Join-Path $DistDir "WixUIBannerBmp.bmp") `
    -Width 493 `
    -Height 58 `
    -LogoSize 40 `
    -LogoX 434 `
    -LogoY 8 `
    -WithText $false

& $WixExe build $WxsPath `
    -ext WixToolset.UI.wixext `
    -arch x64 `
    -d "SourceDir=$DistDir" `
    -d "InstallerDir=$(Join-Path $Root "installer\windows")" `
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
Write-Host "Run setup: `"$SetupPath`""
