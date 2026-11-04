Multi-device restore queues, normal-mode iPhone/iPad detection, and targeted Recovery controls.

## What's new

- List multiple connected devices with individual model, mode, and ECID information.
- Queue a separate firmware version and erase/update choice for each device. Restores run sequentially, with per-device results; a failed entry does not stop the remaining entries.
- Freeze device targets and firmware choices before confirmation. Each restore explicitly targets an ECID, and disconnected targets are never replaced with another connected device.
- Cancel the active operation and skip the remaining queued devices.
- Detect normal-mode iPhone/iPad devices and provide an **Enter Recovery** button. Direct DFU entry still requires the device's physical button sequence.
- Correct the `irecovery` ECID query option and use reported product identifiers to identify newer recovery devices.
- Retain the refreshed restore workspace and Linux USB access fixes from v0.2.2.

## Download and run

This locally built release includes an **x86_64 AppImage** and its SHA-256 checksum file. It requires **glibc 2.43 or newer**, plus a Wayland or X11 desktop. An ARM64 package is not included.

```sh
sha256sum -c SHA256SUMS-v0.2.4.txt
chmod +x PEAR-Configurator-v0.2.4-x86_64.AppImage
./PEAR-Configurator-v0.2.4-x86_64.AppImage
```

The package bundles `idevicerestore`, `irecovery`, `ideviceinfo`, and `ideviceenterrecovery`. Normal-mode controls require the host's `usbmuxd` service; unlock the device and accept Trust if requested. Complete the one-time USB permission setup on launch if prompted.

To queue several restores, enable **Queue for multiple devices**, select a device and firmware, and choose **Queue Erase** or **Queue Keep Data**. Repeat for other devices, then open **Queue → Start queue** and review the complete confirmation list.

**Erase & Install permanently removes device data.** Keep Data is not a guarantee of data preservation. Queue contents are held in memory for the current app session.

## Validation

- 41 unit tests passed, covering device selection, ECID targeting, firmware mismatch rejection, disconnect handling, cancellation, and queue failure handling.
- UI previews checked with sample devices and queue entries.
- The packaged application's detection, identification, and firmware lookup were verified with a connected iPhone 16e in DFU mode.
- Physical restore and multi-device restoration have not been verified on hardware.
