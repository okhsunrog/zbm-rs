import hashlib
import json
from pathlib import Path
import sys
import importlib.util
sys.dont_write_bytecode = True
out = Path(sys.argv[1])
kernel, zfs, test, profile = sys.argv[2:6]
policy = json.loads(Path(sys.argv[6]).read_text())
sizes = {name: (out / path).stat().st_size for name, path in
         [("efi", "esp/EFI/BOOT/BOOTX64.EFI"), ("initramfs", "initramfs.img"), ("kernel", "vmlinuz")]}
microcode = out / "microcode.cpio"
if microcode.exists():
    assert microcode.read_bytes()[:6] == b"070701", "Microcode requires an uncompressed newc archive"
    sizes["microcode"] = microcode.stat().st_size
protected = sys.argv[8] == "yes"
budget = policy[("protected-test" if protected else "test") if test == "yes" else profile]
assert sizes["efi"] <= budget["maxEfiBytes"], f"EFI size budget exceeded: {sizes}"
if budget["baselineEfiBytes"] is not None:
    allowed = budget["baselineEfiBytes"] * (100 + policy["allowedGrowthPercent"]) // 100 + policy["growthAllowanceBytes"]
    assert sizes["efi"] <= allowed, f"Unexplained EFI growth beyond baseline: {sizes}"
(out / "sizes.json").write_text(json.dumps(sizes, indent=2) + "\n")
resume = json.loads((out / "loader-config.json").read_text())["resume"]
capabilities = json.loads((out / "resume-capabilities.json").read_text())
policy_id = None
if resume.get("device") is not None or resume.get("swap_partuuid") is not None:
    spec = importlib.util.spec_from_file_location("protected_hibernation", Path(__file__).with_name("protected-hibernation.py"))
    validator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validator)
    # Identity only, not a protected-hibernation authorization for rescue.
    policy_id = validator.policy_id(resume | {"role": "resume"})
(out / "manifest.json").write_text(json.dumps({
    "kernel": kernel, "zfs": zfs, "test_ssh": test == "yes", "profile": profile,
    "builder": "nix", "libc": json.loads((out / "runtime-audit.json").read_text())["libc"], "compression": sys.argv[7], "sizes": sizes,
    "kernel_sha256": hashlib.sha256((out / "vmlinuz").read_bytes()).hexdigest(),
    "efi_sha256": hashlib.sha256((out / "esp/EFI/BOOT/BOOTX64.EFI").read_bytes()).hexdigest(),
    "resume_capabilities": capabilities,
    "resume_role": resume["role"],
    "security_mode": json.loads((out / "loader-config.json").read_text())["security"]["mode"],
    "resume_policy_sha256": policy_id,
    "initramfs_sha256": hashlib.sha256((out / "initramfs.img").read_bytes()).hexdigest(),
    "microcode_sha256": hashlib.sha256(microcode.read_bytes()).hexdigest() if microcode.exists() else None
}, indent=2) + "\n")
