# Kindle battery monitor: agent guidelines

## Start with the relevant contract

- Before domain or behavior changes, read [CONTEXT.md](CONTEXT.md) and relevant [ADRs](docs/adr/).
- Before BLE payload, identity, DBus, or radio changes, read [the protocol contract](docs/protocol.md).
- Before device operations, firmware support changes, or startup/install/uninstall work, read [compatibility](docs/compatibility.md) and [the startup decision](docs/adr/0001-battery-monitor-startup-controls.md).
- Before build, dependency, or packaging changes, read [the build instructions](README.md#build-and-package).

## Workflow and authorization

- State a brief plan before editing. Inspect existing changes and preserve unrelated work.
- For behavior changes, obtain approval of the test seams and expected results before writing tests or implementation. Work in vertical slices: one failing behavior test, the smallest implementation, then verification.
- For documentation-only changes, verify references and command accuracy; executable tests are required only when behavior changes.
- Run targeted tests before broader suites. Test public behavior and external boundaries with named fakes. Keep structural assertions in narrowly scoped, explicitly named architecture-invariant tests.
- Track multi-step work in `.scratch/<feature>/spec.md`. Update its checkboxes only after the corresponding implementation and verification pass.
- Ask before advancing to another task unless the user has explicitly authorized that step. Report changed behavior, completed checks, and remaining limits; show changed blocks rather than entire files unless requested.
- Git mutations require explicit authorization for the specific operation, including initialization, staging, commits, pushes, checkout, reset, merge, and rebase. Leave changes unstaged by default.

## Device and power boundaries

- Preserve awake-only operation and normal suspend. Block on events, deadline timers, or cancellation; polling, busy waiting, suspend inhibitors, and added wake alarms violate this contract.
- Synchronize through events and state readback, not fixed sleeps. Shutdown and cancellation must complete bounded cleanup.
- Radio transactions must respect ownership and retain other clients' state. Verify advertising stops and privacy/source restoration completes on success, error, and cancellation. Follow the sequence in the protocol contract.
- Treat BTHome bytes, advertiser identity, sampling cadence, and KUAL boot/session behavior as observable contracts. Agree changed behavior and its acceptance tests before implementation.
- Firmware qualification is explicit. Keep installer guards aligned with the qualified hardware/build; architecture alone does not establish compatibility.
- Before a bounded physical test, define its duration, owned state, cleanup, restoration, and acceptance evidence. Obtain authorization for device mutations and disruptive operations. An untouched suspend window permits no SSH or other device probing.
- Keep host, container, and physical evidence separate. Passing a fake-radio test does not prove reception, cleanup on hardware, or suspend.

## Rust and system interfaces

- Document crates and modules with `//!`, and public APIs with `///`, including trait methods, associated types, enum variants, and public fields. Keep docs current in the same change as the API or behavior they describe.
- Describe caller obligations and observable contracts: blocking, deadlines, cancellation, ownership, cleanup, errors, and firmware limits where relevant. Use intra-doc links for related APIs and point to the owning repository contracts; distinguish requests and readbacks from physical evidence. Private helpers need comments when their reasoning is not apparent from the code.
- Verify documentation with `RUSTDOCFLAGS="-D warnings -D missing_docs" rustup run 1.95.0 cargo doc --workspace --no-deps --locked` and formatting. Run doc tests when adding or changing executable examples. Documentation-only edits follow the documentation verification rule above.
- Keep device IO, DBus/BSA transport, and unsafe operations behind safe project-owned APIs. Prefer `#![forbid(unsafe_code)]`; document safety invariants for any necessary unsafe boundary.
- Use explicit ownership, domain types, and typed recoverable errors with IO/operation context. `anyhow` is limited to CLI consumers, not library code. Avoid production panics.
- Reuse the existing channel primitives and explicit cancellation paths. Keep mutable state narrowly owned and dependencies injected at system boundaries.
- Use structured `tracing` events for diagnostics. Exclude credentials and personal device/account information from logs and fixtures.

## Build and release verification

- Keep this workspace independent: local source dependencies must resolve inside the project. Preserve the source-only-copy architecture invariant and bundled LIPC boundary.
- Use the pinned Rustup compiler and cross image through `cargo xtask build` for Kindle target builds. Keep host-only tooling outside the Kindle dependency graph and the executable statically linked for ARM musl; review native dependencies against the container/sysroot before adding them.
- For Rust changes, run targeted tests, then `env -u RUSTC_WRAPPER rustup run 1.95.0 cargo xtask verify --arm` for formatting, strict rustdoc, workspace tests, and host/ARM Clippy with warnings denied. See [Development](README.md#development) for prerequisites and host-only mode. For power/radio changes, also complete the agreed physical acceptance.
- For packaging changes, run the relevant command-contract tests, existing KUAL controller/archive tests, and a real package build. Verify ELF type, manifest/version agreement, payload hashes, executable permissions, and checksum sidecars.
- Preserve safe upgrades: package extraction must not overwrite the live executable or saved state; retain ownership checks, rollback, original mount mode, and independent boot/session choices. Release naming must preserve older versions.
- Source distributions contain project source, tests, configuration, and curated docs. Exclude `.scratch`, build caches, device backups/logs, credentials, and workstation-specific configuration. Use synthetic identifiers in distributable examples.

## Agent skills

### Issue tracker

Before creating, fetching, or updating tickets, read
[the local Markdown tracker conventions](docs/agents/issue-tracker.md).

### Triage labels

Before assigning ticket status, read
[the triage label mapping](docs/agents/triage-labels.md).

### Domain docs

Before domain exploration or design work, read
[the single-context domain rules](docs/agents/domain.md).
