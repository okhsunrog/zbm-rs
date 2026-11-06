"""Live metadata, durability and access checks on disposable guest devices.

Executed by check-luks2-resume.py with its verified device/plan/probe helpers.
All raw header changes below are deliberate fault injection in the VM only.
"""
import fcntl
import mmap
import os
import struct


def read_page(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECT)
    try:
        with mmap.mmap(-1, 4096) as page:
            assert os.preadv(fd, [page], 0) == 4096
            return page[:]
    finally:
        os.close(fd)


def write_page(path, header):
    # The same documented snapshot selection needed for raw writes to active
    # swap also serializes this test-only fault injection with kernel sleep.
    snapshot = os.open("/dev/snapshot", os.O_RDONLY)
    fd = os.open(path, os.O_RDWR | os.O_DIRECT)
    try:
        dev = os.fstat(fd).st_rdev
        major, minor = os.major(dev), os.minor(dev)
        encoded = (minor & 255) | (major << 8) | ((minor & ~255) << 12)
        try:
            fcntl.ioctl(snapshot, 0x400c330d, struct.pack("=qI", 0, encoded))
        except OSError as failure:
            assert failure.errno == 19, failure  # Inactive swap needs no exemption.
        with mmap.mmap(-1, 4096) as page:
            page[:] = header
            assert os.pwritev(fd, [page], 0) == 4096
            os.fsync(fd)
    finally:
        os.close(fd)
        os.close(snapshot)


def edit_record(path, **fields):
    header = bytearray(read_page(path))
    length = int.from_bytes(header[16:20], "little")
    record = json.loads(subprocess.run(
        ["zstd", "-dc"], input=header[20:20 + length], capture_output=True, check=True).stdout)
    record.update(fields)
    packed = subprocess.run(["zstd", "-qc"], input=json.dumps(record).encode(),
                            capture_output=True, check=True).stdout
    assert len(packed) <= 1004
    header[16:20] = len(packed).to_bytes(4, "little")
    header[20:1024] = packed + bytes(1004 - len(packed))
    write_page(path, header)


