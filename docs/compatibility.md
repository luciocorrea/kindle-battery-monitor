# Compatibility and device qualification

The installer accepts only this qualified target:

| Field | Value |
| --- | --- |
| Kindle | Paperwhite 4 |
| Firmware | 5.16.5 |
| Device type | `AJRLVDTOPT1LE` |
| Exact build line | `System Software Version: 042-juno_1605_moonshine_rex-414285` |
| Architecture | `arm-unknown-linux-musleabihf`, static executable |
| Service manager | Upstart 0.6.6 |

The type identifier is shared hardware metadata, not a serial number. The installer also checks stock service files and Bluetooth programs. BSA's wire layout is firmware-specific; another ARM Kindle is not qualified merely because its architecture matches.

The original 0.1.1 runtime passed bounded Home Assistant reception with battery and Complete Local Name in the same primary advertisement, public-address stability, radio cleanup, and a state-preserving KUAL update. The controller previously passed all seven actions, enabled/disabled boot, uninstall/reinstall, and visible KUAL status. The same scheduler/controller design retained a monitor process through about 36 minutes of memory suspend, with power-button wake and zero suspend failures in that trial. This is bounded device evidence, not a long-duration battery-life measurement.

A restart with USB connected was followed by a blank KUAL screen during qualification. Restarting with USB unplugged restored KUAL and SSH without changing package files. The cause was not established. Restart disconnected and reconnect after Home/KUAL loads on this tested setup.

The standalone extraction preserves runtime Rust sources, controller, and boot template. Its qualification verifies source isolation, host tests, static ARM build, and packaging. It does not extend the support matrix or repeat physical boot/suspend trials.
