$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

cargo fmt --check
cargo build --release --bin phase-tool
& (Join-Path $Root "target\release\phase-tool.exe") --smoke-test

$OutDir = Join-Path $Root "dist\windows"
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item -Force (Join-Path $Root "target\release\phase-tool.exe") (Join-Path $OutDir "PhaseAnimatorInstaller.exe")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.png") (Join-Path $OutDir "PhaseAnimator.png")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.ico") (Join-Path $OutDir "PhaseAnimator.ico")

Write-Host "Built dist\windows\PhaseAnimatorInstaller.exe"
