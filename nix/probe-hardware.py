"""Explicit, read-only host probe. The resulting JSON is the only hardware build input."""
import json
from pathlib import Path
import platform
import sys

modules = {"usbhid", "hid_generic", "atkbd", "i8042"}
for device in Path("/sys/bus/pci/devices").glob("*"):
    kind = (device / "class").read_text().strip()
    if kind.startswith("0x01") or kind.startswith("0x0c03"):
        link = device / "driver/module"
        if link.exists(): modules.add(link.resolve().name)
for device in Path("/sys/bus/usb/devices").glob("*"):
    link = device / "driver/module"
    if link.exists(): modules.add(link.resolve().name)
manifest = {"schemaVersion": 1, "probeKernel": platform.release(),
            "modules": sorted(modules), "firmwarePaths": []}
Path(sys.argv[1]).write_text(json.dumps(manifest, indent=2) + "\n")
