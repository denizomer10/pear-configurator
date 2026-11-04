DFU USB access fixes and a refreshed restore workspace.

- Fix Linux USB permissions by installing `70-pear-apple.rules` before systemd applies seat ACLs. Existing users will be prompted to install the corrected rule on launch; reconnect the cable if needed.
- Pause device queries during restore to prevent competing USB access.
- Read both restore output streams concurrently, display diagnostics in Activity, and allow cancellation while the tool is silent.
- Refresh the UI with a green theme, device sidebar, firmware selection and a dedicated restore status panel. Preserve the erase confirmation dialog.
- Fix unprivileged toolchain builds by skipping the redundant upstream udev rule installation.
- Bundle idevicerestore and irecovery in native x86_64 and ARM64 AppImages. Include SHA-256 checksums.

Validation: 29 unit tests and hardware-free UI rendering at default and compact sizes. Physical DFU restore has not been verified on hardware.

Download the AppImage matching your CPU, make it executable, and launch it. The x86_64 build targets Ubuntu 22.04 or newer; the ARM64 build targets Ubuntu 24.04 or newer (or distributions with compatible runtime libraries).
