"""Publish owner authorization for exact boot bytes and ordered arguments."""
import argparse
import hashlib
import json
from pathlib import Path
import os
import re
import subprocess
import tempfile

def argument_id(arguments):
    if len(arguments) > 65:
        raise ValueError("Too many authorization lookup arguments")
    digest, roots = hashlib.sha256(b"zbm-rs:arguments:v1\0"), 0
    for argument in arguments:
        if not argument or len(argument.encode()) > 4096 or "\0" in argument:
            raise ValueError("Invalid authorization lookup argument")
        if argument.startswith("root="):
            roots += 1
            argument = next(prefix for prefix in ("root=ZFS=", "root=zfs:", "root=") if argument.startswith(prefix))
        token = argument.encode()
        digest.update(len(token).to_bytes(4, "big"))
        digest.update(token)
    if roots != 1:
        raise ValueError("Authorization lookup requires one typed root")
    return digest.hexdigest()


def artifact(path):
    with path.open("rb") as file:
        size = os.fstat(file.fileno()).st_size
        if not path.is_file() or not 0 < size <= 1024**3:
            raise ValueError("Expected bounded regular boot artifact")
        digest, total = hashlib.sha256(), 0
        while block := file.read(65536):
            total += len(block)
            if total > size:
                raise ValueError("Artifact changed while hashing")
            digest.update(block)
        if total != size:
            raise ValueError("Artifact changed while hashing")
        return {"size": size, "sha256": digest.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("kernel", "initramfs", "ima-signature", "arguments", "authority-key", "authority-cert", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--root-prefix", choices=("root=ZFS=", "root=zfs:", "root="), default="root=ZFS=")
    parser.add_argument("--allow-snapshot-clones", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.:-]*(/[A-Za-z0-9_.:-]+)*", args.dataset):
        raise ValueError("Expected explicit valid ZFS source dataset")
    arguments = json.loads(args.arguments.read_text())
    if not isinstance(arguments, list) or not 1 <= len(arguments) <= 65 or any(
            not isinstance(value, str) or not value or len(value.encode()) > 4096 or "\0" in value for value in arguments):
        raise ValueError("Expected bounded ordered argument array")
    roots = [index for index, value in enumerate(arguments) if value.startswith("root=")]
    if len(roots) != 1 or arguments[roots[0]] != args.root_prefix + args.dataset:
        raise ValueError("One typed root must match the authorized source dataset")
    if args.root_prefix == "root=" and "rootfstype=zfs" not in arguments:
        raise ValueError("Unprefixed ZFS root needs an explicit signed filesystem type")
    signature = args.ima_signature.read_bytes()
    if not 9 <= len(signature) <= 8192 or signature[:3] != bytes([3, 2, 4]):
        raise ValueError("Expected detached IMA v2 SHA-256 signature")
    kernel, initramfs = artifact(args.kernel), artifact(args.initramfs)
    root_index = roots[0]
    fixed = arguments[:root_index] + arguments[root_index+1:]
    authorization = {"version": 1, "architecture": "x86_64", "kernel": kernel, "initramfs": initramfs,
                     "fixed_arguments": fixed, "root": {"source_dataset": args.dataset, "argument_index": root_index,
                     "prefix": args.root_prefix, "allow_snapshot_clones": args.allow_snapshot_clones},
                     "initramfs_ima_signature": list(signature)}
    name = hashlib.sha256(f'{kernel["sha256"]}:{initramfs["sha256"]}'.encode()).hexdigest()
    args.output.mkdir(parents=True, exist_ok=True)
    index = argument_id(arguments)
    directory = args.output / name
    directory.mkdir(exist_ok=True)
    payload, cms = directory / f"{index}.json", directory / f"{index}.cms"
    if payload.exists() or cms.exists():
        raise ValueError("Authorization already exists; use a fresh staging directory")
    with tempfile.TemporaryDirectory(prefix="zbm-authorize-", dir=args.output) as temporary:
        temporary = Path(temporary)
        (temporary / "payload").write_text(json.dumps(authorization, separators=(",", ":")))
        subprocess.run(["openssl", "cms", "-sign", "-binary", "-md", "sha256", "-in", str(temporary / "payload"),
                        "-signer", str(args.authority_cert), "-inkey", str(args.authority_key), "-outform", "DER",
                        "-out", str(temporary / "signature")], check=True)
        subprocess.run(["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-noverify", "-nointern",
                        "-certfile", str(args.authority_cert), "-in", str(temporary / "signature"), "-content",
                        str(temporary / "payload"), "-out", os.devnull], check=True)
        os.rename(temporary / "payload", payload)
        os.rename(temporary / "signature", cms)
    print(json.dumps({"authorization": str(payload), "cms": str(cms), "artifact_id": name, "argument_id": index}))


if __name__ == "__main__":
    main()
