"""Re-sign disposable test UKIs with absent/invalid PCR policy for negative tests."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile

spec = importlib.util.spec_from_file_location("publisher", Path(__file__).parents[2] / "nix/publish-loader.py")
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--case", choices=("missing", "invalid"), required=True)
    parser.add_argument("--efi-key", type=Path, required=True)
    parser.add_argument("--efi-cert", type=Path, required=True)
    parser.add_argument("--ukify", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((args.image / "manifest.json").read_text())
    if not manifest["test_ssh"] or not manifest["deployment"]["disposable_fixture"]:
        raise ValueError("Negative policies are confined to disposable test images")
    if args.output.exists() or args.output.resolve().is_relative_to(Path("/nix/store")):
        raise ValueError("Use a fresh fixture directory outside Nix")
    with tempfile.TemporaryDirectory(prefix="zbm-tpm-negative-") as temporary:
        temporary = Path(temporary)
        unsigned = temporary / "unsigned.efi"
        sections = []
        if args.case == "invalid":
            policy = json.loads((args.image / "pcr-signature.json").read_text())
            for bank in policy.values():
                for signature in bank:
                    signature["ref"] = "unapproved-fixture-phase"
            bad = temporary / "invalid.json"
            bad.write_text(json.dumps(policy))
            sections = ["--section", f".pcrsig:@{bad}"]
        publisher.command([args.ukify, "build", "--linux", args.image / "vmlinuz",
                           "--initrd", args.image / "initramfs.img", "--uname", manifest["kernel"],
                           "--cmdline", f"@{args.image / 'cmdline'}", "--os-release", f"@{args.image / 'os-release'}",
                           "--pcrpkey", args.image / "pcr-public.pem", "--stub", args.image / "stub.efi",
                           "--output", unsigned, *sections])
        before = publisher.pe_sections(args.image / "esp/EFI/BOOT/BOOTX64.EFI")
        after = publisher.pe_sections(unsigned)
        if any(before.get(f".{name}") != after.get(f".{name}") for name in publisher.MEASURED_SECTIONS):
            raise ValueError("Negative policy must preserve all measured inputs")
        shutil.copytree(args.image, args.output)
        efi = args.output / "esp/EFI/BOOT/BOOTX64.EFI"
        efi.chmod(0o600)
        publisher.command(["sbsign", "--key", args.efi_key, "--cert", args.efi_cert, "--output", efi, unsigned])
        publisher.command(["sbverify", "--cert", args.efi_cert, efi])
    manifest["deployment"]["negative_pcr_fixture"] = args.case
    manifest["sizes"]["efi"] = efi.stat().st_size
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(json.dumps({"fixture": str(args.output), "case": args.case}))


if __name__ == "__main__":
    main()
