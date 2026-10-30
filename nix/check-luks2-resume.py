"""Real encrypted block-device scenarios, only inside the disposable VM."""
import hashlib
import json
from pathlib import Path
import subprocess


def run(*args, ok=True, error=None):
    result = subprocess.run(args, capture_output=True, text=True, timeout=15)
    if ok:
        assert result.returncode == 0, (args, result.stderr)
    else:
        assert result.returncode != 0, args
        if error is not None:
            assert error in result.stderr, result.stderr
    return result.stdout.strip()


run("sgdisk", "--zap-all", "--new=1:2048:+256M", "--new=2:0:+256M", "--new=3:0:+128M", "/dev/vdb")
run("udevadm", "settle")
Path("/root/test-key").write_text("disposable-luks2-resume-test")
Path("/root/wrong-key").write_text("wrong-disposable-key")
uuid = "ca55a30a-636e-4e69-9e68-bb36bf380408"
for partition in ("/dev/vdb1", "/dev/vdb2"):
    run("cryptsetup", "luksFormat", "--batch-mode", "--type", "luks2", "--pbkdf", "pbkdf2", "--pbkdf-force-iterations", "1000", "--uuid", uuid, "--key-file", "/root/test-key", partition)
partuuid = run("blkid", "-s", "PARTUUID", "-o", "value", "/dev/vdb1")
spec = {"type": "luks2", "partuuid": partuuid, "luks_uuid": uuid, "mapping": "resume-swap"}
spec_path = Path("/root/swap.json")
spec_path.write_text(json.dumps(spec))


def probe(action, *args, ok=True, error=None):
    return run("resume-device-probe", str(spec_path), action, *args, ok=ok, error=error)


def header_hash():
    with open("/dev/vdb1", "rb") as disk:
        return hashlib.sha256(disk.read(16 * 1024 * 1024)).hexdigest()


original_header = header_hash()
probe("status", ok=False)  # Closed is unknown, never ordinary empty swap.
run("cryptsetup", "open", "--key-file", "/root/wrong-key", "/dev/vdb1", "resume-swap", ok=False)
probe("status", ok=False)
run("cryptsetup", "open", "--key-file", "/root/test-key", "/dev/vdb2", "resume-swap")
run("mkswap", "/dev/mapper/resume-swap")
probe("status", ok=False)  # Same LUKS UUID, wrong physical partition.
run("cryptsetup", "close", "resume-swap")
sectors = run("blockdev", "--getsz", "/dev/vdb1")
run("dmsetup", "create", "resume-swap", "--uuid", "CRYPT-LUKS2-" + uuid.replace("-", "") + "-resume-swap",
    "--table", f"0 {sectors} linear /dev/vdb1 0")
probe("status", ok=False)  # A forged DM UUID does not make a linear target crypt.
run("dmsetup", "remove", "resume-swap")
mixed_table = f"0 8 linear /dev/vdb1 0\n8 {int(sectors) - 8} crypt aes-xts-plain64 {bytes(range(64)).hex()} 0 /dev/vdb1 8"
run("dmsetup", "create", "resume-swap", "--uuid", "CRYPT-LUKS2-" + uuid.replace("-", "") + "-resume-swap",
    "--table", mixed_table)
probe("status", ok=False, error="not a single active crypt target")
run("dmsetup", "remove", "resume-swap")  # Inspect all targets, not just crypt segments.
run("cryptsetup", "open", "--key-file", "/root/test-key", "/dev/vdb1", "resume-swap")
run("mkswap", "/dev/mapper/resume-swap")
assert json.loads(probe("status"))["state"] == "inactive"
spec_path.write_text(json.dumps(spec | {"luks_uuid": "12345678-1234-1234-1234-123456789abc"}))
probe("status", ok=False)
spec_path.write_text(json.dumps(spec))
run("dmsetup", "suspend", "--noflush", "resume-swap")
try:
    probe("status", ok=False)
finally:
    run("dmsetup", "resume", "resume-swap")

Path("/root/kernel").write_bytes(b"disposable-kernel-A")
plan = {
    "target": {"dataset": "tank/root", "label": "test", "root": "/run/root", "snapshot": None, "mountpoint": None,
               "generation": 1, "specialisation": None, "toplevel": "/nix/store/test-system", "backend": {"kind": "Nixos"}, "issue": None,
               "inputs": {"kernel": "/root/kernel", "initrd": None, "init": "/init", "kernel_params": []}},
    "kernel": "/root/kernel", "initrd": None, "cmdline": ["root=ZFS=tank/root"]}
plan_path = Path("/root/plan.json")
plan_path.write_text(json.dumps(plan))
run("mkswap", "/dev/vdb3")
plain_uuid = run("blkid", "-s", "PARTUUID", "-o", "value", "/dev/vdb3")
spec_path.write_text(json.dumps({"type": "partition", "partuuid": plain_uuid}))
assert json.loads(probe("status"))["state"] == "inactive"
probe("record", str(plan_path))
assert json.loads(probe("status"))["state"] == "inactive"
spec_path.write_text(json.dumps({"type": "partition", "partuuid": partuuid}))
probe("status", ok=False)  # The outer LUKS partition is not plain swap.
spec_path.write_text(json.dumps(spec))
run("cryptsetup", "close", "resume-swap")
run("cryptsetup", "open", "--readonly", "--key-file", "/root/test-key", "/dev/vdb1", "resume-swap")
assert json.loads(probe("status"))["state"] == "inactive"
probe("record", str(plan_path), ok=False)
run("cryptsetup", "close", "resume-swap")
run("cryptsetup", "open", "--key-file", "/root/test-key", "/dev/vdb1", "resume-swap")
run("swapon", "/dev/mapper/resume-swap")
try:
    probe("record", str(plan_path), ok=False)  # Active swap cannot be written.
finally:
    run("swapoff", "/dev/mapper/resume-swap")
probe("record", str(plan_path))
with open("/dev/mapper/resume-swap", "r+b", buffering=0) as swap:
    swap.seek(4096)
    swap.write(b"preserved-payload")
    swap.seek(4096 - 20)
    swap.write(b"SWAPSPACE2S1SUSPEND\0")
pending = json.loads(probe("status"))
assert pending["state"] == "pending", pending
consent = pending["session"]["id"]
probe("verify", str(plan_path))
Path("/root/kernel").write_bytes(b"disposable-kernel-B")
probe("verify", str(plan_path), ok=False)
probe("cancel", "stale-consent", ok=False)
probe("record", str(plan_path), ok=False)
probe("restore", ok=False)  # Never restore from an ordinary booted system.
probe("cancel", consent)
assert json.loads(probe("status"))["state"] == "inactive"
with open("/dev/mapper/resume-swap", "r+b", buffering=0) as swap:
    swap.seek(4096)
    assert swap.read(17) == b"preserved-payload"
    swap.seek(4096 - 10)
    swap.write(b"corrupted!")
probe("status", ok=False)
assert header_hash() == original_header, "LUKS header was changed"
run("cryptsetup", "close", "resume-swap")
probe("status", ok=False)
assert header_hash() == original_header
print("LUKS2 identity, closed/wrong mappings, active swap, record/handoff/cancel and corrupt image checks passed")
