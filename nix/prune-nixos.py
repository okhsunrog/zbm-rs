"""Reconcile prepared zbm-rs generations with retained NixOS profiles.

Only the mounted root is affected; ZFS snapshots have independent copies of
their profile, store and prepared boot artifacts. Unknown files are retained.
"""
import argparse
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import shutil

STORE_NAME = re.compile(r"[0-9a-z]{32}-nixos-system-[^/]+\Z")
PROFILE_NAME = re.compile(r"system-[0-9]+-link\Z")


@contextmanager
def profile_lock(path):
    # Nix may unlink its lock file and mark the old inode while we wait.
    # Match PathLocks' stale-lock retry rather than locking a detached inode.
    while True:
        fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX)
        try:
            locked = os.fstat(fd)
            current = os.stat(path)
            valid = locked.st_ino == current.st_ino and locked.st_dev == current.st_dev and locked.st_size == 0
        except FileNotFoundError:
            valid = False
        if valid:
            break
        os.close(fd)
    try:
        yield
    finally:
        os.close(fd)


def store_system(path, store):
    resolved = path.resolve(strict=True)
    if resolved.parent != store or not STORE_NAME.fullmatch(resolved.name):
        raise ValueError(f"Invalid system link: {path}")
    return resolved


def retained_systems(profiles, run, store):
    links = [profiles / "system", run / "current-system", run / "booted-system"]
    links.extend(path for path in profiles.iterdir() if PROFILE_NAME.fullmatch(path.name))
    if not (profiles / "system").is_symlink():
        raise ValueError("Missing current system profile")
    systems = set()
    for link in links:
        if not link.is_symlink():
            raise ValueError(f"Expected a live system link: {link}")
        system = store_system(link, store)
        systems.add(system.name)
        variants = system / "specialisation"
        if variants.exists():
            for variant in variants.iterdir():
                if not variant.is_symlink():
                    raise ValueError(f"Invalid specialisation: {variant}")
                systems.add(store_system(variant, store).name)
    return systems


def plan(base, gcroots, retained, store):
    published, bundles = base / "nixos", base / "bundles"
    if any(path.is_symlink() or not path.is_dir() for path in (published, bundles, gcroots)):
        raise ValueError("Missing zbm-rs publication directories")
    removals = []
    for entry in sorted(published.iterdir()):
        if entry.name in retained or not STORE_NAME.fullmatch(entry.name):
            continue
        root = gcroots / entry.name
        if not entry.is_symlink() or not root.is_symlink():
            continue
        if root.readlink() != store / entry.name:
            continue
        target = entry.readlink()
        if target.parent != Path("../bundles") or not target.name.startswith(entry.name + "."):
            continue
        bundle = bundles / target.name
        if bundle.is_symlink() or not bundle.is_dir():
            continue
        # Do not remove a bundle shared by another published entry.
        if any(other != entry and other.is_symlink() and other.readlink() == target
               for other in published.iterdir()):
            continue
        removals.append((entry, root, bundle))
    return removals


def finish(base, gcroots, store, retained, name, bundle_name):
    if not STORE_NAME.fullmatch(name) or name in retained:
        raise ValueError("Cleanup journal references a retained or invalid system")
    if not bundle_name.startswith(name + ".") or Path(bundle_name).name != bundle_name:
        raise ValueError("Invalid bundle in cleanup journal")
    entry, root, bundle = base / "nixos" / name, gcroots / name, base / "bundles" / bundle_name
    target = Path("../bundles") / bundle_name
    if entry.is_symlink() and entry.readlink() != target:
        raise ValueError("Publication changed during cleanup")
    if entry.exists() and not entry.is_symlink():
        raise ValueError("Foreign publication encountered")
    if root.is_symlink() and root.readlink() != store / name:
        raise ValueError("GC root changed during cleanup")
    if root.exists() and not root.is_symlink():
        raise ValueError("Foreign GC root encountered")
    if bundle.is_symlink():
        raise ValueError("Refusing to delete a linked bundle")
    if any(other != entry and other.is_symlink() and other.readlink() == target
           for other in (base / "nixos").iterdir()):
        raise ValueError("Bundle is still published")
    entry.unlink(missing_ok=True)
    root.unlink(missing_ok=True)
    if bundle.exists():
        shutil.rmtree(bundle)


def clean(base, gcroots, store, retained, removals):
    journal = base / ".cleanup.json"
    if journal.is_symlink():
        raise ValueError("Invalid cleanup journal")
    if journal.exists():
        data = json.loads(journal.read_text())
        print("Recovering interrupted cleanup: " + data["system"], flush=True)
        finish(base, gcroots, store, retained, data["system"], data["bundle"])
        journal.unlink()
        os.sync()
    for entry, _, bundle in removals:
        with journal.open("x") as file:
            json.dump({"system": entry.name, "bundle": bundle.name}, file)
            file.flush()
            os.fsync(file.fileno())
        os.sync()
        finish(base, gcroots, store, retained, entry.name, bundle.name)
        journal.unlink()
        os.sync()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="remove reconciled stale publications")
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise ValueError("Generation cleanup requires root")
    base = Path("/boot/zbm-rs")
    if os.stat("/").st_dev != os.stat("/boot").st_dev or os.stat("/").st_dev != os.stat("/nix/store").st_dev:
        raise ValueError("/boot and /nix/store must belong to the mounted root")
    profiles = Path("/nix/var/nix/profiles")
    with profile_lock(profiles / "system.lock"), (base / ".publish.lock").open("a") as publication_lock:
        fcntl.flock(publication_lock, fcntl.LOCK_EX)
        retained = retained_systems(profiles, Path("/run"), Path("/nix/store"))
        removals = plan(base, Path("/nix/var/nix/gcroots/zbm-rs"), retained, Path("/nix/store"))
        journal = base / ".cleanup.json"
        if not args.apply and journal.exists():
            print("Interrupted cleanup requires recovery before applying further changes", flush=True)
        for entry, _, _ in removals:
            print(("Removing " if args.apply else "Would remove ") + entry.name, flush=True)
        if args.apply:
            clean(base, Path("/nix/var/nix/gcroots/zbm-rs"), Path("/nix/store"), retained, removals)


if __name__ == "__main__":
    main()
