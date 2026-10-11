#!/bin/sh
# Build glide, assemble dist/Glide.app with its icon, and code-sign it with a
# stable identity so macOS privacy grants (screen, mic, camera, speech)
# survive rebuilds. Usage: scripts/build.sh   (PROFILE=debug for a debug build)
set -eu
cd "$(dirname "$0")/.."
PROFILE="${PROFILE:-release}"
. scripts/env.sh
if [ "$PROFILE" = release ]; then cargo build --release; else cargo build; fi
BIN="$CARGO_TARGET_DIR/$PROFILE/glide"
APP=dist/Glide.app
STAGE=dist/.Glide.app.new
# Build the bundle beside the old one and swap it in, so a running copy keeps
# its executable until it quits.
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"
cp support/Info.plist "$STAGE/Contents/Info.plist"
cp "$BIN" "$STAGE/Contents/MacOS/glide"
ICONSET="$CARGO_TARGET_DIR/AppIcon.iconset"
rm -rf "$ICONSET"
"$BIN" icon "$ICONSET" >/dev/null
iconutil -c icns -o "$STAGE/Contents/Resources/AppIcon.icns" "$ICONSET"
IDENTITY="${GLIDE_SIGN_IDENTITY:-$(security find-identity -v -p codesigning | awk -F'"' '/Apple Development|Developer ID Application/ {print $2; exit}')}"
if [ -z "$IDENTITY" ]; then
  echo "warning: no signing identity found; ad-hoc signing (permissions reset on every rebuild)" >&2
  IDENTITY=-
fi
codesign --force --sign "$IDENTITY" --identifier com.heyfabrika.glide "$STAGE"
codesign --verify --strict "$STAGE"
rm -rf "$APP.old"
[ -d "$APP" ] && mv "$APP" "$APP.old"
mv "$STAGE" "$APP"
rm -rf "$APP.old"
codesign --verify --strict "$APP"
ln -sf "Glide.app/Contents/MacOS/glide" dist/glide
echo "built $APP (signed: $IDENTITY)"
echo "open:  open $APP      cli: dist/glide --help"
