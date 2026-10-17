"""Authorize installed Linux boot pairs using the loader's own planning code.

Does not rebuild kernels/initramfs, change ZFS properties or install an EFI image.
Private keys are passed to signing tools outside Nix. Review the printed plans
with --plan-only before enabling this in a distribution's post-update hook.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("publisher", HERE / "publish-loader.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
command = publisher.command


def publish(source, output, authority_cert):
    """Content-addressed pair: publish CMS before JSON, never a partial payload."""
    output.mkdir(parents=True, exist_ok=True)
    for directory in source.iterdir():
        destination = output / directory.name
        destination.mkdir(exist_ok=True)
        for payload in directory.glob("*.json"):
            signature = payload.with_suffix(".cms")
            final = destination / payload.name
            final_signature = final.with_suffix(".cms")
            if final.exists() or final_signature.exists():
                if final.exists() and final.read_bytes() != payload.read_bytes():
                    raise ValueError("Existing authorization differs; refusing replacement")
                # CMS includes a signing time, so repeated signing need not
                # produce identical bytes. Accept only a valid existing pair.
                command(["openssl", "cms", "-verify", "-binary", "-inform", "DER", "-noverify", "-nointern",
                         "-certfile", authority_cert, "-in", final_signature, "-content", payload, "-out", os.devnull])
                if final.exists():
                    continue
            for source_file, target in ((signature, final_signature), (payload, final)):
                if target == final_signature and target.exists():
                    continue
                with tempfile.NamedTemporaryFile(dir=destination, prefix=".publish-", delete=False) as stream:
                    temporary = Path(stream.name)
                    try:
                        stream.write(source_file.read_bytes())
                        stream.flush()
                        os.fsync(stream.fileno())
                        os.fchmod(stream.fileno(), 0o644)
                        # An existing authorization must never be overwritten.
                        os.link(temporary, target)
                    finally:
                        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--loader", required=True, type=Path)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--config", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--plan-only", action="store_true")
    parser.add_argument("--allow-snapshot-clones", action="store_true")
    for role in ("ima", "authority"):
        parser.add_argument(f"--{role}-key", type=Path)
        parser.add_argument(f"--{role}-cert", type=Path)
    parser.add_argument("--kernel-cert", type=Path)
    parser.add_argument("--evmctl", type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    # Refuse to authorize an unrelated mounted tree for the requested dataset.
    mount = json.loads(command(["findmnt", "--json", "--target", root, "--output", "SOURCE,FSTYPE"]))
    if mount["filesystems"] != [{"source": args.dataset, "fstype": "zfs"}]:
        raise ValueError("Root must be mounted from the exact selected ZFS dataset")
    plans = json.loads(command([args.loader.resolve(), "--linux-plans", root, args.dataset, args.config.resolve()]))
    if not plans:
        raise ValueError("No usable Linux boot pairs found")
    print(json.dumps(plans, indent=2), flush=True)
    if args.plan_only:
        return
    required = (args.output, args.kernel_cert, args.evmctl, args.ima_key, args.ima_cert,
                args.authority_key, args.authority_cert)
    if any(value is None for value in required):
        parser.error("Publishing requires --output, --kernel-cert, --evmctl and both signing key/cert pairs")
    for key in (args.ima_key, args.authority_key):
        if key.resolve().is_relative_to(Path("/nix/store")):
            raise ValueError("Private keys must stay outside Nix")
    output = args.output.resolve()
    if not output.is_relative_to(root) or output.is_relative_to(Path("/nix/store")):
        raise ValueError("Authorizations must be written inside the selected root, outside Nix")
    with tempfile.TemporaryDirectory(prefix="zbm-linux-authorize-") as temporary:
        temporary = Path(temporary)
        environment = os.environ | {"LD_LIBRARY_PATH": str(args.evmctl.resolve().parent)}
        for index, plan in enumerate(plans):
            kernel, initrd = Path(plan["kernel"]), Path(plan["initrd"])
            if not all(path.resolve().is_relative_to(root) for path in (kernel, initrd)):
                raise ValueError("Boot pair escaped selected root")
            command(["sbverify", "--cert", args.kernel_cert.resolve(), kernel])
            staged = temporary / f"initrd-{index}"
            shutil.copyfile(initrd, staged)
            publisher.ima_sign(staged, args.ima_key.resolve(), args.ima_cert.resolve(), args.evmctl.resolve(), environment)
            arguments = temporary / "arguments.json"
            arguments.write_text(json.dumps(plan["cmdline"]))
            root_argument = [value for value in plan["cmdline"] if value.startswith(("root=", "zfs="))]
            if len(root_argument) != 1 or not root_argument[0].endswith(args.dataset):
                raise ValueError("Ambiguous planned root")
            prefix = root_argument[0][:-len(args.dataset)]
            cmd = ["uv", "run", "--no-project", "python", HERE / "authorize-boot.py",
                   "--kernel", kernel, "--initramfs", staged, "--ima-signature", Path(f"{staged}.sig"),
                   "--arguments", arguments, "--dataset", args.dataset, "--root-prefix", prefix,
                   "--authority-key", args.authority_key.resolve(), "--authority-cert", args.authority_cert.resolve(),
                   "--output", temporary / "authorizations"]
            if args.allow_snapshot_clones:
                cmd.append("--allow-snapshot-clones")
            command(cmd)
        publish(temporary / "authorizations", output, args.authority_cert.resolve())


if __name__ == "__main__":
    main()
