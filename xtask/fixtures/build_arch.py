"""Package an archinstall_zfs mkarchiso staging tree for a disposable VM probe.

Run with uv run --no-project. --sudo only grants tar read access to mode-000
staging files; source files are never changed. No host root/cache/key is shared.
"""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("source", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--sudo", action="store_true")
args = parser.parse_args()
source = args.source.resolve(strict=True)
modules = list((source / "usr/lib/modules").iterdir())
if len(modules) != 1 or modules[0].name != "6.18.53-1-lts":
    raise SystemExit("This probe fixture requires the reviewed 6.18.53-1-lts staging kernel")
if (modules[0] / "pkgbase").read_text().strip() != "linux-lts":
    raise SystemExit("Expected linux-lts")
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=False)
archive = output / "root.tar"
archive.touch()
excluded = (
    "root", "home", "var/cache", "var/log", "etc/hostid", "etc/machine-id",
    "etc/ssh", "etc/pacman.d/gnupg", "etc/zfs/zpool.cache", "etc/zfs/zroot.key",
    "proc", "sys", "dev", "run", "tmp",
)
command = (["sudo", "-n"] if args.sudo else []) + [
    "tar", "--create", "--file", str(archive), "--numeric-owner",
    "--owner=0", "--group=0", "--one-file-system",
] + [f"--exclude=./{path}" for path in excluded] + ["-C", str(source), "."]
subprocess.run(command, check=True)
with archive.open("rb") as stream:
    checksum = hashlib.file_digest(stream, "sha256").hexdigest()
manifest = {
    "source": str(source),
    "source_kind": "mkarchiso staging tree deployed as a disposable Arch BE, not a full installer run",
    "kernel": modules[0].name,
    "pkgbase": "linux-lts",
    "excluded": excluded,
    "archive_sha256": checksum,
}
(output / "arch-fixture.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest, indent=2))
