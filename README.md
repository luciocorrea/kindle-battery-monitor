# Kindle battery monitor

Report a Kindle's battery percentage to Home Assistant using unencrypted BTHome v2 Bluetooth LE advertisements. Samples are taken at startup, on battery-change events, and every ten minutes while awake. Each broadcast uses a five-second timer followed by bounded radio cleanup. The monitor permits normal suspend.

The fixed advertised name is `Kindle`. Home Assistant adds an address suffix, for example `Kindle ABCD`. The name is independent of Kindle Settings. Owned bursts use the factory public Bluetooth address so receivers recognize one device across radio restarts. Nearby receivers can read the percentage and address.

## Supported device

The qualified target is a jailbroken Paperwhite 4 on firmware 5.16.5, with KUAL and working stock Bluetooth services. The installer checks the exact model/build and refuses other targets before changing startup files. See [compatibility](docs/compatibility.md).

## Installation and Home Assistant

1. Extract `kindle-battery-monitor-<version>.zip` into the Kindle's `extensions/` directory.
2. Open **KUAL > Kindle battery monitor > Install/update** and wait for its result.
3. Place the awake Kindle near a Home Assistant Bluetooth receiver and use the BTHome integration for discovery.

First installation enables boot startup and starts the monitor. Updates preserve the saved boot setting and current session independently. Restart the tested Kindle with USB disconnected and reconnect after Home/KUAL loads, following the qualification workaround in [compatibility](docs/compatibility.md).

| KUAL action | Effect |
| --- | --- |
| Install/update | Install or upgrade, preserving existing boot/session choices |
| Enable automatic startup and start | Enable boot startup and start now |
| Disable automatic startup and stop | Disable boot startup and stop now |
| Start this session | Start without changing automatic startup |
| Stop this session | Stop without changing automatic startup |
| Status | Show version, startup choice, service state, battery, and last failure |
| Uninstall entire extension | Confirm removal of the extension, owned runtime, and boot job |

Wait for each action to finish. Keep the ZIP outside the Kindle for reinstallation. Use confirmed uninstall instead of deleting an extension that still has a boot job. Home Assistant history remains intact. If recovery records are retained after an error, inspect the failure before retrying.

Home Assistant retains the last percentage while the Kindle sleeps. A competing Bluetooth owner can defer a burst until a later sample trigger. Existing BTHome devices may need their integration reloaded after receiving a named packet to refresh the default device name. The integration entry title and explicit entity-name overrides are separate labels; preserve existing entity IDs and dashboard references when renaming.

## Build and package

Requirements: Rustup with Rust 1.95.0, Clippy and rustfmt; `cross` on PATH; Docker; Python 3.11 or newer. Qualification used cross 0.2.5, commit `65fe72b`. Set `PYTHON` if the default `python3` is older.

```sh
./scripts/test.sh
./scripts/build.sh
./scripts/package.sh
```

The packaging command builds by default and writes a versioned ZIP plus `.zip.sha256` sidecar under `dist/`. To choose a Python executable, existing binary, or output directory:

```sh
PYTHON=python3.14 ./scripts/test.sh
PYTHON=python3.14 ./scripts/package.sh
./scripts/package.sh --binary /path/to/kindle-battery-monitor --output /path/to/releases
```

`rust-toolchain.toml` pins Rust. `Cross.toml` pins the qualified ARM musl image by digest. Cargo downloads registry dependencies on first use; LIPC is bundled under `crates/lipc-dbus`. The project builds without the original SDK checkout, parent workspace, firmware artifacts, or Amazon libraries to link.

The packager rejects non-ARM binaries, dynamic segments, and interpreter requirements. Payload hashes are recorded in the manifest and older-version archives are retained. SHA-256 detects damaged files; it does not authenticate a publisher.

## Development

```sh
rustup run 1.95.0 cargo test --workspace --locked
rustup run 1.95.0 cargo fmt --all --check
compiler=$(rustup which --toolchain 1.95.0 rustc)
PATH="$(dirname "$compiler"):$PATH" rustup run 1.95.0 cross clippy --workspace --target arm-unknown-linux-musleabihf --locked -- -D warnings
```

Use `cross` for target compilation. Host tests use isolated fakes, sockets, and test buses. One radio test is opt-in because it requires a host `dbus-daemon`. Tests do not access a physical Kindle. Preserve zero polling, bounded shutdown, owned radio cleanup, and normal suspend.

See the [glossary](CONTEXT.md), [protocol notes](docs/protocol.md), [startup decision](docs/adr/0001-battery-monitor-startup-controls.md), and [verification](docs/verification.md). Runtime Rust sources and KUAL controller/job assets were extracted unchanged from qualified version 0.1.1. Source/build verification and physical device qualification are recorded separately.

MIT license. See [LICENSE](LICENSE). Dependencies retain their own licenses.
