"""Stage an independent unsigned rescue; promote only an explicitly tested hash."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import sys
import tempfile

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("efi_installer", Path(__file__).with_name("install-efi.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate(plan):
    if plan["version"] != 2 or plan["efi"]["certificate"] is not None:
        raise ValueError("Expected an unsigned rescue lifecycle plan")
    image = Path(plan["image"])
    manifest = json.loads(Path(plan["manifest"]).read_text())
    capabilities = manifest.get("resume_capabilities", {})
    if (manifest.get("resume_role") != "cancel-only"
            or manifest.get("security_mode") != "off"
            or manifest.get("test_ssh") is not False
            or capabilities.get("version") != 1
            or capabilities.get("record_protocol") != 2
            or capabilities.get("durable_attempt_tracking") is not True
            or capabilities.get("cancel_only") is not True):
        raise ValueError("Rescue must support cancellation of durable resume attempts")
    if (not re.fullmatch(r"[0-9a-f]{64}", plan["resumePolicySha256"])
            or manifest.get("resume_policy_sha256") != plan["resumePolicySha256"]):
        raise ValueError("Rescue must use the declared swap and credential policy")
    installer.verify(image, None)
    image_hash = digest(image)
    if manifest.get("efi_sha256") != image_hash:
        raise ValueError("Rescue manifest does not describe the exact EFI image")
    active = PurePosixPath(plan["efi"]["path"])
    if set(plan["candidate"]) != {"path", "label"}:
        raise ValueError("Candidate may change only the independent path and label")
    candidate = PurePosixPath(plan["candidate"]["path"])
    for path in (active, candidate):
        if path.is_absolute() or ".." in path.parts or not str(path).startswith("EFI/"):
            raise ValueError("Rescue destinations must be relative EFI/ paths")
    if active == candidate or plan["efi"]["label"] == plan["candidate"]["label"]:
        raise ValueError("Candidate and known-good rescue must remain independent")
    return {"version": 1, "image_sha256": image_hash,
            "resume_policy_sha256": plan["resumePolicySha256"],
            "manifest_sha256": digest(Path(plan["manifest"]))}


def receipt_path(esp, relative):
    path = esp / (relative + ".json")
    if path.is_symlink() or not path.parent.resolve().is_relative_to(esp.resolve()):
        raise ValueError("Rescue receipt escapes the selected ESP")
    return path


def write_receipt(path, receipt):
    temporary = None
    try:
        fd, name = tempfile.mkstemp(prefix=".zbm-rescue-", dir=path.parent)
        temporary = Path(name)
        with os.fdopen(fd, "w") as stream:
            json.dump(receipt, stream, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        temporary = None
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def publish(plan, esp, *, promote=None):
    receipt = validate(plan)
    candidate = plan["efi"] | plan["candidate"] | {"image": plan["image"]}
    candidate_receipt = receipt_path(esp, candidate["path"])
    if promote is None:
        installer.install(candidate, esp)
        # An interrupted image/receipt pair is deliberately not promotable.
        write_receipt(candidate_receipt, receipt)
        print("Candidate staged; boot and test it before promoting SHA256 " + receipt["image_sha256"])
        return
    if not re.fullmatch(r"[0-9a-f]{64}", promote) or promote != receipt["image_sha256"]:
        raise ValueError("Promotion requires the exact tested candidate SHA256")
    if json.loads(candidate_receipt.read_text()) != receipt:
        raise ValueError("Staged candidate does not match this immutable rescue plan")
    candidate_image = esp / candidate["path"]
    if candidate_image.is_symlink() or digest(candidate_image) != promote:
        raise ValueError("Staged candidate changed after testing")
    active_receipt = receipt_path(esp, plan["efi"]["path"])
    installer.install(plan["efi"] | {"image": str(candidate_image)}, esp)
    write_receipt(active_receipt, receipt)
    print("Known-good rescue promoted: " + promote)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path)
    parser.add_argument("--promote", metavar="TESTED_SHA256")
    args = parser.parse_args()
    plan_path = args.plan.resolve(strict=True)
    if os.geteuid() != 0 or plan_path.parent != Path("/nix/store"):
        raise ValueError("Expected root and an immutable Nix rescue plan")
    plan = json.loads(plan_path.read_text())
    validate(plan)
    with installer.locked_esp(plan["efi"]) as esp:
        publish(plan, esp, promote=args.promote)


if __name__ == "__main__":
    main()
