"""The Cargo version and filenames consumed by application/worker deployment."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    "aarch64-apple-darwin": "macos-aarch64",
    "x86_64-apple-darwin": "macos-x86_64",
    "x86_64-pc-windows-msvc": "windows-x86_64",
    "x86_64-unknown-linux-gnu": "linux-x86_64",
}
WORKER = "silicolab-compute-x86_64-unknown-linux-musl"
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-(alpha|beta)\.(0|[1-9][0-9]*))?\Z")


def cargo_version():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=ROOT))
    return next(p["version"] for p in metadata["packages"] if p["name"] == "silicolab")


def apple_versions(version):
    match = VERSION.fullmatch(version)
    if not match:
        raise ValueError(f"Unsupported release version: {version}")
    major, minor, patch = map(int, match.group(1, 2, 3))
    stage = {"alpha": 0, "beta": 100000, None: 200000}[match[4]]
    sequence = int(match[5] or 0)
    build = patch * 1000000 + stage + sequence
    if sequence > 99999 or max(major, minor, build) > 2147483647:
        raise ValueError("Apple version components exceed the supported numeric mapping")
    return f"{major}.{minor}.{patch}", f"{major}.{minor}.{build}"


def archive_name(version, target):
    # self_update selects by Rust target substring and expects exactly one archive.
    return f"silicolab-{version}-{target}.zip"


def dmg_name(version, target):
    # Deliberately omit the Rust triple so legacy self_update cannot select a DMG.
    return f"silicolab-{version}-{TARGETS[target]}.dmg"


def expected_assets(version):
    apple_versions(version)
    return sorted([archive_name(version, t) for t in TARGETS]
                  + [dmg_name(version, t) for t in TARGETS if t.endswith("apple-darwin")]
                  + [WORKER, WORKER + ".sha256"])


if __name__ == "__main__":
    import os
    version = cargo_version()
    apple_versions(version)
    tag = os.environ.get("TAG", "")
    if tag and tag != "v" + version:
        raise ValueError(f"Tag {tag!r} does not match Cargo version {version}")
    result = f"version={version}\nprerelease={'true' if '-' in version else 'false'}\n"
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            file.write(result)
    print(result, end="")
