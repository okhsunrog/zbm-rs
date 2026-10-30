"""Install one explicitly configured EFI image, preserving other files and boot order."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import tempfile


def command(args):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(f"{Path(args[0]).name}: {result.stderr.decode(errors='replace')}")
    return result.stdout.decode()


def verify(image, certificate):
    data = image.read_bytes()
    if not 512 <= len(data) <= 80 * 1024 * 1024 or data[:2] != b"MZ":
        raise ValueError("Expected a bounded EFI executable")
    pe = struct.unpack_from("<I", data, 0x3c)[0]
    if pe + 264 > len(data) or data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("Invalid PE header")
    if struct.unpack_from("<H", data, pe + 24)[0] != 0x20b:
        raise ValueError("Expected PE32+ EFI image")
    if (struct.unpack_from("<H", data, pe + 4)[0] != 0x8664
            or struct.unpack_from("<H", data, pe + 24 + 68)[0] != 10):
        raise ValueError("Expected an x86-64 EFI application")
    # PE's certificate directory is the fifth data directory in PE32+.
    certificate_offset, certificate_size = struct.unpack_from("<II", data, pe + 24 + 112 + 4 * 8)
    if certificate:
        if not certificate_size:
            raise ValueError("The main EFI must be signed")
        command(["sbverify", "--cert", certificate, image])
    elif certificate_size or certificate_offset:
        raise ValueError("Rescue EFI must be unsigned: do not authorize a verification bypass")


def entry(config):
    """Create only our entry; append it without moving any existing choice."""
    output = command(["efibootmgr", "--verbose"])
    path = "\\" + config["path"].replace("/", "\\")
    entries = re.findall(r"^Boot([0-9A-Fa-f]{4})\*?\s+([^\t\n]+)\t([^\n]+)$", output, re.M)
    matches = [(number, device) for number, label, device in entries if label.rstrip() == config["label"]]
    if len(matches) > 1:
        raise ValueError("Ambiguous EFI entry label")
    if matches:
        number, device = matches[0]
        expected_path = (f"File({path})".casefold() in device.casefold()
                         or device.casefold().endswith("/" + path.casefold()))
        if (f",GPT,{config['partuuid']},".casefold() not in device.casefold()
                or not expected_path):
            raise ValueError("Existing EFI label refers to another partition or file")
    else:
        block = Path("/dev/disk/by-partuuid", config["partuuid"]).resolve(strict=True)
        info = json.loads(command(["lsblk", "--json", "--nodeps", "--output", "PKNAME,PARTN", block]))["blockdevices"][0]
        if not info["pkname"] or not info["partn"]:
            raise ValueError("EFI device must be an explicit partition")
        command(["efibootmgr", "--create-only", "--disk", "/dev/" + info["pkname"],
                 "--part", str(info["partn"]), "--label", config["label"], "--loader", path])
        updated = command(["efibootmgr", "--verbose"])
        found = re.findall(r"^Boot([0-9A-Fa-f]{4})\*?\s+" + re.escape(config["label"]) + r"\t", updated, re.M)
        if len(found) != 1:
            raise ValueError("Cannot identify newly created EFI entry")
        number = found[0]
    order = re.search(r"^BootOrder: ([0-9A-Fa-f,]+)$", output, re.M)
    choices = order[1].split(",") if order else []
    if number.upper() not in [choice.upper() for choice in choices]:
        command(["efibootmgr", "--bootorder", ",".join([*choices, number])])


def install(config, esp):
    relative = PurePosixPath(config["path"])
    if relative.is_absolute() or ".." in relative.parts or not str(relative).startswith("EFI/"):
        raise ValueError("Expected a relative EFI/ path")
    image = Path(config["image"])
    verify(image, config.get("certificate"))
    target = esp / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    if not target.parent.resolve().is_relative_to(esp.resolve()) or target.is_symlink():
        raise ValueError("EFI destination escapes the selected ESP")
    changed = not target.exists() or hashlib.sha256(target.read_bytes()).digest() != hashlib.sha256(image.read_bytes()).digest()
    # Verify the staged bytes before publishing a filename reachable by firmware.
    temporary = None
    try:
        if changed:
            fd, temporary = tempfile.mkstemp(prefix=".zbm-efi-", dir=target.parent)
            temporary = Path(temporary)
            with os.fdopen(fd, "wb") as stream, image.open("rb") as source:
                shutil.copyfileobj(source, stream)
                stream.flush()
                os.fsync(stream.fileno())
            verify(temporary, config.get("certificate"))
        entry(config)
        if changed:
            os.replace(temporary, target)
            temporary = None
            directory = os.open(target.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(f"EFI {config['label']}: {'installed' if changed else 'unchanged'}")


@contextmanager
def mounted(config, state):
    device = Path("/dev/disk/by-partuuid", config["partuuid"])
    # JSON avoids escaping ambiguities in mount points. Never mount a foreign disk.
    result = subprocess.run(["findmnt", "--json", "--source", str(device), "--output", "TARGET,FSTYPE,OPTIONS"], capture_output=True)
    filesystems = json.loads(result.stdout).get("filesystems", []) if result.returncode == 0 else []
    if filesystems:
        if len(filesystems) != 1 or filesystems[0]["fstype"] != "vfat" or "rw" not in filesystems[0]["options"].split(","):
            raise ValueError("Selected ESP must have one writable vfat mount")
        yield Path(filesystems[0]["target"])
    else:
        if command(["blkid", "--match-tag", "TYPE", "--output", "value", device]).strip() != "vfat":
            raise ValueError("Selected partition is not vfat")
        with tempfile.TemporaryDirectory(prefix="esp-", dir=state) as directory:
            command(["mount", "-t", "vfat", "-o", "umask=0077,nosuid,nodev,noexec", device, directory])
            try:
                yield Path(directory)
            finally:
                command(["umount", directory])


@contextmanager
def locked_esp(config):
    """Share the publication lock and selected ESP across related EFI updates."""
    if os.geteuid() != 0:
        raise ValueError("EFI installation requires root")
    if not re.fullmatch(r"[0-9a-fA-F-]{36}", config["partuuid"]):
        raise ValueError("Expected an explicit ESP PARTUUID")
    if not config["label"] or any(ord(char) < 32 for char in config["label"]):
        raise ValueError("EFI label must be a nonempty single line")
    state = Path("/var/lib/zbm-rs/efi")
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (state / "install.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        with mounted(config, state) as esp:
            yield esp


def deploy(config):
    with locked_esp(config) as esp:
        install(config, esp)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    deploy(json.loads(parser.parse_args().config.read_text()))
