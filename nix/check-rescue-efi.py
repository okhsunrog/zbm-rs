"""Exercise independent rescue publication on the VM's disposable FAT ESP."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("rescue", Path(__file__).with_name("rescue-efi.py"))
rescue = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rescue)
work = Path("/root/efi-test")
esp = Path("/mnt/esp")
active = esp / "EFI/zbm-rs/rescue.EFI"
initial = active.read_bytes()
main = (esp / "EFI/zbm-rs/zbm-rs.EFI").read_bytes()
image = work / "candidate.efi"
image.write_bytes(initial + b"candidate-v2")
manifest_path = work / "manifest.json"
manifest = {"efi_sha256": rescue.digest(image), "security_mode": "off", "test_ssh": False,
            "resume_role": "cancel-only", "resume_policy_sha256": "ab" * 32,
            "resume_capabilities": {"version": 1, "record_protocol": 2,
                                    "durable_attempt_tracking": True, "cancel_only": True}}
manifest_path.write_text(json.dumps(manifest))
plan = {"version": 2, "image": str(image), "manifest": str(manifest_path),
        "resumePolicySha256": "ab" * 32,
        "efi": {"partuuid": "ed5d4b72-0aac-48d3-8f89-43362d952ca0",
                "path": "EFI/zbm-rs/rescue.EFI", "label": "zbm-rs (rescue)", "certificate": None},
        "candidate": {"path": "EFI/zbm-rs/rescue-candidate.EFI", "label": "zbm-rs (rescue candidate)"}}


def rejects(operation):
    try:
        operation()
    except (ValueError, FileNotFoundError):
        return
    raise AssertionError("Invalid rescue publication succeeded")


rescue.publish(plan, esp)
candidate = esp / plan["candidate"]["path"]
receipt = Path(str(candidate) + ".json")
assert active.read_bytes() == initial and candidate.read_bytes() == image.read_bytes()
staged = receipt.read_bytes()
tested_hash = hashlib.sha256(image.read_bytes()).hexdigest()
rejects(lambda: rescue.publish(plan, esp, promote="00" * 32))
receipt.unlink()  # Power loss after image publication, before receipt publication.
rejects(lambda: rescue.publish(plan, esp, promote=tested_hash))
receipt.write_bytes(staged)
candidate.write_bytes(candidate.read_bytes() + b"changed-after-test")
rejects(lambda: rescue.publish(plan, esp, promote=tested_hash))
assert active.read_bytes() == initial
rescue.publish(plan, esp)
rescue.publish(plan, esp, promote=tested_hash)
assert active.read_bytes() == image.read_bytes()
accepted = active.read_bytes()
image.write_bytes(initial + b"candidate-v3")
manifest["efi_sha256"] = rescue.digest(image)
manifest_path.write_text(json.dumps(manifest))
rescue.publish(plan, esp)
assert active.read_bytes() == accepted and candidate.read_bytes() == image.read_bytes()
rejects(lambda: rescue.publish(plan, esp, promote=tested_hash))

for field, invalid in (("record_protocol", 1), ("durable_attempt_tracking", False), ("cancel_only", False)):
    changed = copy.deepcopy(manifest)
    changed["resume_capabilities"][field] = invalid
    manifest_path.write_text(json.dumps(changed))
    rejects(lambda: rescue.publish(plan, esp))
for field, invalid in (("resume_policy_sha256", "cd" * 32), ("resume_role", "resume"),
                       ("efi_sha256", "00" * 32), ("security_mode", "enforce"), ("test_ssh", True)):
    manifest_path.write_text(json.dumps(manifest | {field: invalid}))
    rejects(lambda: rescue.publish(plan, esp))
manifest_path.write_text(json.dumps(manifest))
rejects(lambda: rescue.publish(plan | {"image": str(work / "main.efi")}, esp))
rejects(lambda: rescue.publish(plan | {"candidate": plan["candidate"] | {"certificate": "bypass"}}, esp))
assert active.read_bytes() == accepted
assert (esp / "EFI/zbm-rs/zbm-rs.EFI").read_bytes() == main
assert (esp / "EFI/foreign/existing.EFI").read_text() == "keep"
print("Independent rescue stage/promotion, interruptions, binding and isolation passed")
