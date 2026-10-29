# Contributing

This project is licensed under the [MIT license](LICENSE). Contributions should preserve the [protocol](docs/protocol.md), [qualified firmware limits](docs/compatibility.md), and normal Kindle suspend.

Use the pinned Rust toolchain and run `cargo xtask verify --arm` before opening a pull request. See [development prerequisites](README.md#development). Add tests at the relevant public behavior or system boundary when behavior changes, and update Rust documentation with the implementation.

Host tests use fakes and isolated DBus connections. Physical tests require an explicit plan covering duration, ownership, cleanup, restoration and evidence. Keep hardware results separate from host/container checks. Never include device backups, private identifiers, credentials or `.scratch` evidence in a pull request.

Pull requests run verification and build a downloadable KUAL ZIP in GitHub Actions. Include the problem, resulting behavior and relevant validation in the description. See [GitHub and releases](docs/github.md) for the publishing process.
