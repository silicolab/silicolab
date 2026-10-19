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

### Native macOS DMGs

In addition to the legacy archives, publish both `silicolab-<version>-macos-aarch64.dmg`
and `silicolab-<version>-macos-x86_64.dmg`. These names deliberately omit the Rust
triple: the existing updater must continue selecting the matching archive, not a
DMG. Each DMG contains `SilicoLab.app` and an Applications shortcut, with large
left/right icons and a Retina-aware arrow background.

The app uses bundle identifier `com.silicolab.SilicoLab`, declares macOS 13.0,
and carries license notices in Resources. Only DMG bundles carry
`Contents/Resources/manual-dmg-updates`: they require whole-app replacement to
preserve their signature. The legacy archive's lowercase `silicolab.app` path,
absence of that marker and existing binary-update behavior are unchanged.

Cargo metadata is the only version source. `CFBundleShortVersionString` is
`X.Y.Z`; `SilicoLabCargoVersion` retains full SemVer. `CFBundleVersion` is
`X.Y.(Z * 1000000 + stage + N)`, with stage 0 for alpha, 100000 for beta and
200000 for stable (stable N = 0). Thus `0.1.0-beta.1` maps to `0.1.100001` and
`0.1.0` to `0.1.200000`. Prerelease N must be 0–99999 and components must fit
signed 32-bit integers; unsupported values fail rather than truncate. This
bundle mapping does not change update detection or the worker's version pin.

DMG apps are **ad-hoc signed, not Developer ID signed or notarized**; the DMG
itself is unsigned. A successful local build/copy/launch is not Internet-download
acceptance. Trusted public distribution additionally requires authorized Developer
ID signing with hardened runtime, notarization, stapling and testing the final
downloaded artifact. Never remove quarantine or change Gatekeeper policy as a
packaging step. Record actual `codesign`, `spctl` and quarantine results.

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

## Reproduce packaging and rehearse CI

On macOS with Python 3.11+, Apple command-line tools and the matching Rust target:

```sh
python3 -m venv /tmp/silicolab-packaging-python
/tmp/silicolab-packaging-python/bin/python -m pip install -r scripts/requirements-packaging.txt
/tmp/silicolab-packaging-python/bin/python scripts/package_release.py --target aarch64-apple-darwin
```

Use `x86_64-apple-darwin` on Intel. Cross-packaging requires that Rust target and
`--skip-runtime-smoke`; it records that no executable was run. A matching CI
runner never uses that flag. `--skip-build` only reuses matching release binaries
already built with `MACOSX_DEPLOYMENT_TARGET=13.0`. `CARGO_TARGET_DIR` is respected
for application builds. `--output` selects a separate output directory; quit any
running test copy before reusing the same directory.

The shared script writes packages under `dist/`, mounts each actual DMG read-only,
checks Finder layout/background references, copies the app to a separate
`installed-*` directory, ejects the image and validates that copy. It never
installs into Applications or launches the GUI. Validation reports record
architecture, minimum OS, dependencies, resources, signature, quarantine probes
and Gatekeeper assessment; a rejected assessment is not reported as acceptance.
All runtime resources are embedded; external engines remain optional installations.

`scripts/gen-icons.py` remains the canonical brand-icon generator. Packaging uses
its `--macos-only --output` mode and the optional xtask SVG renderer, so CI does not
need Finder or QuickLook. The master SVG is rendered at 1024 pixels before
smaller ICNS representations are downsampled; no low-resolution asset is enlarged.
Python image/layout tools are pinned build-time tools, not app runtime dependencies.

Run `cargo xtask check-packaging` with these tools installed. Set
`SILICOLAB_PACKAGING_PYTHON` to the virtual environment's interpreter if it is not
the default Python. This matches CI's packaging checks without changing the
existing Python-independent `cargo pr-check` Rust gates.

The Release workflow builds four legacy archives, two DMGs, the exact worker and
its checksum. Collection requires those eight assets, validates the legacy archive
paths and worker checksum, then generates `SHA256SUMS` for all eight. The manifest
is the ninth release attachment. **Run workflow** uploads the complete
`validated-release` artifact without creating a release. A matching `v*` tag
creates or updates a draft only; an already-published release is never overwritten.

## 4. Publish and verify

1. Create a draft GitHub Release for the exact tag.
2. Upload the validated four application archives, both DMGs, the worker, its
   checksum, `SHA256SUMS`, and release notes.
3. Verify the worker reports the release version with
   `silicolab-compute --version`.
4. Download each archive, inspect its internal executable path, and start the
   application on its target OS.
5. Verify the worker checksum from the downloaded assets, not the build tree.
6. Publish the release only after all checks pass.
7. From a previous legacy-archive installation, confirm update detection and one
   in-place update on a writable installation. For DMG installations, quit the app
   and verify whole-app replacement keeps projects and settings.
8. From the new application, submit one production remote job and confirm that
   the exact-version worker is downloaded, verified, deployed, and executed.

Package-manager publication is not part of this release contract. Developer ID
signing and notarization remain the explicit trusted-distribution gate above.
