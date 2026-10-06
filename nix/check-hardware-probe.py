"""Exercise controller and block-frontend discovery using an isolated sysfs fixture."""
import importlib.util
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True

specification = importlib.util.spec_from_file_location(
    "hardware_probe", Path(__file__).with_name("probe-hardware.py"))
probe = importlib.util.module_from_spec(specification)
specification.loader.exec_module(probe)

with tempfile.TemporaryDirectory(prefix="zbm-hardware-probe-") as folder:
    root = Path(folder)

    def driver(device, module):
        target = root / "module" / module
        target.mkdir(parents=True, exist_ok=True)
        (device / "driver").mkdir(parents=True, exist_ok=True)
        (device / "driver/module").symlink_to(target)

    def disk(name, controller, frontend=True):
        device = controller / "host0/target0/disk0"
        device.mkdir(parents=True)
        (root / "bus/scsi").mkdir(parents=True, exist_ok=True)
        (device / "subsystem").symlink_to(root / "bus/scsi")
        (device / "type").write_text("0\n")
        block = root / "class/block" / name
        block.mkdir(parents=True)
        (block / "device").symlink_to(device)
        if frontend:
            driver(device, "sd_mod")

    pci = root / "bus/pci/devices/ahci0"
    pci.mkdir(parents=True)
    (pci / "class").write_text("0x010601\n")
    driver(pci, "ahci")
    disk("sda", pci)
    modules = probe.probe_modules(root)
    assert {"ahci", "sd_mod"} <= modules, modules

    usb = root / "bus/usb/devices/storage0"
    usb.mkdir(parents=True)
    driver(usb, "usb_storage")
    disk("sdb", usb, frontend=False)
    # Remove the modular frontend: both disks must still retain sd_mod by their
    # SCSI device type when the development kernel has it built in.
    (pci / "host0/target0/disk0/driver/module").unlink()
    modules = probe.probe_modules(root)
    assert {"ahci", "usb_storage", "sd_mod"} <= modules, modules
    assert {"usbhid", "hid_generic", "atkbd", "i8042"} <= modules

print("Hardware probe checks passed: AHCI/USB storage, disk frontend, built-in frontend")
