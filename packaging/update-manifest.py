"""Writes update.json, the manifest the app's updater reads.

    python3 packaging/update-manifest.py <version> <dist dir>

Lists, for each platform the app updates on, the release file it installs
with its size and SHA-256. The release workflow then signs the file's exact
bytes with the release key (update.json.sig); the app only trusts it if
that signature checks out (crates/client/src/update.rs).
"""

import hashlib
import json
import os
import sys

# Platform (as the app names itself) -> the file it installs.
FILES = {
    "windows-x86_64": "Sisyphus-windows-x86_64-setup.exe",
    "macos-universal": "Sisyphus-macos-universal.app.tar.gz",
    "linux-x86_64": "Sisyphus-linux-x86_64.AppImage",
    "linux-aarch64": "Sisyphus-linux-aarch64.AppImage",
}


def main() -> None:
    version, dist = sys.argv[1], sys.argv[2]
    files = {}
    for platform, name in FILES.items():
        path = os.path.join(dist, name)
        with open(path, "rb") as f:
            data = f.read()
        files[platform] = {
            "name": name,
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        }
    manifest = {"kind": "sisyphus-update/1", "version": version, "files": files}
    with open(os.path.join(dist, "update.json"), "w") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    main()
