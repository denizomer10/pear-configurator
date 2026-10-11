#!/usr/bin/env bash
# Builds the libimobiledevice toolchain (idevicerestore + irecovery)
# into ~/.pear/toolchain, without needing root.
#
# Usage:
#   scripts/install-toolchain.sh --deps   # install build dependencies via apt (needs sudo)
#   scripts/install-toolchain.sh          # build the tools (deps must be present)
#
# Environment variables:
#   PEAR_HOME   (default: ~/.pear)
#   JOBS        (default: nproc)
set -euo pipefail

PEAR_HOME="${PEAR_HOME:-$HOME/.pear}"
PREFIX="$PEAR_HOME/toolchain"
SRC="$PEAR_HOME/src"
JOBS="${JOBS:-$(nproc 2>/dev/null || echo 2)}"
INSTALL_DEPS=0

for arg in "$@"; do
  case "$arg" in
    --deps) INSTALL_DEPS=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
  esac
done

APT_PACKAGES=(
  build-essential autoconf automake libtool pkg-config git curl
  libusb-1.0-0-dev libssl-dev zlib1g-dev libcurl4-openssl-dev libzip-dev
)

log() { printf '\033[1;34m[pear]\033[0m %s\n' "$*"; }
err() { printf '\033[1;31m[pear]\033[0m %s\n' "$*" >&2; }

if [[ "$INSTALL_DEPS" == "1" ]]; then
  log "Installing build dependencies via apt (may require sudo)..."
  sudo apt-get update
  sudo apt-get install -y "${APT_PACKAGES[@]}"
fi

for tool in git make pkg-config; do
  command -v "$tool" >/dev/null 2>&1 || { err "'$tool' not found. Run with --deps first."; exit 1; }
done
if ! pkg-config --exists libusb-1.0 2>/dev/null; then
  err "libusb-1.0 development headers not found. Install them with --deps."
  exit 1
fi

mkdir -p "$PREFIX" "$SRC"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export PATH="$PREFIX/bin:$PATH"
export LDFLAGS="-L$PREFIX/lib ${LDFLAGS:-}"
export CPPFLAGS="-I$PREFIX/include ${CPPFLAGS:-}"

# name|github-url|branch
REPOS=(
  "libplist|https://github.com/libimobiledevice/libplist.git|master"
  "libimobiledevice-glue|https://github.com/libimobiledevice/libimobiledevice-glue.git|master"
  "libusbmuxd|https://github.com/libimobiledevice/libusbmuxd.git|master"
  "libtatsu|https://github.com/libimobiledevice/libtatsu.git|master"
  "libirecovery|https://github.com/libimobiledevice/libirecovery.git|master"
  "libimobiledevice|https://github.com/libimobiledevice/libimobiledevice.git|master"
  "idevicerestore|https://github.com/libimobiledevice/idevicerestore.git|master"
)

build_repo() {
  local name="$1" url="$2" branch="$3"
  local dir="$SRC/$name"
  log "==> $name"
  if [[ -d "$dir/.git" ]]; then
    git -C "$dir" fetch --depth 1 origin "$branch" >/dev/null 2>&1 || true
    git -C "$dir" reset --hard "origin/$branch" >/dev/null 2>&1 || true
  else
    git clone --depth 1 --branch "$branch" "$url" "$dir"
  fi
  ( cd "$dir"
    if [[ -x ./autogen.sh ]]; then
      ./autogen.sh --prefix="$PREFIX" --disable-static
    elif [[ -x ./configure ]]; then
      ./configure --prefix="$PREFIX" --disable-static
    else
      err "$name has no build script"; exit 1
    fi
    make -j"$JOBS"
    make install
  )
}

for entry in "${REPOS[@]}"; do
  IFS='|' read -r name url branch <<< "$entry"
  build_repo "$name" "$url" "$branch"
done

# Environment file so libraries are found without running ldconfig.
cat > "$PREFIX/env.sh" <<EOF
# PE Configurator toolchain environment
export PATH="$PREFIX/bin:\$PATH"
export LD_LIBRARY_PATH="$PREFIX/lib:\${LD_LIBRARY_PATH:-}"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig:\${PKG_CONFIG_PATH:-}"
EOF

log "Done. Tools are in: $PREFIX/bin"
"$PREFIX/bin/idevicerestore" --version 2>/dev/null || true
log "PE Configurator will use these tools automatically on startup."
