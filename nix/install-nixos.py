"""NixOS installBootLoader hook: prepare and authorize generations outside /nix/store.

No EFI/NVRAM changes and no loader image rebuild. Source Bootspec/store files
are never modified. Runtime keys are paths in root-owned configuration only.
"""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile

HERE = Path(__file__).resolve().parent


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


publisher = load("publisher", "publish-loader.py")
authorizer = load("authorizer", "authorize-linux.py")
command = publisher.command


def replace_link(destination, target):
    temporary = destination.with_name(f".{destination.name}.{os.getpid()}")
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(target)
    os.replace(temporary, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("system", type=Path)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise ValueError("Generation installation requires root")
    config = json.loads(args.config.read_text())
    system = args.system.resolve(strict=True)
    if system.parent != Path("/nix/store"):
        raise ValueError("Expected a NixOS system toplevel in /nix/store")
    document_bytes = (system / "boot.json").read_bytes()
    if len(document_bytes) > 1024 * 1024:
        raise ValueError("Oversized Bootspec")
    document = json.loads(document_bytes)
    spec = document["org.nixos.bootspec.v1"]
    if Path(spec["toplevel"]).resolve() != system or document.get("org.nixos.extra-initrd.v1") is not None:
        raise ValueError("Mismatched toplevel or unsupported extra initrd")
    # nixos-enter retains the outer mount prefixes: identify / by device ID.
    device = os.stat("/").st_dev
    mounts = command(["findmnt", "-rn", "-t", "zfs", "-o", "MAJ:MIN,SOURCE,FSROOT"]).decode().splitlines()
    sources = [line.split()[1].split("[", 1)[0] for line in mounts
               if line.split()[0] == f"{os.major(device)}:{os.minor(device)}" and line.split()[2] == "/"]
    if len(set(sources)) != 1 or not sources[0].startswith(config["pool"] + "/"):
        raise ValueError("Refusing publication outside the selected ZFS pool")
    dataset = sources[0]
    Path("/boot").mkdir(exist_ok=True)
    if os.stat("/boot").st_dev != device:
        raise ValueError("Keep /boot and /nix in the selected root dataset")
    base = Path("/boot/zbm-rs")
    base.mkdir(parents=True, exist_ok=True)
    with (base / ".publish.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        install(config, system, spec, document_bytes, dataset, base)


def install(config, system, spec, document_bytes, dataset, base):
    bundles, generations = base / "bundles", base / "nixos"
    bundles.mkdir(mode=0o700, exist_ok=True)
    generations.mkdir(exist_ok=True)
    destination = generations / system.name
    if destination.exists() and not destination.is_symlink():
        raise ValueError("Refusing to replace a foreign generation directory")
    previous = os.readlink(destination) if destination.is_symlink() else None
    stage = Path(tempfile.mkdtemp(prefix=system.name + ".", dir=bundles))
    committed = False
    linked = False
    try:
        for name in ("kernel", "initrd"):
            source = Path(spec[name]).resolve(strict=True)
            if not source.is_relative_to("/nix/store") or not source.is_file():
                raise ValueError("Bootspec artifact must be a regular Nix store file")
            shutil.copyfile(source, stage / name)
            (stage / name).chmod(0o600)
        if appender := spec.get("initrdSecrets"):
            appender = Path(appender).resolve(strict=True)
            if not appender.is_relative_to("/nix/store"):
                raise ValueError("initrdSecrets must be a store program")
            command([appender, stage / "initrd"])
        signing = config.get("signing")
        if signing:
            for role in ("kernel", "ima", "authority"):
                if Path(signing[role + "Key"]).resolve().is_relative_to("/nix/store"):
                    raise ValueError("Private signing keys must stay outside Nix")
            command(["sbsign", "--key", signing["kernelKey"], "--cert", signing["kernelCertificate"],
                     "--output", stage / "kernel.signed", stage / "kernel"])
            os.replace(stage / "kernel.signed", stage / "kernel")
            command(["sbverify", "--cert", signing["kernelCertificate"], stage / "kernel"])
            environment = os.environ | {"LD_LIBRARY_PATH": str(Path(signing["evmctl"]).resolve().parent)}
            publisher.ima_sign(stage / "initrd", Path(signing["imaKey"]), Path(signing["imaCertificate"]),
                               Path(signing["evmctl"]), environment)
        (stage / "prepared.json").write_text(json.dumps({"version": 1,
            "bootspec_sha256": hashlib.sha256(document_bytes).hexdigest()}) + "\n")
        # A transient incomplete entry cannot pass the protected boot verifier.
        # Restore the previous prepared entry on every failure, including signing.
        replace_link(destination, Path("../bundles") / stage.name)
        linked = True
        plans = json.loads(command([config["loader"], "--nixos-plans", "/", dataset,
                                    config["loaderConfig"], system]))
        if len(plans) != 1:
            raise ValueError("Expected one native NixOS boot plan")
        if signing:
            arguments = stage / "arguments.json"
            arguments.write_text(json.dumps(plans[0]["cmdline"]))
            roots = [arg for arg in plans[0]["cmdline"] if arg.startswith("root=")]
            if len(roots) != 1 or not roots[0].endswith(dataset):
                raise ValueError("Expected exactly one explicit selected root")
            staged_authorizations = stage / "authorizations"
            command(["uv", "run", "--offline", "--no-project", "python", HERE / "authorize-boot.py",
                "--kernel", stage / "kernel", "--initramfs", stage / "initrd",
                "--ima-signature", stage / "initrd.sig", "--arguments", arguments,
                "--dataset", dataset, "--root-prefix", roots[0][:-len(dataset)],
                "--allow-snapshot-clones", "--authority-key", signing["authorityKey"],
                "--authority-cert", signing["authorityCertificate"], "--output", staged_authorizations])
            authorizer.publish(staged_authorizations, base / "authorizations", Path(signing["authorityCertificate"]))
        gcroots = Path("/nix/var/nix/gcroots/zbm-rs")
        gcroots.mkdir(parents=True, exist_ok=True)
        replace_link(gcroots / system.name, system)
        command(["sync", "-f", base])
        committed = True
        print(f"Prepared native NixOS generation {system.name} for {dataset}")
    finally:
        if not committed:
            if linked:
                if previous is None:
                    destination.unlink(missing_ok=True)
                else:
                    replace_link(destination, previous)
            shutil.rmtree(stage)


if __name__ == "__main__":
    main()
