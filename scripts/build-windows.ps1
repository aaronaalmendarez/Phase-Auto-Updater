param(
    [ValidateSet("x64", "x86", "arm64")]
    [string] $Arch = "x64"
)

$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Get-RustTarget {
    param([string] $Arch)
    switch ($Arch) {
        "x64" { "x86_64-pc-windows-msvc" }
        "x86" { "i686-pc-windows-msvc" }
        "arm64" { "aarch64-pc-windows-msvc" }
        default { throw "Unsupported Windows architecture: $Arch" }
    }
}

function Get-MsvcLibArch {
    param([string] $Arch)
    switch ($Arch) {
        "x64" { "x64" }
        "x86" { "x86" }
        "arm64" { "arm64" }
        default { throw "Unsupported Windows architecture: $Arch" }
    }
}

function Get-CargoTargetEnvKey {
    param([string] $Target)
    $Target.ToUpperInvariant().Replace("-", "_")
}

function Get-EnvPathList {
    param([string] $Name)

    $Value = [Environment]::GetEnvironmentVariable($Name, "Process")
    if (!$Value) {
        return @()
    }
    @($Value -split ";" | Where-Object { $_ })
}

function Join-ExistingPathList {
    param([string[]] $Paths)

    (@($Paths) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique) -join ";"
}

function Prepend-EnvPath {
    param(
        [string] $Name,
        [string[]] $Paths
    )

    $NextValue = @(
        @($Paths) | Where-Object { $_ -and (Test-Path $_) }
        Get-EnvPathList $Name
    ) | Select-Object -Unique

    if ($NextValue.Count -gt 0) {
        Set-Item -Path "Env:$Name" -Value ($NextValue -join ";")
    }
}

function Set-CargoTargetEnv {
    param(
        [string] $Target,
        [string] $Suffix,
        [string] $Value
    )

    $Key = "CARGO_TARGET_$(Get-CargoTargetEnvKey $Target)_$Suffix"
    Set-Item -Path "Env:$Key" -Value $Value
}

function Set-TargetToolEnv {
    param(
        [string] $Target,
        [string] $Tool,
        [string] $Value
    )

    $Key = "$Tool" + "_" + $Target.Replace("-", "_")
    Set-Item -Path "Env:$Key" -Value $Value
}

function Test-CanRunBuiltExe {
    param([string] $Arch)

    $ProcessArch = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture.ToString()
    if ($Arch -eq "arm64" -and $ProcessArch -ne "Arm64") {
        return $false
    }

    $true
}

function Get-MsvcToolsRoots {
    if ($script:MsvcToolsRoots) {
        return $script:MsvcToolsRoots
    }

    $Candidates = @()
    $VsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $VsWhere) {
        $InstallPath = & $VsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($InstallPath) {
            $ToolsRoot = Join-Path $InstallPath "VC\Tools\MSVC"
            if (Test-Path $ToolsRoot) {
                $Candidates += Get-ChildItem -Path $ToolsRoot -Directory -ErrorAction SilentlyContinue
            }
        }
    }

    $VsRoots = @(
        (Join-Path $env:ProgramFiles "Microsoft Visual Studio"),
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio")
    ) | Where-Object { $_ -and (Test-Path $_) }

    foreach ($VsRoot in $VsRoots) {
        Get-ChildItem -Path $VsRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            Get-ChildItem -Path $_.FullName -Directory -ErrorAction SilentlyContinue | ForEach-Object {
                $ToolsRoot = Join-Path $_.FullName "VC\Tools\MSVC"
                if (Test-Path $ToolsRoot) {
                    $Candidates += Get-ChildItem -Path $ToolsRoot -Directory -ErrorAction SilentlyContinue
                }
            }
        }
    }

    $Selected = $Candidates |
        Sort-Object FullName -Unique |
        Sort-Object { try { [Version] $_.Name } catch { [Version] "0.0" } } -Descending

    if ($null -eq $Selected -or @($Selected).Count -eq 0) {
        throw "Could not find MSVC build tools. Install Visual Studio Build Tools with the C++ desktop workload."
    }

    $script:MsvcToolsRoots = @($Selected | ForEach-Object { $_.FullName })
    $script:MsvcToolsRoots
}

