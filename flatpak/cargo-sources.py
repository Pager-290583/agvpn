#!/usr/bin/python3
"""Generates flatpak/cargo-sources.json from Cargo.lock (like flatpak-cargo-generator).

Every crate becomes an archive source with its crates.io checksum, so flatpak-builder
downloads and verifies them itself and cargo builds fully offline.

    python3 flatpak/cargo-sources.py Cargo.lock flatpak/cargo-sources.json
"""

import json
import sys
import tomllib

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR = "cargo/vendor"

lock_path, out_path = sys.argv[1], sys.argv[2]
with open(lock_path, "rb") as fh:
    lock = tomllib.load(fh)

sources = []
for pkg in lock["package"]:
    source = pkg.get("source")
    if source is None:
        continue  # the app itself
    if source != CRATES_IO:
        raise SystemExit(f"unsupported source for {pkg['name']}: {source}")
    name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
    dest = f"{VENDOR}/{name}-{version}"
    sources.append({
        "type": "archive",
        "archive-type": "tar-gzip",
        "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
        "sha256": checksum,
        "dest": dest,
    })
    sources.append({
        "type": "inline",
        "contents": json.dumps({"package": checksum, "files": {}}),
        "dest": dest,
        "dest-filename": ".cargo-checksum.json",
    })

sources.append({
    "type": "inline",
    "contents": f'[source.vendored-sources]\ndirectory = "{VENDOR}"\n\n'
                '[source.crates-io]\nreplace-with = "vendored-sources"\n',
    "dest": "cargo",
    "dest-filename": "config.toml",
})

with open(out_path, "w", encoding="utf-8") as fh:
    json.dump(sources, fh, indent=2)
    fh.write("\n")
print(f"{len(sources) // 2} crates -> {out_path}")
