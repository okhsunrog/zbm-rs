"""Corrupt an ECDSA signature or RAM payload in a disposable 7.2 x86_64 VM image.

The fixture uses 4 KiB pages and hibernate=nocompress. Follow the kernel's
swap-map chain instead of searching RAM contents for a coincidental magic.
Never use this helper on a host swap device.
"""

import argparse
import os
import struct

PAGE = 4096
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("device")
parser.add_argument("mode", nargs="?", default="signature", choices=["signature", "payload"])
args = parser.parse_args()
with open(args.device, "r+b", buffering=0) as device:
    header = device.read(PAGE)
    assert header[-10:] == b"S1SUSPEND\0", "no saved VM image"
    image, flags = struct.unpack_from("<QI", header, PAGE - 32)
    assert flags & 2, "fixture requires an uncompressed image"
    seen = set()
    mapped = []
    while image:
        assert image not in seen and len(seen) < 4096, "invalid swap map"
        seen.add(image)
        device.seek(image * PAGE)
        entries = struct.unpack("<512Q", device.read(PAGE))
        pages = [entry for entry in entries[:-1] if entry]
        mapped.extend(pages)
        image = entries[-1]
    assert mapped, "empty swap map"
    last = mapped[-1]
    device.seek(last * PAGE)
    trailer = device.read(PAGE)
    assert trailer[:8] == b"LNXHSIG1", "last image page is not a TPM signature"
    assert not any(trailer[136:]), "unexpected signature trailer layout"
    if args.mode == "signature":
        # Keep provenance, public key and image payload intact; corrupt r only.
        offset = last * PAGE + 8 + 32 + 32 + 31
    else:
        device.seek(mapped[0] * PAGE)
        info = device.read(PAGE)
        # Pinned x86_64 swsusp_info: image_pages, pages, size, sig_pages.
        copies, total, _, signatures = struct.unpack_from("<QQQQ", info, 416)
        metadata = total - copies - signatures - 1
        assert total == len(mapped) and copies > 0 and metadata > 0 and signatures == 1
        # Change actual copied RAM, leaving header/PFN metadata and the valid
        # ECDSA trailer intact. Uncompressed mode isolates image authentication.
        offset = mapped[1 + metadata] * PAGE + 77
    device.seek(offset)
    original = device.read(1)
    assert len(original) == 1
    changed = bytes([original[0] ^ 1])
    device.seek(offset)
    device.write(changed)
    os.fsync(device.fileno())
    device.seek(offset)
    assert device.read(1) == changed
    device.seek(0)
    assert device.read(PAGE) == header, "swap header changed"
    if args.mode == "payload":
        device.seek(last * PAGE)
        assert device.read(PAGE) == trailer, "signature trailer changed"
    print(f"corrupted one {args.mode} byte; swap header unchanged")
