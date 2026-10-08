"""Sign a disposable generated NixOS fixture outside Nix, never its source store."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("publisher", REPO / "nix/publish-loader.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
command = publisher.command


def unpack_tar(archive, root):
    """Extract regular files before symlinks, never follow a guest link on host."""
    if archive.stat().st_size > 16 * 1024**3:
        raise ValueError("Oversized target fixture")
    with tarfile.open(archive) as source:
        members, entries = source.getmembers(), {}
        if len(members) > 250000 or sum(m.size for m in members) > 16 * 1024**3:
            raise ValueError("Oversized target fixture members")
        for member in members:
            path = Path(member.name)
            if path == Path("."):
                continue
            if path.is_absolute() or ".." in path.parts or any(ord(c) < 32 for c in member.name):
                raise ValueError("Unsafe target fixture path")
            if path in entries or not (member.isdir() or member.isfile() or member.issym()) or member.size > 1024**3:
                raise ValueError("Unsupported/duplicate target fixture member")
            entries[path] = member
        for path in entries:
            if any(parent in entries and not entries[parent].isdir() for parent in path.parents):
                raise ValueError("Target fixture has a non-directory ancestor")
        for path, member in entries.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            if member.isdir():
                target.mkdir(exist_ok=True)
                target.chmod((member.mode & 0o777) | 0o700)
            elif member.isfile():
                with source.extractfile(member) as input_file, target.open("wb") as output:
                    shutil.copyfileobj(input_file, output)
                target.chmod((member.mode & 0o777) | 0o200)
        for path, member in entries.items():
            if member.issym():
                (root / path).symlink_to(member.linkname)


def owned_file(root, guest):
    path = Path(guest)
    if not guest.startswith("/nix/store/") or ".." in path.parts:
        raise ValueError("Expected generated Nix store artifact")
    file = root / guest.lstrip("/")
    if file.is_symlink() or not file.resolve().is_relative_to(root) or not file.is_file():
        raise ValueError("Expected owned regular Nix artifact")
    return file


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("output", type=Path)
    for role in ("kernel", "module", "ima", "authority"):
        parser.add_argument(f"--{role}-key", required=True, type=Path)
        parser.add_argument(f"--{role}-cert", required=True, type=Path)
    parser.add_argument("--sign-file", required=True, type=Path)
    parser.add_argument("--evmctl", required=True, type=Path)
    parser.add_argument("--dataset", default="zbm_fixture/nixos")
    args = parser.parse_args()
    destination = args.output.resolve()
    if destination.exists() or destination.is_relative_to(Path("/nix/store")):
        raise ValueError("Use a fresh deployment outside Nix")
    for role in ("kernel", "module", "ima", "authority"):
        key = getattr(args, f"{role}_key")
        if key.resolve().is_relative_to(Path("/nix/store")):
            raise ValueError("Private keys must stay outside Nix")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".nixos-authorize-", dir=destination.parent) as temporary:
        temporary = Path(temporary)
        root, output = temporary / "root", temporary / "output"
        root.mkdir()
        output.mkdir()
        print("Extracting disposable NixOS root", flush=True)
        unpack_tar(args.fixture / "root.tar", root)
        print("Signing all target modules", flush=True)
        modules = publisher.sign_modules(root, args.module_key.resolve(), args.module_cert.resolve(),
                                         args.sign_file.resolve(), directory=root / "nix/store")
        records, kernels, initrds = [], set(), set()
        environment = os.environ | {"LD_LIBRARY_PATH": str(args.evmctl.resolve().parent)}
        for generation in (1, 2):
            toplevel = (args.fixture / f"generation-{generation}").read_text().strip()
            spec = json.loads(owned_file(root, toplevel + "/boot.json").read_text())["org.nixos.bootspec.v1"]
            kernel, initrd = owned_file(root, spec["kernel"]), owned_file(root, spec["initrd"])
            if kernel not in kernels:
                signed = temporary / "kernel.signed"
                command(["sbsign", "--key", args.kernel_key.resolve(), "--cert", args.kernel_cert.resolve(), "--output", signed, kernel])
                command(["sbverify", "--cert", args.kernel_cert.resolve(), signed])
                shutil.copyfile(signed, kernel)
                kernels.add(kernel)
            if initrd not in initrds:
                print("Signing target initramfs modules and final IMA bytes", flush=True)
                raw = temporary / "initrd.cpio"
                with raw.open("wb") as stream:
                    command(["zstd", "-q", "-d", "-c", initrd], output=stream)
                initrd_root = temporary / "initrd-root"
                initrd_root.mkdir()
                publisher.unpack(raw, initrd_root)
                # NixOS stores actual module files under its copied Nix closure.
                publisher.sign_modules(initrd_root, args.module_key.resolve(), args.module_cert.resolve(),
                                       args.sign_file.resolve(), directory=initrd_root)
                publisher.archive(initrd_root, raw)
                with initrd.open("wb") as stream:
                    command(["zstd", "-q", "-T1", "-19", "-c", raw], output=stream)
                publisher.ima_sign(initrd, args.ima_key.resolve(), args.ima_cert.resolve(), args.evmctl.resolve(), environment)
                shutil.rmtree(initrd_root)
                initrds.add(initrd)
            params = spec["kernelParams"]
            lsms = next((value.removeprefix("lsm=") for value in params if value.startswith("lsm=")), "").split(",")
            if not {"ima", "lockdown"}.issubset(lsms):
                raise ValueError("Protected target fixture must retain ima and lockdown")
            arguments = [f"init={spec['init']}", *[value for value in params if not value.startswith("root=")],
                         f"root={args.dataset}", "rootfstype=zfs", "rootflags=defaults", "rw", "rd.fstab=no"]
            argument_file = temporary / "arguments.json"
            argument_file.write_text(json.dumps(arguments))
            result = json.loads(command(["uv", "run", "--no-project", "python", REPO / "nix/authorize-boot.py",
                "--kernel", kernel, "--initramfs", initrd, "--ima-signature", Path(f"{initrd}.sig"),
                "--arguments", argument_file, "--authority-key", args.authority_key.resolve(),
                "--authority-cert", args.authority_cert.resolve(), "--dataset", args.dataset,
                "--root-prefix", "root=", "--allow-snapshot-clones", "--output", root / "boot/zbm-rs/authorizations"]))
            records.append({"generation": generation, "artifact_id": result["artifact_id"],
                            "argument_id": result["argument_id"], "arguments": arguments})
            (output / f"generation-{generation}").write_text(toplevel + "\n")
        print("Packing owner-signed target fixture", flush=True)
        command(["tar", "--numeric-owner", "--owner=0", "--group=0", "-cf", output / "root.tar", "-C", root, "."])
        with (output / "root.tar").open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        (output / "authorization-manifest.json").write_text(json.dumps({"disposable_fixture": True,
            "dataset": args.dataset, "modules_signed": len(modules), "authorizations": records,
            "archive_sha256": digest}, indent=2) + "\n")
        output.rename(destination)
    print(json.dumps({"fixture": str(destination), "modules_signed": len(modules)}))


if __name__ == "__main__":
    main()
