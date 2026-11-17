#!/bin/bash
# Builds build/ClipRust.app: Rust core (staticlib) -> Swift app -> bundle -> codesign.
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$APP_DIR/.." && pwd)"
IDENTITY="${CLIPRUST_SIGN_IDENTITY:-Apple Development: Felix Wegener (6NH6KBTA54)}"
VERSION="0.1.0"
OUT="$APP_DIR/build/ClipRust.app"

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

echo "==> Rust core"
export MACOSX_DEPLOYMENT_TARGET=26.0
(cd "$ROOT" && cargo build --release --features ffi --lib)

echo "==> Swift app"
(cd "$APP_DIR" && swift build -c release --product ClipRust)
BIN="$(cd "$APP_DIR" && swift build -c release --show-bin-path)/ClipRust"

echo "==> Bundle"
rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp "$BIN" "$OUT/Contents/MacOS/ClipRust"

ICONSET="$APP_DIR/build/AppIcon.iconset"
rm -rf "$ICONSET"
if "$BIN" --render-icon "$ICONSET" && iconutil -c icns "$ICONSET" -o "$OUT/Contents/Resources/AppIcon.icns"; then
    rm -rf "$ICONSET"
else
    echo "warning: app icon not generated" >&2
fi

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>app.cliprust.ClipRust</string>
    <key>CFBundleName</key><string>ClipRust</string>
    <key>CFBundleDisplayName</key><string>ClipRust</string>
    <key>CFBundleExecutable</key><string>ClipRust</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>LSUIElement</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSAppleEventsUsageDescription</key><string>ClipRust reads the address of the current browser page when you copy an API key, so you can tell keys apart later.</string>
    <key>NSHumanReadableCopyright</key><string>Felix Wegener</string>
</dict>
</plist>
PLIST

echo "==> Sign"
if security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$IDENTITY\"" \
    && codesign --force --timestamp=none --sign "$IDENTITY" "$OUT" 2>"$APP_DIR/build/codesign.log"; then
    echo "Signed with $IDENTITY"
else
    echo "warning: signing with \"$IDENTITY\" failed; using an ad-hoc signature." >&2
    echo "warning: Accessibility permission will have to be granted again after each rebuild." >&2
    codesign --force --sign - "$OUT"
fi
codesign --verify --strict "$OUT"
echo "Built $OUT"
