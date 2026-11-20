# Contributing

Bug reports, ideas and pull requests are welcome. For larger changes, open an issue first so we can agree on the approach. Check [docs/roadmap.md](docs/roadmap.md) for what is planned and what is out of scope.

## Setup

- macOS 26 with Xcode 26 for the app. The Rust core and CLI also build on Linux, but the clipboard monitor and the app are macOS only.
- Rust via [rustup](https://rustup.rs). `rust-toolchain.toml` selects the stable toolchain with rustfmt and clippy. The minimum supported Rust version is the `rust-version` in `Cargo.toml`.

```sh
cargo build --features ffi
app/scripts/build-app.sh        # → app/build/ClipRust.app
```

`build-app.sh` signs with the first "Apple Development" identity in your keychain, or the one in `CLIPRUST_SIGN_IDENTITY`. Without one it falls back to an ad-hoc signature, and macOS asks for Accessibility permission again after each rebuild.

To keep your real history out of the way while testing the CLI, point it at another database with `CLIPRUST_DB=/tmp/cliprust-test.db`.

## Checks

CI runs these on every pull request; run them before you push:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features ffi -- -D warnings
cargo test --features ffi
app/scripts/build-app.sh
```

## Pull requests

- Keep each pull request to one change, with tests for behaviour changes. Few, meaningful tests beat many near-duplicates.
- Commit messages: a short imperative summary in plain English ("Add regex search", "Fix dedup after import"), with a body when the reason isn't obvious.
- Add a line to `CHANGELOG.md` under the unreleased version for user-visible changes.
- No real data in tests or fixtures: no real API keys or tokens, personal names, email addresses, phone numbers or addresses, private IP addresses or paths from your machine. Use `example.com`, `192.0.2.x` and obviously fake keys.

## Releases

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed. The version comes from `Cargo.toml`; the tag must match it (`v0.1.0` for `0.1.0`). The workflow builds arm64 binaries, packages `ClipRust.app` as a zip and the CLI as a tarball, writes `SHA256SUMS.txt` and creates a GitHub release with the matching `CHANGELOG.md` section.

Signing and notarization run only when these repository secrets are set. Without them the release is unsigned.

| Secret | Contents |
| --- | --- |
| `APPLE_DEVELOPER_ID_CERT_P12` | Developer ID Application certificate and private key, exported as .p12 and base64-encoded |
| `APPLE_DEVELOPER_ID_CERT_PASSWORD` | Password of the .p12 file |
| `APPLE_NOTARY_APPLE_ID` | Apple ID used for notarization |
| `APPLE_NOTARY_TEAM_ID` | Team ID of the developer account |
| `APPLE_NOTARY_PASSWORD` | App-specific password for that Apple ID |

After a release, point the Homebrew cask at it with `scripts/bump-cask.sh <version>` and merge that change through a pull request (`main` requires the CI checks, so the release job cannot push it).

## License

By contributing you agree that your contributions are licensed under the [MIT License](LICENSE).
