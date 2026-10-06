import hashlib
import json
from pathlib import Path
import sys
out = Path(sys.argv[1])
kernel, zfs, test, profile = sys.argv[2:6]
policy = json.loads(Path(sys.argv[6]).read_text())
sizes = {name: (out / path).stat().st_size for name, path in
         [("efi", "esp/EFI/BOOT/BOOTX64.EFI"), ("initramfs", "initramfs.img"), ("kernel", "vmlinuz")]}
budget = policy["test" if test == "yes" else profile]
assert sizes["efi"] <= budget["maxEfiBytes"], f"EFI size budget exceeded: {sizes}"
if budget["baselineEfiBytes"] is not None:
    allowed = budget["baselineEfiBytes"] * (100 + policy["allowedGrowthPercent"]) // 100 + policy["growthAllowanceBytes"]
    assert sizes["efi"] <= allowed, f"Unexplained EFI growth beyond baseline: {sizes}"
(out / "sizes.json").write_text(json.dumps(sizes, indent=2) + "\n")
(out / "manifest.json").write_text(json.dumps({
    "kernel": kernel, "zfs": zfs, "test_ssh": test == "yes", "profile": profile,
    "builder": "nix", "compression": sys.argv[7], "sizes": sizes,
    "kernel_sha256": hashlib.sha256((out / "vmlinuz").read_bytes()).hexdigest(),
    "initramfs_sha256": hashlib.sha256((out / "initramfs.img").read_bytes()).hexdigest()
}, indent=2) + "\n")
