# PEAR Configurator

A desktop app that restores Apple devices with a single click while they are in
**DFU** or **recovery** mode.

- **Built with:** Rust + [Slint](https://slint.dev) (native desktop UI)
- **Platform:** Linux, x86_64 and arm64 (Wayland / X11)
- **Supported devices:** iPhone, iPad, iPod, Apple TV and **Apple Silicon Mac**

Inspired by libimobiledevice and restorekit.

## What it does

1. **Detects** an attached Apple device and its mode (DFU / recovery) instantly.
2. **Identifies** the exact model automatically (e.g. Mac mini M1 → `Macmini9,1`).
3. **Finds** the **signed** firmware versions for that device from ipsw.me.
4. **Downloads** the selected IPSW if missing (resumes + verifies SHA-256).
5. **Restores** it with `idevicerestore`, showing live progress.

Two restore modes are available:

| Button | Effect |
| --- | --- |
| **Erase & Install** | Erases all data and installs from scratch (`-e`) |
| **Keep Data** | Update / revive (does not erase user data) |

## Requirements

- A Linux desktop (Wayland or X11)
- Either the prebuilt **AppImage** (no dependencies), or the following to build
  from source:
  - Rust (installable via rustup)
  - System libraries: `libfontconfig1-dev`, `libfreetype-dev`
- `idevicerestore` and `irecovery` (installed by the script below, or bundled in
  the AppImage)
- A udev rule for USB access to DFU devices

## Quick start (AppImage)

Download the AppImage for your architecture (`x86_64` or `aarch64`) from the
[Releases](https://github.com/denizomer10/pear-configurator/releases) page:

```sh
chmod +x PEAR-Configurator-<arch>.AppImage
./PEAR-Configurator-<arch>.AppImage
```

The AppImage bundles all runtime libraries and the `idevicerestore` toolchain,
so nothing needs to be installed on the system (a desktop with Wayland or X11 is
still required). The udev rule below is still needed for USB access.

## Installation

### 1) Build the app

```sh
sudo apt-get install -y build-essential pkg-config libfontconfig1-dev libfreetype-dev
cargo build --release
```

### 2) Install the Apple toolchain

```sh
scripts/install-toolchain.sh --deps   # build dependencies via apt (needs sudo)
scripts/install-toolchain.sh          # builds idevicerestore + irecovery
```

The tools are installed under `~/.pear/toolchain` (no root required).

### 3) USB access rule (udev)

```sh
sudo install -m 644 scripts/99-pear-apple.rules /etc/udev/rules.d/99-pear-apple.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

The user must be in the `plugdev` group.

### 4) (Optional) Build a self-contained AppImage

```sh
scripts/build-appimage.sh   # -> dist/PEAR-Configurator-x86_64.AppImage
```

It uses `linuxdeploy` and `appimagetool` (downloaded automatically) and bundles
the runtime libraries plus the Apple toolchain into a single file. The build is
architecture-aware: it produces `PEAR-Configurator-x86_64.AppImage` on x86_64
and `PEAR-Configurator-aarch64.AppImage` on aarch64.

Prebuilt AppImages for both architectures are produced by CI
(`.github/workflows/build-appimage.yml`) on every `v*` tag.

## Running

```sh
./target/release/pear-configurator
```

If the window does not open (GPU-less environment), force the software renderer:

```sh
SLINT_BACKEND=winit-software ./target/release/pear-configurator
```

Extra commands:

```sh
./target/release/pear-configurator --selftest   # detection + model + firmware test (no GUI)
./target/release/pear-configurator --download   # pre-download and verify the IPSW
```

## Usage

1. Connect the device and put it into **DFU** or **recovery** mode.
2. The app automatically shows the device, its mode and model.
3. Pick a version from the **Version (IPSW)** list (signed ones are recommended).
4. Click **Erase & Install** (or **Keep Data**).
5. If needed the IPSW is downloaded, then restore progress is shown live.

## File locations

| What | Where |
| --- | --- |
| Toolchain | `~/.pear/toolchain` |
| Downloaded IPSW | `~/.pear/ipsw` |
| Cache / database | `~/.pear/cache`, `~/.pear/data` |
| Logs | `~/.pear/logs` |

Override with the `PEAR_HOME` and `PEAR_DOWNLOAD_DIR` environment variables.

## Tests

```sh
cargo test
```

## Warnings

- **Erase & Install cannot be undone**; all data on the device is erased.
- Apple Silicon Mac restore requires a recent `idevicerestore` (hence the tools
  are built from source).
- If `irecovery` is missing, the device is still identified over USB (CPID/BDID).

## License

**MIT** — see [LICENSE](LICENSE).
