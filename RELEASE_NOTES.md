DFU USB access fixes and a refreshed restore workspace.

- Fix Linux USB permissions by installing `70-pear-apple.rules` before systemd applies seat ACLs. Existing users will be prompted to install the corrected rule on launch; reconnect the cable if needed.
- Pause device queries during restore to prevent competing USB access.
- Read both restore output streams concurrently, display diagnostics in Activity, and allow cancellation while the tool is silent.
- Refresh the UI with a green theme, device sidebar, firmware selection and a dedicated restore status panel. Preserve the erase confirmation dialog.
- Fix unprivileged toolchain builds by skipping the redundant upstream udev rule installation.
- Locally built x86_64 AppImage with idevicerestore and irecovery reused from the v0.2.0 release. Include SHA-256 checksums.

Validation: 29 unit tests and hardware-free UI rendering at default and compact sizes. Physical DFU restore has not been verified on hardware.

Download the x86_64 AppImage, make it executable, and launch it. This release was built locally on Ubuntu 26.04 and requires glibc 2.43 or newer. ARM64 is not included in this local build.
