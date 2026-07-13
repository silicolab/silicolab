# Releasing SilicoLab

This document owns the release contract consumed by automatic application
updates and remote-worker deployment. Do not publish a non-draft GitHub Release
until every required asset is present and verified.

## 1. Prepare the version

1. Update `workspace.package.version` in `Cargo.toml`; every crate inherits it.
2. Update `Cargo.lock` and any user-facing release notes.
3. If an on-disk format changed incompatibly, add a migration before changing
   its format constant. Never reuse a format version for a different shape.
4. Run `cargo pr-check` from a clean checkout and let the cross-OS PR matrix pass.
5. Merge the release preparation PR to `main`.

The Git tag must be exactly `v<workspace package version>`, for example
`v0.2.0`. The application update check and production worker resolver both
reject a different tag scheme.

## 2. Build application archives

Build release binaries from the tagged commit for every target the release
claims to support. Each GitHub asset filename must contain its Rust target
triple because the `self_update` client selects an asset by that substring.

Use `.zip` or `.tar.gz`; both formats are supported by the current updater.
Inside an archive:

- Windows contains `silicolab.exe` at the archive root.
- Linux contains `silicolab` at the archive root.
- macOS contains
  `silicolab.app/Contents/MacOS/silicolab` exactly, with no leading `./`.

Include `LICENSE`, `LICENSES/`, and `THIRD-PARTY-NOTICES.md` in every binary
distribution. Use an unambiguous naming scheme such as
`silicolab-<version>-<target>.zip` or `.tar.gz`; do not publish two application
archives containing the same target substring.

## 3. Build the remote worker

Build the worker from the same tagged commit:

```text
cargo xtask build-dev-worker
```

Publish the raw ELF executable under this exact asset name:

```text
silicolab-compute-x86_64-unknown-linux-musl
```

Publish its SHA-256 as a second asset with the exact name:

```text
silicolab-compute-x86_64-unknown-linux-musl.sha256
```

The checksum file's first whitespace-delimited token must be the 64-character
hex digest of the uploaded worker bytes. Production deployment fails closed if
either asset is missing or the digest differs.

## 4. Publish and verify

1. Create a draft GitHub Release for the exact tag.
2. Upload every application archive, the worker, its checksum, and release notes.
3. Verify the worker reports the release version with
   `silicolab-compute --version`.
4. Download each archive, inspect its internal executable path, and start the
   application on its target OS.
5. Verify the worker checksum from the downloaded assets, not the build tree.
6. Publish the release only after all checks pass.
7. From the previous released application, confirm update detection and one
   in-place update on a writable installation.
8. From the new application, submit one production remote job and confirm that
   the exact-version worker is downloaded, verified, deployed, and executed.

The repository does not currently claim code signing or package-manager
publication. Add those requirements here before presenting them as part of the
release process.
