#!/usr/bin/env python3
"""Generate every committed icon artifact from the master SVG.

Self-contained on macOS: `qlmanage` (QuickLook) rasterizes the SVG with no
install; Pillow/numpy downscale, bake Apple's superellipse squircle, and pack
the multi-res .ico; `iconutil` builds the .icns. Re-run after editing the master
art or tuning the squircle.

Deps (all present on stock macOS + conda Python): qlmanage, iconutil, python3
with Pillow + numpy. Fallback rasterizer if qlmanage misbehaves:
`cargo install resvg` then run with RASTERIZER=resvg.
"""
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ICON = ROOT / "assets" / "icon"
MASTER = ICON / "master-dark.svg"

# Squircle knobs — tune to match native Dock icons on macOS 27 (spec §6/§10).
SQ_N = 5.0      # superellipse exponent (~5 ~ Apple corner)
SQ_FILL = 1.0   # fraction of the canvas the shape spans
BASE = 1024     # master raster size; all targets downscale from here


def rasterize_svg(svg: Path, size: int) -> Image.Image:
    """SVG -> RGBA image at size×size. QuickLook by default; resvg if asked."""
    with tempfile.TemporaryDirectory() as td:
        if os.environ.get("RASTERIZER") == "resvg":
            out = Path(td) / "out.png"
            subprocess.run(
                ["resvg", "-w", str(size), "-h", str(size), str(svg), str(out)],
                check=True,
            )
        else:
            subprocess.run(
                ["qlmanage", "-t", "-s", str(size), "-o", td, str(svg)],
                check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            out = Path(td) / (svg.name + ".png")
        if not out.exists():
            sys.exit(f"error: rasterizer produced no PNG for {svg}")
        img = Image.open(out).convert("RGBA")
    if img.size != (size, size):
        img = img.resize((size, size), Image.LANCZOS)
    return img


def squircle_mask(size: int, n: float, fill: float, ss: int = 4) -> Image.Image:
    """Anti-aliased superellipse alpha mask (255 inside). Supersampled ss×."""
    s = size * ss
    c = s / 2.0
    a = c * fill
    ys, xs = np.mgrid[0:s, 0:s].astype(np.float64)
    r = np.abs((xs + 0.5 - c) / a) ** n + np.abs((ys + 0.5 - c) / a) ** n
    inside = (r <= 1.0).astype(np.uint8) * 255
    return Image.fromarray(inside, "L").resize((size, size), Image.LANCZOS)


def masked_master() -> Image.Image:
    art = rasterize_svg(MASTER, BASE)
    mask = squircle_mask(BASE, SQ_N, SQ_FILL)
    r, g, b, a = art.split()
    combined = np.minimum(np.asarray(a), np.asarray(mask)).astype(np.uint8)
    return Image.merge("RGBA", (r, g, b, Image.fromarray(combined, "L")))


def main() -> None:
    if not MASTER.exists():
        sys.exit(f"error: {MASTER} missing (run Task 1 first)")
    master = masked_master()

    def at(px: int) -> Image.Image:
        return master.resize((px, px), Image.LANCZOS)

    print("==> macOS .icns")
    with tempfile.TemporaryDirectory() as td:
        iconset = Path(td) / "AppIcon.iconset"
        iconset.mkdir()
        # (render px, iconset label) — Apple needs 1x and 2x per base size.
        for px, label in [
            (16, "16x16"), (32, "16x16@2x"), (32, "32x32"), (64, "32x32@2x"),
            (128, "128x128"), (256, "128x128@2x"), (256, "256x256"),
            (512, "256x256@2x"), (512, "512x512"), (1024, "512x512@2x"),
        ]:
            at(px).save(iconset / f"icon_{label}.png")
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(ICON / "AppIcon.icns")],
            check=True,
        )

    print("==> Windows .ico")
    at(256).save(
        ICON / "silicolab.ico",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (256, 256)],
    )

    print("==> runtime window icon")
    at(256).save(ICON / "window-256.png")

    print("==> Linux hicolor")
    for px in (16, 22, 24, 32, 48, 64, 128, 256):
        out = ICON / "hicolor" / f"{px}x{px}" / "apps"
        out.mkdir(parents=True, exist_ok=True)
        at(px).save(out / "silicolab.png")

    print("done.")


if __name__ == "__main__":
    main()
