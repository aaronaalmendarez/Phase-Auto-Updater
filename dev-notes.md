# Dev Notes

Items to review before tagging a public release:

- macOS app bundle support is intentionally minimal right now. It builds an `.app`, but signing and notarization still need to happen on macOS or CI.
- Windows builds produce a one-click setup executable plus the raw per-user MSI. The setup executable enables startup and launches the app after install.
- Roblox local plugin paths are best-effort. The manual folder picker is staying because Studio path behavior has changed before.
- The installer should keep using stable file names like `PhaseAnimator.rbxm`; changing that later will make existing local installs harder to detect.
- Keep the client focused on install/update UX and avoid mixing release hosting details into the desktop app.
- If the UI starts getting more tabs, split `main.rs`. It is still one file mostly because the first pass was easier to tune visually that way.