def live_checks(device_spec, path):
    spec_path.write_text(json.dumps(device_spec))
    run("mkswap", path)
    original = read_page(path)
    # Payload immediately after the header must survive even the 4Kn RMW path.
    payload = bytes(range(256)) * 16
    with open(path, "r+b", buffering=0) as disk:
        disk.seek(4096)
        disk.write(payload)
        os.fsync(disk.fileno())
    run("swapon", path)
    try:
        probe("record", str(plan_path), ok=False)  # Exclusive path stays closed.
        probe("record-live", str(plan_path))
        recorded = read_page(path)
        assert recorded[1024:] == original[1024:]
        assert json.loads(probe("status"))["state"] == "inactive"
        with open("/dev/snapshot", "rb"):
            probe("record-live", str(plan_path), ok=False)
            probe("finish", ok=False)
            # The kernel lease must also exclude native hibernation itself.
            run("sh", "-c", "echo disk > /sys/power/state", ok=False)
        assert read_page(path) == recorded
        # A runtime metadata handle still serializes with all ordinary readers
        # and exclusive writers through the block inode's flock.
        with open(path, "rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_SH | fcntl.LOCK_NB)
            probe("record-live", str(plan_path), ok=False)
            probe("finish", ok=False)
        probe("finish")  # Early wake / failed hibernate needs no metadata write.
        assert read_page(path) == recorded

        # A still-present image must never be rewritten by a live operation.
        pending = bytearray(recorded)
        pending[-20:] = b"SWAPSPACE2S1SUSPEND\0"
        write_page(path, pending)
        probe("record-live", str(plan_path), ok=False)
        probe("finish", ok=False)
        probe("cancel", json.loads(probe("status"))["session"]["id"], ok=False)
        assert read_page(path) == pending
        write_page(path, recorded)

        # Emulate a consumed image's durable flag without touching swap data.
        edit_record(path, attempting=True, boot_id="12345678-1234-1234-1234-123456789abc")
        wrong_boot = read_page(path)
        probe("finish", ok=False, error="Only the restored kernel")
        probe("record-live", str(plan_path), ok=False)
        assert read_page(path) == wrong_boot
        edit_record(path, boot_id=Path("/proc/sys/kernel/random/boot_id").read_text().strip())
        probe("finish")
        assert json.loads(probe("status"))["state"] == "inactive"
        assert read_page(path)[1024:] == original[1024:]

        # An interrupted metadata update must retain conservative boot status.
        damaged = bytearray(read_page(path))
        damaged[20:128] = bytes(108)
        write_page(path, damaged)
        assert json.loads(probe("status"))["state"] == "unavailable"
        probe("record-live", str(plan_path), ok=False)
        probe("finish", ok=False)
        assert read_page(path) == damaged
        write_page(path, recorded)
        with open(path, "rb", buffering=0) as disk:
            disk.seek(4096)
            assert disk.read(4096) == payload
    finally:
        run("swapoff", path)
    print("Live boot-record, wrong-boot, torn-record, locking and payload checks passed:", path)


live_checks(spec, "/dev/mapper/resume-swap")
live_checks({"type": "partition", "partuuid": plain_uuid}, "/dev/vdb3")

# Real 4096-byte logical sectors exercise direct full-sector RMW, not just a
# unit-tested alignment calculation. The loop disk and GPT belong to this VM.
run("truncate", "-s", "128M", "/root/boot-record-4kn.img")
loop = run("losetup", "--find", "--show", "--partscan", "--sector-size", "4096", "/root/boot-record-4kn.img")
try:
    run("sgdisk", "--new=1:256:0", loop)
    # sgdisk already asks the kernel to rescan. A second BLKRRPART races udev's
    # partition probe and can spuriously fail with EBUSY.
    run("udevadm", "settle")
    partition = loop + "p1"
    assert run("blockdev", "--getss", partition) == "4096"
    uuid4k = run("blkid", "-s", "PARTUUID", "-o", "value", partition)
    live_checks({"type": "partition", "partuuid": uuid4k}, partition)
finally:
    run("losetup", "--detach", loop)

# Inject acknowledged-but-lost writes and EIO under a plain dedicated device.
# Drop-write failures must be caught by direct readback; a consumed attempt
# must remain protected. The synthetic PARTUUID alias exists only in the VM.
fault_uuid = "feedface-1234-1234-1234-123456789abc"
fault_link = Path("/dev/disk/by-partuuid") / fault_uuid
spec_path.write_text(json.dumps({"type": "partition", "partuuid": plain_uuid}))
probe("record", str(plan_path))
baseline = read_page("/dev/vdb3")
sectors = run("blockdev", "--getsz", "/dev/vdb3")
run("modprobe", "dm-flakey")
for fault in ("drop_writes", "error_writes"):
    run("dmsetup", "create", "record-fault", "--table",
        f"0 {sectors} flakey /dev/vdb3 0 0 3600 1 {fault}")
    run("udevadm", "settle")
    fault_link.symlink_to("/dev/mapper/record-fault")
    spec_path.write_text(json.dumps({"type": "partition", "partuuid": fault_uuid}))
    try:
        probe("record-live", str(plan_path), ok=False)
        assert read_page("/dev/vdb3") == baseline
        edit_record("/dev/vdb3", attempting=True)
        attempted = read_page("/dev/vdb3")
        probe("finish", ok=False)
        assert read_page("/dev/vdb3") == attempted
        assert json.loads(probe("status"))["state"] == "unavailable"
    finally:
        fault_link.unlink()
        run("udevadm", "settle")
        run("dmsetup", "remove", "--retry", "record-fault")
        write_page("/dev/vdb3", baseline)
    print("Boot-record durability fault was rejected:", fault)
spec_path.write_text(json.dumps(spec))
