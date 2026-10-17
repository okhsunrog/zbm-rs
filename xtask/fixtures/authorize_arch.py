"""Sign explicit disposable Arch fixture inputs outside the guest and Nix."""
import argparse
import importlib.util
import json
import os
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("publisher", REPO / "nix/publish-loader.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)
command = publisher.command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", type=Path)
    parser.add_argument("profile", type=Path)
    args = parser.parse_args()
    profile = json.loads(args.profile.read_text())
    if profile.get("disposable_fixture") is not True:
        raise ValueError("Only an explicitly disposable signing profile is permitted")
    paths = {key: Path(profile[key]).resolve(strict=True) for key in
             ("kernel_key", "kernel_cert", "ima_key", "ima_cert", "authority_key", "authority_cert", "evmctl")}
    # sbverify alone accepts an expired certificate; Linux also checks the
    # PE signing time against its validity window. Fail before signing.
    command(["openssl", "x509", "-in", paths["kernel_cert"], "-checkend", "0", "-noout"])
    root = args.inputs.resolve()
    kernel, signed, initrd = root / "kernel", root / "kernel.signed", root / "initrd"
    command(["sbsign", "--key", paths["kernel_key"], "--cert", paths["kernel_cert"],
             "--output", signed, kernel])
    command(["sbverify", "--cert", paths["kernel_cert"], signed])
    publisher.ima_sign(initrd, paths["ima_key"], paths["ima_cert"], paths["evmctl"],
                       os.environ | {"LD_LIBRARY_PATH": str(paths["evmctl"].parent)})
    plans = json.loads((root / "plans.json").read_text())
    if len(plans) != 1 or plans[0]["target"]["dataset"] != "zbm_arch/arch0/root":
        raise ValueError("Expected one disposable Arch boot pair")
    arguments = root / "arguments.json"
    arguments.write_text(json.dumps(plans[0]["cmdline"]))
    command(["uv", "run", "--no-project", "python", REPO / "nix/authorize-boot.py",
             "--kernel", signed, "--initramfs", initrd, "--ima-signature", root / "initrd.sig",
             "--arguments", arguments, "--dataset", "zbm_arch/arch0/root", "--root-prefix", "zfs=",
             "--allow-snapshot-clones", "--authority-key", paths["authority_key"],
             "--authority-cert", paths["authority_cert"], "--output", root / "authorizations"])


if __name__ == "__main__":
    main()
