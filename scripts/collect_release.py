"""Require the complete application/DMG/production-worker release contract."""
import hashlib
from pathlib import Path
import sys
import zipfile

from release_metadata import TARGETS, WORKER, archive_name, expected_assets


def digest(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def collect(directory, version):
    expected = expected_assets(version)
    actual = sorted(p.name for p in directory.iterdir() if p.is_file() and p.name != "SHA256SUMS")
    if actual != expected:
        raise ValueError(f"Expected exactly eight release assets; missing={sorted(set(expected)-set(actual))}, unexpected={sorted(set(actual)-set(expected))}")
    for name in expected:
        if not (directory / name).stat().st_size:
            raise ValueError(f"Empty release asset: {name}")
    for target in TARGETS:
        binary = "silicolab.exe" if "windows" in target else "silicolab"
        executable = "silicolab.app/Contents/MacOS/silicolab" if "apple-darwin" in target else binary
        with zipfile.ZipFile(directory / archive_name(version, target)) as archive:
            if executable not in archive.namelist() or archive.testzip() is not None:
                raise ValueError(f"Invalid legacy application archive: {target}")
    checksums = {name: digest(directory / name) for name in expected}
    recorded = (directory / (WORKER + ".sha256")).read_text().split()
    if not recorded or recorded[0] != checksums[WORKER]:
        raise ValueError("Production worker checksum does not match its executable")
    (directory / "SHA256SUMS").write_text("".join(f"{checksums[name]}  {name}\n" for name in expected))


if __name__ == "__main__":
    collect(Path(sys.argv[1]), sys.argv[2])
