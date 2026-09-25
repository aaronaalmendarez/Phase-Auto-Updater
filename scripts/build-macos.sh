#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo fmt --check
cargo build --release --bin phase-tool
PACKAGE_VERSION="$(awk -F\" '/^version =/ { print $2; exit }' Cargo.toml)"
APP_NAME="Phase Companion"
BUNDLE_ID="xyz.motioncore.phase.companion"

OUTPUT_DIR="$ROOT/dist/macos"
FINAL_APP_DIR="$OUTPUT_DIR/$APP_NAME.app"
PACKAGE_WORK="$(mktemp -d "${TMPDIR:-/tmp}/phase-companion-package.XXXXXX")"
ICON_WORK="$(mktemp -d "${TMPDIR:-/tmp}/phase-app-icon.XXXXXX")"
DMG_STAGE=""
cleanup() {
  rm -rf "$PACKAGE_WORK" "$ICON_WORK"
  if [[ -n "$DMG_STAGE" ]]; then
    rm -rf "$DMG_STAGE"
  fi
}
trap cleanup EXIT

APP_DIR="$PACKAGE_WORK/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"
ICON_SOURCE="$ROOT/assets/PhaseAnimator.png"
DMG_PATH="$OUTPUT_DIR/$APP_NAME.dmg"

rm -rf "$FINAL_APP_DIR" "$OUTPUT_DIR/Phase Animator Installer.app"
mkdir -p "$OUTPUT_DIR"
mkdir -p "$MACOS" "$RESOURCES"
cp "$ROOT/target/release/phase-tool" "$MACOS/$APP_NAME"
cp "$ICON_SOURCE" "$RESOURCES/PhaseAnimator.png"

ICONSET="$ICON_WORK/AppIcon.iconset"
mkdir -p "$ICONSET"

make_icon() {
  local size="$1"
  local name="$2"
  sips -z "$size" "$size" "$ICON_SOURCE" --out "$ICONSET/$name" >/dev/null
}

make_icon 16 icon_16x16.png
make_icon 32 icon_16x16@2x.png
make_icon 32 icon_32x32.png
make_icon 64 icon_32x32@2x.png
make_icon 128 icon_128x128.png
make_icon 256 icon_128x128@2x.png
make_icon 256 icon_256x256.png
make_icon 512 icon_256x256@2x.png
make_icon 512 icon_512x512.png
make_icon 1024 icon_512x512@2x.png
ICON_PLIST=""
if iconutil -c icns "$ICONSET" -o "$RESOURCES/AppIcon.icns"; then
  ICON_PLIST=$'  <key>CFBundleIconFile</key>\n  <string>AppIcon.icns</string>'
else
  echo "warning: iconutil rejected the generated iconset; continuing without CFBundleIconFile" >&2
  rm -f "$RESOURCES/AppIcon.icns"
fi

cat > "$CONTENTS/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
${ICON_PLIST}
  <key>CFBundleShortVersionString</key>
  <string>${PACKAGE_VERSION}</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

# Finder metadata and resource forks copied from source assets are not valid
# inside a signed app bundle.
xattr -cr "$APP_DIR"

if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$APP_DIR"
  echo "Signed $APP_NAME.app with $MACOS_SIGN_IDENTITY"
else
  # A sealed ad-hoc signature keeps local builds structurally valid. Public
  # releases still need a Developer ID Application identity and notarization.
  codesign --force --sign - "$APP_DIR"
  echo "Ad-hoc signed $APP_NAME.app for local use"
fi

DMG_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/phase-companion-dmg.XXXXXX")"
cp -R "$APP_DIR" "$DMG_STAGE/"
ln -s /Applications "$DMG_STAGE/Applications"
xattr -cr "$DMG_STAGE/$APP_NAME.app"
rm -f "$DMG_PATH"
hdiutil create -quiet -volname "$APP_NAME" -srcfolder "$DMG_STAGE" -ov -format UDZO "$DMG_PATH"

if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$DMG_PATH"
fi

# Copy the already-signed bundle into the workspace only after packaging.
# Cloud-backed Desktop folders can attach Finder metadata immediately, which
# otherwise races with codesign during local builds.
ditto --norsrc "$APP_DIR" "$FINAL_APP_DIR"

echo "Built dist/macos/$APP_NAME.app"
echo "Built dist/macos/$APP_NAME.dmg"
