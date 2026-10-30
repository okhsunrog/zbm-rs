"""Corrupt only the ECDSA trailer of a disposable 7.2 x86_64 VM image.

The fixture uses 4 KiB pages and hibernate=nocompress. Follow the kernel's
swap-map chain instead of searching RAM contents for a coincidental magic.
Never use this helper on a host swap device.
"""

import os
import struct
import sys

PAGE = 4096
with open(sys.argv[1], "r+b", buffering=0) as device:
    header = device.read(PAGE)
    assert header[-10:] == b"S1SUSPEND\0", "no saved VM image"
    image, flags = struct.unpack_from("<QI", header, PAGE - 32)
    assert flags & 2, "fixture requires an uncompressed image"
    seen = set()
    last = None
    while image:
        assert image not in seen and len(seen) < 4096, "invalid swap map"
        seen.add(image)
        device.seek(image * PAGE)
        entries = struct.unpack("<512Q", device.read(PAGE))
        pages = [entry for entry in entries[:-1] if entry]
        if pages:
            last = pages[-1]
        image = entries[-1]
    assert last is not None, "empty swap map"
    device.seek(last * PAGE)
    trailer = device.read(PAGE)
    assert trailer[:8] == b"LNXHSIG1", "last image page is not a TPM signature"
    assert not any(trailer[136:]), "unexpected signature trailer layout"
    # Keep provenance, public key and image payload intact; corrupt r only.
    offset = last * PAGE + 8 + 32 + 32 + 31
    device.seek(offset)
    device.write(bytes([trailer[103] ^ 1]))
    os.fsync(device.fileno())
    device.seek(offset)
    assert device.read(1) == bytes([trailer[103] ^ 1])
    device.seek(0)
    assert device.read(PAGE) == header, "swap header changed"
    print("corrupted one ECDSA signature byte; swap header unchanged")
