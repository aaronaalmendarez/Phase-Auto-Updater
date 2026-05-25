$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Add-MsvcStaticRuntimePath {
    $VsRoot = Join-Path $env:ProgramFiles "Microsoft Visual Studio"
    if (!(Test-Path $VsRoot)) {
        return
    }

    $Lib = Get-ChildItem -Path $VsRoot -Recurse -Filter libcmt.lib -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -match "\\lib\\x64\\libcmt\.lib$" } |
        Select-Object -First 1
    if ($null -eq $Lib) {
        $Lib = Get-ChildItem -Path $VsRoot -Recurse -Filter libcmt.lib -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match "\\lib\\onecore\\x64\\libcmt\.lib$" } |
            Select-Object -First 1
    }
    if ($null -eq $Lib) {
        return
    }

    $LibDir = Split-Path -Parent $Lib.FullName
    $Existing = @($env:LIB -split ";" | Where-Object { $_ })
    if ($Existing -notcontains $LibDir) {
        $env:LIB = "$LibDir;$env:LIB"
    }
}

Add-MsvcStaticRuntimePath
$env:RUSTFLAGS = (($env:RUSTFLAGS, "-C target-feature=+crt-static") |
    Where-Object { $_ } |
    Select-Object -Unique) -join " "

cargo fmt --check
cargo build --release --bin phase-tool
& (Join-Path $Root "target\release\phase-tool.exe") --smoke-test

$OutDir = Join-Path $Root "dist\windows"
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item -Force (Join-Path $Root "target\release\phase-tool.exe") (Join-Path $OutDir "PhaseAnimatorInstaller.exe")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.png") (Join-Path $OutDir "PhaseAnimator.png")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.ico") (Join-Path $OutDir "PhaseAnimator.ico")

Write-Host "Built dist\windows\PhaseAnimatorInstaller.exe"
