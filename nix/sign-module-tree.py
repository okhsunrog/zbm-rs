"""Sign a public copy of a Nix module tree, without keys entering a derivation."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).with_name("publish-loader.py"))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--key", required=True, type=Path)
    parser.add_argument("--cert", required=True, type=Path)
    parser.add_argument("--sign-file", required=True, type=Path)
    parser.add_argument("--kernel", required=True, type=Path)
    args = parser.parse_args()
    source, output = args.source.resolve(strict=True), args.output.resolve()
    if not source.is_relative_to("/nix/store") or output.is_relative_to("/nix/store") or output.exists():
        raise ValueError("Use a store source and fresh output outside the store")
    if args.key.resolve().is_relative_to("/nix/store"):
        raise ValueError("Private key must stay outside Nix")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".module-sign-", dir=output.parent) as temporary:
        temporary = Path(temporary)
        tree = temporary / "tree"
        # aggregateModules has owned metadata and store symlinks, not build trees.
        shutil.copytree(source, tree, symlinks=False)
        # copytree preserves the store root's read-only mode too. The manifest
        # is published at this root after all module signatures are verified.
        tree.chmod(0o755)
        for directory in tree.rglob("*"):
            if directory.is_dir():
                directory.chmod(0o755)
            else:
                directory.chmod(0o644)
        signed = publisher.sign_modules(tree, args.key.resolve(), args.cert.resolve(),
                                         args.sign_file.resolve(), directory=tree)
        versions = [p.name for p in (tree / "lib/modules").iterdir() if p.is_dir()]
        if len(versions) != 1 or not args.kernel.resolve(strict=True).is_relative_to("/nix/store"):
            raise ValueError("Expected one module ABI and its Nix store kernel")
        (tree / "zbm-module-manifest.json").write_text(json.dumps({"version": 1,
            "kernel_store_path": str(args.kernel.resolve()), "module_version": versions[0],
            "signed_modules": len(signed),
            "certificate_sha256": hashlib.sha256(publisher.public_der(args.cert.resolve())).hexdigest()}) + "\n")
        tree.rename(output)
    print(f"Signed and verified {len(signed)} modules in {output}")


if __name__ == "__main__":
    main()
