"""Public test inputs for the real Rust verified executor; no signing keys in guest."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).parents[2] / "nix/publish-loader.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
command = publisher.command


def digest(file):
    return {"size": file.stat().st_size, "sha256": hashlib.sha256(file.read_bytes()).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--key", required=True, type=Path)
    parser.add_argument("--cert", required=True, type=Path)
    parser.add_argument("--evmctl", required=True, type=Path)
    parser.add_argument("--ima-key", required=True, type=Path)
    parser.add_argument("--ima-cert", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True)
    key, cert = args.key.resolve(), args.cert.resolve()
    environment = os.environ | {"LD_LIBRARY_PATH": str(args.evmctl.resolve().parent)}
    with tempfile.TemporaryDirectory(prefix="zbm-security-fixture-") as temporary:
        temporary = Path(temporary)
        source, second = temporary / "source", temporary / "second"
        source.mkdir()
        publisher.unpack(args.image / "root.cpio", source)
        (second / "bin").mkdir(parents=True)
        (second / "nix/store").mkdir(parents=True)
        for store in (source / "nix/store").iterdir():
            if "busybox" in store.name or "glibc" in store.name:
                shutil.copytree(store, second / "nix/store" / store.name, symlinks=True)
        (second / "bin/busybox").symlink_to(os.readlink(source / "bin/busybox"))
        (second / "bin/sh").symlink_to("busybox")
        (second / "init").write_text('''#!/bin/sh
/bin/busybox --install -s /bin
mkdir -p /proc /sys /dev
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /sys/firmware/efi/efivars
mount -t efivarfs efivarfs /sys/firmware/efi/efivars
echo ZBM_TEST_SECOND_KERNEL_BOOTED
cat /proc/cmdline
echo ZBM_TEST_SECOND_BOOT_ID=$(cat /proc/sys/kernel/random/boot_id)
echo ZBM_TEST_SECOND_SECURE_BOOT=$(od -An -tu1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c | awk '{print $5}')
poweroff -f
''')
        (second / "init").chmod(0o755)
        publisher.archive(second, temporary / "second.cpio")
        with (temporary / "second.img").open("wb") as stream:
            command(["gzip", "-n", "-c", temporary / "second.cpio"], output=stream)
        publisher.ima_sign(temporary / "second.img", args.ima_key.resolve(), args.ima_cert.resolve(), args.evmctl.resolve(), environment)
        signature = (temporary / "second.img.sig").read_bytes()
        command(["sbsign", "--key", key, "--cert", cert, "--output", temporary / "kernel", args.image / "vmlinuz"])
        shared = args.output / "shared"
        shared.mkdir()
        shutil.copyfile(temporary / "kernel", shared / "kernel")
        shutil.copyfile(args.image / "vmlinuz", shared / "unsigned-kernel")
        shutil.copyfile(temporary / "second.img", shared / "initramfs.img")
        shutil.copyfile(temporary / "second.img", shared / "changed-initramfs.img")
        with (shared / "changed-initramfs.img").open("ab") as file:
            file.write(b"changed after signing")
        for case in ("valid", "wrong-ima", "unsigned-kernel", "wrong-cms", "wrong-arguments", "changed-initramfs"):
            root = args.output / case
            authdir = root / "boot/zbm-rs/authorizations"
            authdir.mkdir(parents=True)
            kernel_name = "unsigned-kernel" if case == "unsigned-kernel" else "kernel"
            kernel, initramfs = digest(shared / kernel_name), digest(shared / "initramfs.img")
            ima_signature = signature if case != "wrong-ima" else signature[:-1] + bytes([signature[-1] ^ 1])
            authorization = {
                "version": 1, "architecture": "x86_64", "kernel": kernel, "initramfs": initramfs,
                "fixed_arguments": ["console=ttyS0,115200", "zbm.fixture=second"],
                "root": {"source_dataset": "tank/root", "argument_index": 1, "prefix": "root=ZFS=", "allow_snapshot_clones": False},
                "initramfs_ima_signature": list(ima_signature),
            }
            pair = hashlib.sha256(f'{kernel["sha256"]}:{initramfs["sha256"]}'.encode()).hexdigest()
            payload = authdir / f"{pair}.json"
            cms = payload.with_suffix(".cms")
            if case == "valid":
                # Exercise the real owner authorization publisher, then demand
                # the fixture's expected policy rather than duplicating it.
                arguments = temporary / "arguments.json"
                arguments.write_text(json.dumps(["console=ttyS0,115200", "root=ZFS=tank/root", "zbm.fixture=second"]))
                command(["uv", "run", "--no-project", "python",
                         Path(__file__).parents[2] / "nix/authorize-boot.py",
                         "--kernel", shared / kernel_name, "--initramfs", shared / "initramfs.img",
                         "--ima-signature", temporary / "second.img.sig", "--arguments", arguments,
                         "--authority-key", key, "--authority-cert", cert, "--dataset", "tank/root",
                         "--output", authdir])
                if json.loads(payload.read_text()) != authorization:
                    raise ValueError("Owner publisher produced a different fixture policy")
            else:
                payload.write_text(json.dumps(authorization, separators=(",", ":")))
                command(["openssl", "cms", "-sign", "-binary", "-md", "sha256", "-in", payload,
                         "-signer", cert, "-inkey", key, "-outform", "DER", "-out", cms])
            guest = f"/fixtures/{case}"
            plan = {
                "target": {"dataset": "tank/root", "generation": None, "label": case, "root": guest,
                           "toplevel": None, "backend": {"kind": "Linux", "rootprefix": "root=ZFS=", "commandline": []},
                           "snapshot": None, "mountpoint": "/", "issue": None,
                           "inputs": {"kernel": "/kernel", "initrd": "/initramfs.img", "init": None, "kernel_params": []}},
                "kernel": f"/fixtures/shared/{kernel_name}", "initrd": "/fixtures/shared/initramfs.img",
                "cmdline": ["console=ttyS0,115200", "root=ZFS=tank/root", "zbm.fixture=second"],
            }
            if case == "wrong-arguments":
                plan["cmdline"].append("init=/bin/sh")
            elif case == "wrong-cms":
                data = cms.read_bytes()
                cms.write_bytes(data[:-1] + bytes([data[-1] ^ 1]))
            elif case == "changed-initramfs":
                plan["initrd"] = "/fixtures/shared/changed-initramfs.img"
            (root / "plan.json").write_text(json.dumps(plan))
    print(json.dumps({"fixture_payload": str(args.output), "cases": 6}))


if __name__ == "__main__":
    main()
