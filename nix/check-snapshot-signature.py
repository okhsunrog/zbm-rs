"""Refuse an unsigned VM image through /dev/snapshot without attempting restore.

Pinned to the disposable Linux 7.2 x86_64 fixture with 4 KiB pages and an
uncompressed image. Only read its swap; write two header pages to the snapshot
stream. Never freeze processes or issue SNAPSHOT_ATOMIC_RESTORE.
"""
import errno
import os
import struct
import sys

PAGE = 4096
with open(sys.argv[1], "rb", buffering=0) as swap:
    before = swap.read(PAGE)
    assert before[-10:] == b"S1SUSPEND\0", "no saved VM image"
    image, flags = struct.unpack_from("<QI", before, PAGE - 32)
    assert image and flags & 2, "fixture requires an uncompressed image"
    swap.seek(image * PAGE)
    first = struct.unpack("<Q", swap.read(8))[0]
    assert first, "missing image header page"
    swap.seek(first * PAGE)
    header = bytearray(swap.read(PAGE))
    # swsusp_info: six 65-byte UTS slots (or architecture header), version,
    # num_physpages, cpus, image_pages, pages, size, sig_pages. Pinned ABI only.
    assert len(header) == PAGE and struct.unpack_from("<Q", header, 440)[0] == 1
    struct.pack_into("<Q", header, 440, 0)
    fd = os.open("/dev/snapshot", os.O_WRONLY | os.O_CLOEXEC)
    try:
        assert os.write(fd, header) == PAGE
        try:
            # Loading/validating the first page happens on the next write.
            os.write(fd, bytes(PAGE))
        except OSError as error:
            assert error.errno == errno.EKEYREJECTED, error
        else:
            raise AssertionError("Unsigned snapshot stream was accepted")
    finally:
        os.close(fd)
    swap.seek(0)
    assert swap.read(PAGE) == before, "saved image header changed"
print("Unsigned snapshot stream rejected; no RAM restoration attempted")
