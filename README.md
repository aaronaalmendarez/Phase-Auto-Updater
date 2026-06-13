# Phase Companion

<p align="center">
  <img src="assets/PhaseAnimator.png" alt="Phase Animator logo" width="96">
</p>

Native companion app for the Phase Animator Roblox Studio plugin.

This is the public client app. It is written in Rust with egui so it can feel close to the plugin without needing a browser wrapper.

The companion connects to Phase, checks for the latest release, installs the plugin into the local Roblox Studio plugins folder, runs local video-reference sync, and exposes a local acceleration bridge for future high-pressure plugin workloads.

<p align="center">
  <img src="assets/ScreenshotPhase.png" alt="Phase Auto Updater app screenshot" width="456">
</p>

## what it does

- detects the Roblox Studio plugin folder on Windows and macOS
- lets you pick a folder manually when Roblox uses a different local path
- connects a Phase account
- supports Roblox OAuth verification
- supports license key activation
- can apply public Phase marketplace themes to the installer UI
- opens a Video Reference tab for syncing Phase Animator to YouTube or local video files
- runs a local `ws://127.0.0.1:27731/phase-video-reference` bridge for Studio timeline/playback sync
- runs a local `ws://127.0.0.1:27730/phase-companion` bridge for Studio companion detection and future native acceleration
- shows a Boost Doctor snapshot in the Video tab so Studio can surface which bottlenecks deserve companion acceleration first
- advertises planned native boost capabilities such as IK playback prebake, timeline indexing, local cache, and compact payload batching
- renders local MP4/MOV/M4V/WebM files in a native popup with custom controls, volume, scrubbing, rate, loop, fullscreen, swap video, and show-folder actions
- embeds YouTube with the native YouTube player while still listening for play, pause, seek, and rate sync events
- watches for new updater events and sends a desktop notification
- downloads the plugin `.rbxm`
- checks the file hash before replacing local files
- makes a backup of the existing plugin file first
- checks GitHub Releases for newer installer builds

The UI is compact and shaped like a small installer. Long account names, file paths, and video references use horizontal scrolling so they do not break the layout.

## building it

Windows app only:

```powershell
.\scripts\build-windows.ps1 -Arch x64
```

Windows MSI:

```powershell
.\scripts\build-msi.ps1 -Arch x64
.\scripts\build-msi.ps1 -Arch x86
.\scripts\build-msi.ps1 -Arch arm64
```

This creates architecture-specific packages:

- `dist/windows/x64/PhaseAnimatorSetup-x64.exe`
- `dist/windows/x86/PhaseAnimatorSetup-x86.exe`
- `dist/windows/arm64/PhaseAnimatorSetup-arm64.exe`
- matching `PhaseAutoUpdater-<version>-<arch>.msi` files

`PhaseAnimatorSetup-x64.exe` is for normal 64-bit Intel/AMD Windows. `PhaseAnimatorSetup-x86.exe` is for 32-bit Windows. `PhaseAnimatorSetup-arm64.exe` is for native Windows on ARM. The setup app carries the matching MSI inside it, shows the normal installer wizard, installs per-user, creates desktop/start menu shortcuts, enables startup, and launches Phase Companion when setup finishes.
Running the same setup again repairs or reinstalls the app.

Local ARM64 builds require the Visual Studio C++ ARM64 tools component (`Microsoft.VisualStudio.Component.VC.Tools.ARM64`). GitHub-hosted Windows runners include the required ARM64 C++ toolchain.

```powershell
.\dist\windows\x64\PhaseAnimatorSetup-x64.exe
```

macOS:

```bash
bash ./scripts/build-macos.sh
```

For just running locally:

```bash
cargo run --bin phase-tool
```

For video popup debugging:

```bash
cargo run --bin phase-tool -- --video-popup <path-to-rendered-player-html>
```

## github builds

There are GitHub Actions in `.github/workflows`:

- `build.yml` builds Windows x64, x86, arm64, and macOS on pushes / PRs and uploads artifacts.
- `release.yml` builds `PhaseAnimatorSetup-x64.exe`, `PhaseAnimatorSetup-x86.exe`, `PhaseAnimatorSetup-arm64.exe`, matching Windows MSIs, and the macOS zip when a tag like `v0.20.0` is pushed.

When a GitHub Release has a newer `PhaseAutoUpdater-*-<arch>.msi` asset, the app shows it in the Options tab and launches the installer update matching the current Windows architecture.

macOS still needs signing and notarization before a public customer release. The app bundle script is here, but the Apple packaging pass still needs to happen on macOS or CI.

## latest update

`0.20.1` fixes marketplace theme application for the current PA2 JSON theme-code format.

`0.20.0` adds architecture-specific Windows setup packages for x64, x86, and arm64 so customers on 32-bit Windows or Windows on ARM do not receive an incompatible setup executable.

## repo notes

`dev-notes.md` tracks a few release and maintenance notes that should stay visible while this is being prepared for public use.
