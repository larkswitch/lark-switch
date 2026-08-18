#!/usr/bin/env python3
"""Prepare target-suffixed Tauri sidecars and a config overlay.

Usage:
  python scripts/prepare-sidecars.py x86_64-pc-windows-msvc
  python scripts/prepare-sidecars.py aarch64-apple-darwin
"""
from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TAURI = ROOT / "apps" / "desktop" / "src-tauri"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: prepare-sidecars.py <rust-target>", file=sys.stderr)
        return 2
    target = sys.argv[1]
    extension = ".exe" if "windows" in target else ""
    release = ROOT / "target" / target / "release"
    binaries = TAURI / "binaries"
    binaries.mkdir(parents=True, exist_ok=True)

    entries = []
    for cargo_name, bundle_name in [("lpcctl", "lpcctl"), ("lark-cli", "lark-cli")]:
        source = release / f"{cargo_name}{extension}"
        if not source.is_file():
            raise SystemExit(f"missing built sidecar: {source}")
        destination = binaries / f"{bundle_name}-{target}{extension}"
        shutil.copy2(source, destination)
        entries.append(f"binaries/{bundle_name}")

    overlay = TAURI / "tauri.sidecars.conf.json"
    overlay.write_text(
        json.dumps({"bundle": {"externalBin": entries}}, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(overlay)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
