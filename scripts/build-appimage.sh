#!/usr/bin/env bash
# Builds a self-contained AppImage for PEAR Configurator.
#
# It bundles the GUI runtime libraries and, when present, the Apple
# idevicerestore/irecovery toolchain, so the resulting .AppImage can be run on
# any Linux desktop (x86_64 or aarch64) without installing dependencies.
#
# Usage: scripts/build-appimage.sh
# Output: dist/PEAR-Configurator-<arch>.AppImage
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCH="${ARCH:-$(uname -m)}"
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
    curl --fail --location --retry 3 -o "$dest" "$url"
    chmod +x "$dest"
  fi
}

fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage" "$LD"
fetch "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage" "$AT"

echo "==> building release binary"
cargo build --release --locked

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
echo "==> bundling Apple toolchain from $TC"
for tool in idevicerestore irecovery ideviceinfo ideviceenterrecovery; do
  if [ ! -x "$TC/bin/$tool" ]; then
    echo "ERROR: $tool missing. Run scripts/install-toolchain.sh first." >&2
    exit 1
  fi
  cp -a "$TC/bin/$tool" "$APPDIR/usr/bin/"
  EXE_ARGS+=(--executable "$APPDIR/usr/bin/$tool")
done
cp -a "$TC/lib/." "$APPDIR/usr/lib/"

# Custom AppRun: put bundled tools/libs on PATH and LD_LIBRARY_PATH.
cat > "$TOOLS/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
[ -x "$HERE/usr/bin/idevicerestore" ] && export PEAR_IDEVICERESTORE="$HERE/usr/bin/idevicerestore"
[ -x "$HERE/usr/bin/irecovery" ] && export PEAR_IRECOVERY="$HERE/usr/bin/irecovery"
[ -x "$HERE/usr/bin/ideviceinfo" ] && export PEAR_IDEVICEINFO="$HERE/usr/bin/ideviceinfo"
[ -x "$HERE/usr/bin/ideviceenterrecovery" ] && export PEAR_IDEVICEENTERRECOVERY="$HERE/usr/bin/ideviceenterrecovery"
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

# Copied toolchains can carry owner-only directory modes from prior extraction.
chmod -R a+rX "$APPDIR"

echo "==> packaging AppImage"
"$AT" "$APPDIR" "$OUT/PEAR-Configurator-$ARCH.AppImage"

echo "==> done"
ls -la "$OUT"
