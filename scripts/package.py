#!/usr/bin/env python3
"""Portable package: dist/ModelCraft-<version>-portable.zip with both executables, the README,
NOTICE and the licenses (the same files as the installer, nothing to install).

Run after the Windows build has copied modelcraft.exe and swcraft-scan.exe into dist/:
    python3 scripts/package.py
"""
import pathlib, re, zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
ver = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)
dist = ROOT / "dist"
out = dist / f"ModelCraft-{ver}-portable.zip"
base = f"ModelCraft-{ver}"
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for name in ["modelcraft.exe", "swcraft-scan.exe"]:
        z.write(dist / name, f"{base}/{name}")
    for name in ["README.md", "NOTICE.md"]:
        z.write(ROOT / name, f"{base}/{name}")
    for f in sorted((ROOT / "licenses").rglob("*")):
        if f.is_file():
            z.write(f, f"{base}/licenses/{f.relative_to(ROOT / 'licenses').as_posix()}")
print(out)
