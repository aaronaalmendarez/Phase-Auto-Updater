#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo fmt --check
cargo build --release --bin phase-tool
PACKAGE_VERSION="$(awk -F\" '/^version =/ { print $2; exit }' Cargo.toml)"

APP_DIR="$ROOT/dist/macos/Phase Animator Installer.app"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS" "$RESOURCES"
cp "$ROOT/target/release/phase-tool" "$MACOS/Phase Animator Installer"
cp "$ROOT/assets/PhaseAnimator.png" "$RESOURCES/PhaseAnimator.png"

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>Phase Animator Installer</string>
  <key>CFBundleIdentifier</key>
  <string>xyz.motioncore.phase.installer</string>
  <key>CFBundleName</key>
  <string>Phase Animator Installer</string>
  <key>CFBundleDisplayName</key>
  <string>Phase Animator Installer</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${PACKAGE_VERSION}</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

echo "Built dist/macos/Phase Animator Installer.app"