function Get-WindowsSdkRoot {
    if ($script:WindowsSdkRoot) {
        return $script:WindowsSdkRoot
    }

    $SdkRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10"
    if (!(Test-Path $SdkRoot)) {
        throw "Could not find Windows SDK 10. Install the Windows 10/11 SDK."
    }

    $script:WindowsSdkRoot = $SdkRoot
    $script:WindowsSdkRoot
}

function Get-WindowsSdkVersion {
    if ($script:WindowsSdkVersion) {
        return $script:WindowsSdkVersion
    }

    $SdkRoot = Get-WindowsSdkRoot
    $IncludeRoot = Join-Path $SdkRoot "Include"
    $LibRoot = Join-Path $SdkRoot "Lib"
    $Selected = Get-ChildItem -Path $IncludeRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object {
            (Test-Path (Join-Path $_.FullName "ucrt")) -and
            (Test-Path (Join-Path $LibRoot "$($_.Name)\ucrt"))
        } |
        Sort-Object { try { [Version] $_.Name } catch { [Version] "0.0" } } -Descending |
        Select-Object -First 1

    if ($null -eq $Selected) {
        throw "Could not find a usable Windows SDK include/lib version."
    }

    $script:WindowsSdkVersion = $Selected.Name
    $script:WindowsSdkVersion
}

function Get-MsvcPaths {
    param([string] $Arch)

    $LibArch = Get-MsvcLibArch $Arch
    foreach ($MsvcRoot in Get-MsvcToolsRoots) {
        $IncludeDir = Join-Path $MsvcRoot "include"
        $LibDir = Join-Path $MsvcRoot "lib\$LibArch"
        if (!(Test-Path $LibDir)) {
            $LibDir = Join-Path $MsvcRoot "lib\onecore\$LibArch"
        }
        if (!(Test-Path (Join-Path $IncludeDir "vcruntime.h")) -or !(Test-Path (Join-Path $LibDir "libcmt.lib"))) {
            continue
        }

        $HostBin = if (Test-Path (Join-Path $MsvcRoot "bin\HostX64\$LibArch\link.exe")) {
            Join-Path $MsvcRoot "bin\HostX64\$LibArch"
        } elseif (Test-Path (Join-Path $MsvcRoot "bin\HostX86\$LibArch\link.exe")) {
            Join-Path $MsvcRoot "bin\HostX86\$LibArch"
        } else {
            $null
        }
        if (!$HostBin) {
            continue
        }

        return [pscustomobject] @{
            BinDir     = $HostBin
            ClExe      = Join-Path $HostBin "cl.exe"
            LibExe     = Join-Path $HostBin "lib.exe"
            LinkExe    = Join-Path $HostBin "link.exe"
            IncludeDir = $IncludeDir
            LibDir     = $LibDir
        }
    }

    $Component = switch ($Arch) {
        "arm64" { "Microsoft.VisualStudio.Component.VC.Tools.ARM64" }
        default { "Microsoft.VisualStudio.Component.VC.Tools.x86.x64" }
    }
    throw "Could not find MSVC include/lib/linker paths for $Arch. Install the Visual Studio C++ component $Component."
}

function Get-WindowsSdkPaths {
    param([string] $Arch)

    $LibArch = Get-MsvcLibArch $Arch
    $SdkRoot = Get-WindowsSdkRoot
    $Version = Get-WindowsSdkVersion
    $IncludeRoot = Join-Path $SdkRoot "Include\$Version"
    $LibRoot = Join-Path $SdkRoot "Lib\$Version"
    $IncludeDirs = @(
        (Join-Path $IncludeRoot "ucrt"),
        (Join-Path $IncludeRoot "shared"),
        (Join-Path $IncludeRoot "um"),
        (Join-Path $IncludeRoot "winrt"),
        (Join-Path $IncludeRoot "cppwinrt")
    ) | Where-Object { Test-Path $_ }
    $LibDirs = @(
        (Join-Path $LibRoot "ucrt\$LibArch"),
        (Join-Path $LibRoot "um\$LibArch")
    ) | Where-Object { Test-Path $_ }

    if ($IncludeDirs.Count -eq 0 -or $LibDirs.Count -eq 0) {
        throw "Could not find Windows SDK include/lib paths for $Arch."
    }

    [pscustomobject] @{
        IncludeDirs = $IncludeDirs
        LibDirs     = $LibDirs
    }
}

