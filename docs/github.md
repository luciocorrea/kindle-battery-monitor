# GitHub and releases

The repository contains an MIT-licensed Rust application, its bundled LIPC crate, KUAL controller and host-only xtask tooling. It needs no Kindle credentials or hardware in CI.

## Initial public repository setup

Create an empty public GitHub repository with the name `kindle-battery-monitor`. Keep this checkout's existing license and history. After reviewing and committing the publication files, add the repository as `origin` and push `main`. Repository creation, commits and pushes are separate operator actions; adding these files performs none of them.

Enable GitHub Actions. No custom repository secrets are required. CI uses the built-in token with read-only repository contents permission. Set `CI / Verify and package` as a required check in a branch ruleset for `main`; confirm the check's displayed name after the first run. Require pull requests for changes to `main` and restrict creation or modification of release tags to maintainers.

Before the first public push, inspect both tracked files and history for private material. Keep `.scratch`, `target`, `dist`, device backups/logs, credentials and workstation settings outside Git. `.gitattributes` also excludes local evidence and build directories from Git-generated source archives. It cannot remove files already present in history.

## Verification and artifacts

Pushes to `main`, pull requests and manual CI runs execute `cargo xtask verify --arm`, then `cargo xtask package` on Ubuntu 24.04. Rust comes from `rust-toolchain.toml`; cross is pinned to the qualified commit; `Cross.toml` selects the ARM musl image by digest. Third-party actions are pinned to full commit hashes.

The artifact `kindle-kual-<commit>` contains only the versioned KUAL ZIP and its SHA-256 sidecar. Download it from the successful workflow run; artifacts expire after 14 days. CI never uploads the workspace, device evidence or build cache. Physical radio/suspend acceptance remains separate from these checks.

## Versioned release

1. Set the application version in the root `Cargo.toml`, refresh `Cargo.lock`, and complete verification and any required physical qualification.
2. Merge the release change into `main`, then create and push `v<version>`, for example `v0.1.2` for package version `0.1.2`.
3. The release workflow reuses CI at the tagged commit. A mismatched tag fails before verification/build. After success, a separate job downloads and checks the same package artifacts and creates a **draft** GitHub Release.
4. Review the draft, compatibility statement, package checksum and qualification evidence. Publish it through GitHub when ready. Leave it as a draft while physical acceptance is incomplete.

Only the draft-creation job gets `contents: write`. Pull requests use `pull_request`, run on hosted runners and receive no custom secrets. Avoid changing this to `pull_request_target` to run contributor code with publishing permissions. Failed publication can leave an existing draft; inspect it before retrying. The workflow intentionally does not overwrite releases or assets.

The workflow creates the KUAL distribution, not a crates.io publication. The package manifests retain `publish = false`.
