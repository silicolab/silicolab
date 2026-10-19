"""Build legacy archives and native macOS drag-install packages using one core."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

from release_metadata import ROOT, TARGETS, WORKER, apple_versions, archive_name, cargo_version, dmg_name

IDENTIFIER = "com.silicolab.SilicoLab"
MARKER = "manual-dmg-updates"
ARCHITECTURES = {"aarch64-apple-darwin": "arm64", "x86_64-apple-darwin": "x86_64"}


def run(*args, **kwargs):
    print("+", " ".join(map(str, args)), flush=True)
    kwargs.setdefault("cwd", ROOT)
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def legal_files(destination):
    destination.mkdir(parents=True, exist_ok=True)
    for name in ["LICENSE", "THIRD-PARTY-NOTICES.md"]:
        shutil.copy2(ROOT / name, destination / name)
    shutil.copytree(ROOT / "LICENSES", destination / "LICENSES")
    notices = destination / "assets"
    (notices / "fonts").mkdir(parents=True)
    shutil.copy2(ROOT / "assets/fonts/LiberationSans-LICENSE", notices / "fonts")
    (notices / "forcefields/charmm36.ff").mkdir(parents=True)
    shutil.copy2(ROOT / "assets/forcefields/charmm36.ff/LICENSE", notices / "forcefields/charmm36.ff")


def info_plist(version):
    short, build = apple_versions(version)
    return {
        "CFBundleInfoDictionaryVersion": "6.0", "CFBundlePackageType": "APPL",
        "CFBundleName": "SilicoLab", "CFBundleDisplayName": "SilicoLab",
        "CFBundleIdentifier": IDENTIFIER, "CFBundleExecutable": "silicolab",
        "CFBundleIconFile": "AppIcon.icns", "CFBundleShortVersionString": short,
        "CFBundleVersion": build, "SilicoLabCargoVersion": version,
        "LSMinimumSystemVersion": "13.0", "NSHighResolutionCapable": True,
        "LSApplicationCategoryType": "public.app-category.education",
    }


def create_app(binary, app, version, icon, manual):
    contents = app / "Contents"
    macos = contents / "MacOS"
    resources = contents / "Resources"
    macos.mkdir(parents=True)
    shutil.copy2(binary, macos / "silicolab")
    (macos / "silicolab").chmod(0o755)
    legal_files(resources)
    shutil.copy2(icon, resources / "AppIcon.icns")
    if manual:
        (resources / MARKER).write_text("Quit SilicoLab and replace the complete app from a new DMG.\n")
    with (contents / "Info.plist").open("wb") as file:
        plistlib.dump(info_plist(version), file)


def inspect_binary(binary, architecture):
    if binary.stat().st_mode & 0o7777 != 0o755:
        raise ValueError(f"Executable must have mode 0755: {binary}")
    arches = subprocess.check_output(["lipo", "-archs", str(binary)], text=True).strip()
    if arches != architecture:
        raise ValueError(f"Expected {architecture}, got {arches}")
    dependencies = subprocess.check_output(["otool", "-L", str(binary)], text=True)
    for line in dependencies.splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        if not dependency.startswith(("/usr/lib/", "/System/Library/")):
            raise ValueError(f"Non-system dependency: {dependency}")
    commands = subprocess.check_output(["otool", "-l", str(binary)], text=True)
    minimum = re.search(r"\bminos\s+(\d+\.\d+(?:\.\d+)?)", commands)
    if not minimum or tuple(map(int, minimum[1].split(".")[:2])) != (13, 0):
        raise ValueError("The release binary must target macOS 13.0")
    return {"architecture": arches, "minimum_macos": minimum[1], "mode": "0755",
            "dependencies": dependencies}


def validate_app(app, target, version, manual):
    contents = app / "Contents"
    resources = contents / "Resources"
    if plistlib.loads((contents / "Info.plist").read_bytes()) != info_plist(version):
        raise ValueError("Bundle metadata differs from Cargo version metadata")
    run("plutil", "-lint", contents / "Info.plist")
    for name in ["LICENSE", "THIRD-PARTY-NOTICES.md", "AppIcon.icns",
                 "LICENSES/GPL-3.0-or-later.txt", "assets/fonts/LiberationSans-LICENSE",
                 "assets/forcefields/charmm36.ff/LICENSE"]:
        if not (resources / name).is_file() or not (resources / name).stat().st_size:
            raise ValueError(f"Missing bundle resource: {name}")
    if (resources / MARKER).is_file() != manual:
        raise ValueError("Only the new DMG bundle may disable legacy binary updates")
    with tempfile.TemporaryDirectory() as temporary:
        iconset = Path(temporary) / "verify.iconset"
        run("iconutil", "-c", "iconset", "-o", iconset, resources / "AppIcon.icns")
        for size in [16, 32, 128, 256, 512]:
            for retina in [False, True]:
                png = iconset / f"icon_{size}x{size}{'@2x' if retina else ''}.png"
                data = png.read_bytes()
                expected = size * (2 if retina else 1)
                if (int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")) != (expected, expected):
                    raise ValueError(f"Incorrect icon representation: {png.name}")
    if manual:
        run("codesign", "--verify", "--deep", "--strict", app)
    return inspect_binary(contents / "MacOS/silicolab", ARCHITECTURES[target])


def create_dmg(staging, dmg):
    from dmg_layout import validate_layout, write_layout
    with tempfile.TemporaryDirectory(prefix="silicolab-dmg-") as temporary:
        temporary = Path(temporary)
        tiffs = []
        for width, name, dpi in [(640, "background", 72), (1280, "background@2x", 144)]:
            png = temporary / f"{name}.png"
            tiff = temporary / f"{name}.tiff"
            run("cargo", "run", "--locked", "-q", "-p", "xtask", "--features", "macos-packaging",
                "--", "render-svg", ROOT / "assets/dmg-background.svg", width, png)
            run("sips", "-s", "format", "tiff", "-s", "dpiWidth", dpi, "-s", "dpiHeight", dpi,
                png, "--out", tiff)
            tiffs.append(tiff)
        run("tiffutil", "-cathidpicheck", *tiffs, "-out", staging / ".background.tiff")
        writable = temporary / "writable.dmg"
        run("hdiutil", "create", "-volname", "SilicoLab", "-srcfolder", staging,
            "-fs", "HFS+", "-format", "UDRW", writable)
        mount = temporary / "volume"
        mount.mkdir()
        run("hdiutil", "attach", "-nobrowse", "-mountpoint", mount, writable)
        try:
            write_layout(mount)
            validate_layout(mount)
        finally:
            run("hdiutil", "detach", mount)
        run("hdiutil", "convert", writable, "-format", "UDZO", "-ov", "-o", dmg)
    run("hdiutil", "verify", dmg)


def smoke(executable, directory):
    result = run(executable, "--help", cwd=directory, capture_output=True, text=True, timeout=30)
    if not result.stdout.strip():
        raise ValueError("Packaged executable help returned empty output")


def security_evidence(app, dmg):
    def probe(*args):
        result = subprocess.run(list(map(str, args)), capture_output=True, text=True)
        return {"exit_code": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    return {
        "signature_kind": "ad-hoc", "notarized": False, "stapled": False,
        "dmg_signature_kind": "unsigned",
        "codesign": probe("codesign", "-dv", "--verbose=2", app),
        "gatekeeper": probe("spctl", "--assess", "--type", "execute", "--verbose=2", app),
        "app_quarantine": probe("xattr", "-p", "com.apple.quarantine", app),
        "dmg_quarantine": probe("xattr", "-p", "com.apple.quarantine", dmg),
        "internet_download_install": "not tested; local mounting/copying does not establish download acceptance",
    }


def package(target, output, skip_build=False, skip_runtime_smoke=False):
    macos = target in ARCHITECTURES
    if macos and sys.platform != "darwin":
        raise RuntimeError("macOS packages require macOS and Apple command-line tools")
    version = cargo_version()
    apple_versions(version)
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    if macos:
        environment["MACOSX_DEPLOYMENT_TARGET"] = "13.0"
    if not skip_build:
        run("cargo", "build", "--locked", "--release", "--target", target, "-p", "silicolab", env=environment)
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    if not target_dir.is_absolute():
        target_dir = ROOT / target_dir
    binary_name = "silicolab.exe" if "windows" in target else "silicolab"
    binary = target_dir / target / "release" / binary_name
    staging = output / f"package-{TARGETS[target]}"
    if staging.exists():
        shutil.rmtree(staging)
    legal_files(staging)
    artwork = output / f"artwork-{TARGETS[target]}"
    if macos:
        environment["RASTERIZER"] = "xtask"
        run(sys.executable, "scripts/gen-icons.py", "--macos-only", "--output", artwork, env=environment)
        app = staging / "silicolab.app"
        create_app(binary, app, version, artwork / "AppIcon.icns", manual=False)
        validate_app(app, target, version, manual=False)
    else:
        shutil.copy2(binary, staging / binary_name)
    archive = output / archive_name(version, target)
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
        for file in sorted(staging.rglob("*")):
            if file.is_file():
                zipped.write(file, file.relative_to(staging).as_posix())
    with zipfile.ZipFile(archive) as zipped:
        if zipped.testzip() is not None:
            raise ValueError("Archive CRC validation failed")
        expected = "silicolab.app/Contents/MacOS/silicolab" if macos else binary_name
        if expected not in zipped.namelist():
            raise ValueError("Archive does not preserve the existing updater executable path")
        with tempfile.TemporaryDirectory(prefix="silicolab-archive-smoke-") as temporary:
            zipped.extractall(temporary)
            executable = Path(temporary) / expected
            executable.chmod(0o755)
            if not skip_runtime_smoke:
                smoke(executable, Path(temporary))
    if not macos:
        return
    dmg_staging = output / f"dmg-{TARGETS[target]}"
    if dmg_staging.exists():
        shutil.rmtree(dmg_staging)
    app = dmg_staging / "SilicoLab.app"
    create_app(binary, app, version, artwork / "AppIcon.icns", manual=True)
    run("codesign", "--force", "--sign", "-", "--timestamp=none", "--identifier", IDENTIFIER, app)
    validate_app(app, target, version, manual=True)
    (dmg_staging / "Applications").symlink_to("/Applications", target_is_directory=True)
    dmg = output / dmg_name(version, target)
    create_dmg(dmg_staging, dmg)
    installed = output / f"installed-{TARGETS[target]}"
    if installed.exists():
        shutil.rmtree(installed)
    installed.mkdir()
    from dmg_layout import validate_layout
    with tempfile.TemporaryDirectory(prefix="silicolab-installed-") as temporary:
        mount = Path(temporary) / "volume"
        mount.mkdir()
        run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg)
        try:
            layout = validate_layout(mount)
            if os.readlink(mount / "Applications") != "/Applications":
                raise ValueError("Invalid Applications drag-install shortcut")
            if sorted(p.name for p in mount.iterdir() if not p.name.startswith(".")) != ["Applications", "SilicoLab.app"]:
                raise ValueError("Unexpected visible DMG contents")
            run("ditto", mount / "SilicoLab.app", installed / "SilicoLab.app")
        finally:
            run("hdiutil", "detach", mount)
    installed_app = installed / "SilicoLab.app"
    binary_report = validate_app(installed_app, target, version, manual=True)
    if not skip_runtime_smoke:
        smoke(installed_app / "Contents/MacOS/silicolab", installed)
    report = {"target": target, "version": version, "dmg": str(dmg), "archive": str(archive),
              "installed_copy": str(installed_app), "finder_layout": layout,
              "binary": binary_report,
              "cli_smoke": "not executed (--skip-runtime-smoke)" if skip_runtime_smoke else "passed",
              "gui_runtime": "not tested by packaging script", "security": security_evidence(installed_app, dmg)}
    (output / f"macos-validation-{TARGETS[target]}.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Created {archive}\nCreated {dmg}\nSignature: ad-hoc; not notarized.")


def package_worker(output):
    version = cargo_version()
    run("cargo", "xtask", "build-dev-worker")
    binary = ROOT / "target/x86_64-unknown-linux-musl/release/silicolab-compute"
    header = binary.read_bytes()[:20]
    if header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != 62:
        raise ValueError("Worker is not a Linux x86-64 ELF executable")
    result = run(binary, "--version", capture_output=True, text=True, timeout=30)
    if result.stdout.strip() != version:
        raise ValueError("Worker version does not match Cargo metadata")
    output.mkdir(parents=True, exist_ok=True)
    destination = output / WORKER
    shutil.copy2(binary, destination)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    (output / (WORKER + ".sha256")).write_text(f"{digest}  {WORKER}\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    choice = parser.add_mutually_exclusive_group(required=True)
    choice.add_argument("--target", choices=TARGETS)
    choice.add_argument("--worker", action="store_true", help="Linux x86-64 release worker and checksum")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--skip-runtime-smoke", action="store_true", help="Cross-packaging only; record runtime checks as not performed")
    args = parser.parse_args()
    if args.worker:
        if args.skip_build or args.skip_runtime_smoke:
            parser.error("worker releases always build and execute the version smoke check")
        package_worker(args.output.resolve())
    else:
        package(args.target, args.output.resolve(), args.skip_build, args.skip_runtime_smoke)