function Write-TargetLinkerWrapper {
    param(
        [string] $Arch,
        [object] $MsvcPaths,
        [object] $SdkPaths
    )

    $WrapperDir = Join-Path $Root "target\link-wrappers"
    New-Item -ItemType Directory -Force $WrapperDir | Out-Null
    $WrapperPath = Join-Path $WrapperDir "link-$Arch.cmd"
    $LibPath = Join-ExistingPathList (@($MsvcPaths.LibDir) + @($SdkPaths.LibDirs))
    $IncludePath = Join-ExistingPathList (@($MsvcPaths.IncludeDir) + @($SdkPaths.IncludeDirs))

    @(
        "@echo off",
        "set ""LIB=$LibPath;%LIB%""",
        "set ""INCLUDE=$IncludePath;%INCLUDE%""",
        """$($MsvcPaths.LinkExe)"" %*"
    ) | Set-Content -Path $WrapperPath -Encoding ASCII

    $WrapperPath
}

$Target = Get-RustTarget $Arch
rustup target add $Target

$HostArch = if ([Environment]::Is64BitProcess) { "x64" } else { "x86" }
$HostMsvc = Get-MsvcPaths $HostArch
$HostSdk = Get-WindowsSdkPaths $HostArch
$TargetMsvc = Get-MsvcPaths $Arch
$TargetSdk = Get-WindowsSdkPaths $Arch

Prepend-EnvPath "PATH" @($HostMsvc.BinDir, $TargetMsvc.BinDir)
Prepend-EnvPath "INCLUDE" (@($TargetMsvc.IncludeDir, $HostMsvc.IncludeDir) + @($TargetSdk.IncludeDirs) + @($HostSdk.IncludeDirs))
Prepend-EnvPath "LIB" (@($HostMsvc.LibDir) + @($HostSdk.LibDirs))

$LinkerWrapper = Write-TargetLinkerWrapper -Arch $Arch -MsvcPaths $TargetMsvc -SdkPaths $TargetSdk
Set-CargoTargetEnv $Target "LINKER" $LinkerWrapper
Set-CargoTargetEnv $Target "RUSTFLAGS" "-C target-feature=+crt-static"
Set-TargetToolEnv $Target "CC" $TargetMsvc.ClExe
Set-TargetToolEnv $Target "AR" $TargetMsvc.LibExe

cargo fmt --check
cargo build --release --target $Target --bin phase-tool
if (Test-CanRunBuiltExe $Arch) {
    & (Join-Path $Root "target\$Target\release\phase-tool.exe") --smoke-test
} else {
    Write-Warning "Skipping $Arch phase-tool smoke test because this machine cannot run $Arch Windows executables."
}

$OutDir = Join-Path $Root "dist\windows\$Arch"
New-Item -ItemType Directory -Force $OutDir | Out-Null
Copy-Item -Force (Join-Path $Root "target\$Target\release\phase-tool.exe") (Join-Path $OutDir "PhaseAnimatorInstaller.exe")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.png") (Join-Path $OutDir "PhaseAnimator.png")
Copy-Item -Force (Join-Path $Root "assets\PhaseAnimator.ico") (Join-Path $OutDir "PhaseAnimator.ico")

if ($Arch -eq "x64") {
    $LegacyOutDir = Join-Path $Root "dist\windows"
    Copy-Item -Force (Join-Path $OutDir "PhaseAnimatorInstaller.exe") (Join-Path $LegacyOutDir "PhaseAnimatorInstaller.exe")
    Copy-Item -Force (Join-Path $OutDir "PhaseAnimator.png") (Join-Path $LegacyOutDir "PhaseAnimator.png")
    Copy-Item -Force (Join-Path $OutDir "PhaseAnimator.ico") (Join-Path $LegacyOutDir "PhaseAnimator.ico")
}

Write-Host "Built dist\windows\$Arch\PhaseAnimatorInstaller.exe"
