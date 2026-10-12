#!/usr/bin/env bash
# Builds a self-contained AppImage for PEAR Configurator.
#
# It bundles the GUI runtime libraries and, when present, the Apple
# idevicerestore/irecovery toolchain, so the resulting .AppImage can be run on
# any x86_64 Linux desktop without installing dependencies.
#
# Usage: scripts/build-appimage.sh
# Output: dist/PEAR-Configurator-x86_64.AppImage
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCH=x86_64
TOOLS="$ROOT/.appimage-tools"
OUT="$ROOT/dist"
APPDIR="$ROOT/.appdir"
LD="$TOOLS/linuxdeploy-$ARCH.AppImage"
AT="$TOOLS/appimagetool-$ARCH.AppImage"

export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH
export PATH="$HOME/.cargo/bin:$PATH"

mkdir -p "$TOOLS" "$OUT"

fetch() {
  local url="$1" dest="$2"
  if [ ! -f "$dest" ]; then
    echo "==> downloading $(basename "$dest")"
    curl -sSL -o "$dest" "$url"
    chmod +x "$dest"
  fi
}

fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage" "$LD"
fetch "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage" "$AT"

echo "==> building release binary"
cargo build --release

echo "==> assembling AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp target/release/pear-configurator "$APPDIR/usr/bin/"
python3 scripts/make-icon.py "$APPDIR/usr/share/icons/hicolor/256x256/apps/pear-configurator.png"

cat > "$APPDIR/usr/share/applications/pear-configurator.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=PEAR Configurator
Comment=One-click restore tool for Apple devices
Exec=pear-configurator
Icon=pear-configurator
Categories=Utility;System;
Terminal=false
EOF

# Bundle the Apple toolchain when available.
EXE_ARGS=(--executable "$APPDIR/usr/bin/pear-configurator")
TC="$HOME/.pear/toolchain"
if [ -x "$TC/bin/idevicerestore" ] && [ -x "$TC/bin/irecovery" ]; then
  echo "==> bundling Apple toolchain from $TC"
  cp -a "$TC/bin/idevicerestore" "$TC/bin/irecovery" "$APPDIR/usr/bin/"
  cp -a "$TC/lib/." "$APPDIR/usr/lib/" 2>/dev/null || true
  EXE_ARGS+=(--executable "$APPDIR/usr/bin/idevicerestore" --executable "$APPDIR/usr/bin/irecovery")
else
  echo "!! idevicerestore toolchain not found; the AppImage will lack restore support"
fi

# Custom AppRun: put bundled tools/libs on PATH and LD_LIBRARY_PATH.
cat > "$TOOLS/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
[ -x "$HERE/usr/bin/idevicerestore" ] && export PEAR_IDEVICERESTORE="$HERE/usr/bin/idevicerestore"
[ -x "$HERE/usr/bin/irecovery" ] && export PEAR_IRECOVERY="$HERE/usr/bin/irecovery"
exec "$HERE/usr/bin/pear-configurator" "$@"
EOF
chmod +x "$TOOLS/AppRun"

echo "==> gathering shared libraries (linuxdeploy)"
LIB_ARGS=()
while IFS= read -r so; do
  LIB_ARGS+=(--library "$so")
done < <(find "$APPDIR/usr/lib" -maxdepth 1 -name '*.so*' 2>/dev/null)

LD_LIBRARY_PATH="$APPDIR/usr/lib" "$LD" --appdir "$APPDIR" \
  "${EXE_ARGS[@]}" \
  --desktop-file "$APPDIR/usr/share/applications/pear-configurator.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/pear-configurator.png" \
  --icon-filename pear-configurator \
  --custom-apprun "$TOOLS/AppRun" \
  "${LIB_ARGS[@]}"

chmod +x "$APPDIR/AppRun" 2>/dev/null || true

echo "==> packaging AppImage"
"$AT" "$APPDIR" "$OUT/PEAR-Configurator-$ARCH.AppImage"

echo "==> done"
ls -la "$OUT"
