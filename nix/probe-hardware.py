"""Explicit, read-only host probe. The resulting JSON is the only hardware build input."""
import json
from pathlib import Path
import platform
import sys

def probe_modules(sys_root=Path("/sys")):
    modules = {"usbhid", "hid_generic", "atkbd", "i8042"}

    def driver(device):
        link = device / "driver/module"
        if link.exists():
            modules.add(link.resolve().name)

    for device in (sys_root / "bus/pci/devices").glob("*"):
        kind = (device / "class").read_text().strip()
        if kind.startswith("0x01") or kind.startswith("0x0c03"):
            driver(device)
    for device in (sys_root / "bus/usb/devices").glob("*"):
        driver(device)

    # A controller's dependencies do not include the SCSI disk frontend. Follow
    # block devices too, including their parent transport/controller drivers.
    boundary = sys_root.resolve()
    for block in (sys_root / "class/block").glob("*"):
        device = (block / "device").resolve()
        if not device.is_relative_to(boundary):
            continue
        subsystem = device / "subsystem"
        kind = device / "type"
        if (subsystem.exists() and subsystem.resolve().name == "scsi"
                and kind.exists() and kind.read_text().strip() in {"0", "7", "14"}):
            # Also retain the frontend when it is built into the probed kernel:
            # it may be modular in the kernel selected for the image.
            modules.add("sd_mod")
        while device != boundary:
            driver(device)
            device = device.parent
    return modules


def main():
    manifest = {"schemaVersion": 1, "probeKernel": platform.release(),
                "modules": sorted(probe_modules()), "firmwarePaths": []}
    Path(sys.argv[1]).write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
