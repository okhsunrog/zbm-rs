"""Keep verifiable anonymous pages swapped out across a real VM hibernation."""
import hashlib
import mmap
from pathlib import Path
import time

size = 640 * 1024 * 1024
memory = mmap.mmap(-1, size, flags=mmap.MAP_PRIVATE | mmap.MAP_ANONYMOUS)
for offset in range(0, size, 4096):
    memory[offset:offset + 4096] = hashlib.sha256(str(offset).encode()).digest() * 128


def checksum():
    digest = hashlib.sha256()
    for offset in range(0, size, 1024 * 1024):
        digest.update(memory[offset:offset + 1024 * 1024])
    return digest.hexdigest()


expected = checksum()
Path("/run/boot-record-pressure-ready").write_text(expected)
round_number = 0
while True:
    request = Path("/run/boot-record-pressure-check")
    if request.exists():
        request.unlink()
        assert checksum() == expected, "Anonymous data changed across hibernation"
        round_number += 1
        Path("/run/boot-record-pressure-verified").write_text(str(round_number))
    time.sleep(0.2)
