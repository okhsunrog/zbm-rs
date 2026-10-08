"""Replay real firmware/userspace SHA-256 measurements against captured TPM PCRs."""
import hashlib
import json
from pathlib import Path
import struct
import sys


def firmware_events(data):
    if not 32 <= len(data) <= 16 * 1024 * 1024:
        raise ValueError("Invalid bounded firmware log")
    pcr, kind = struct.unpack_from("<II", data)
    size = struct.unpack_from("<I", data, 28)[0]
    spec, offset = data[32:32+size], 32+size
    if pcr != 0 or kind != 3 or len(spec) != size or spec[:16] != b"Spec ID Event03\0":
        raise ValueError("Expected TCG2 Spec ID event")
    count = struct.unpack_from("<I", spec, 24)[0]
    if not 1 <= count <= 16 or len(spec) < 28+count*4:
        raise ValueError("Invalid TPM algorithm table")
    algorithms = dict(struct.unpack_from("<HH", spec, 28+index*4) for index in range(count))
    if len(algorithms) != count or algorithms.get(0x000b) != 32 or any(not 1 <= size <= 64 for size in algorithms.values()):
        raise ValueError("Unsupported/ambiguous TPM digest sizes")
    while offset < len(data):
        pcr, kind, count = struct.unpack_from("<III", data, offset)
        offset += 12
        if pcr >= 24 or not 1 <= count <= len(algorithms):
            raise ValueError("Invalid TPM event header")
        digests = {}
        for _ in range(count):
            algorithm = struct.unpack_from("<H", data, offset)[0]
            offset += 2
            if algorithm not in algorithms or algorithm in digests:
                raise ValueError("Unknown/duplicate event digest")
            size = algorithms[algorithm]
            digests[algorithm] = data[offset:offset+size]
            if len(digests[algorithm]) != size:
                raise ValueError("Truncated digest")
            offset += size
        size = struct.unpack_from("<I", data, offset)[0]
        offset += 4
        if offset+size > len(data):
            raise ValueError("Truncated TPM event")
        offset += size
        if kind != 3 and pcr == 11:
            yield digests[0x000b]


def extend(value, digest):
    if len(value) != 32 or len(digest) != 32:
        raise ValueError("Expected SHA-256 PCR values")
    return hashlib.sha256(value + digest).digest()


def verify(run):
    pcr11 = bytes(32)
    events = list(firmware_events((run / "tpm-firmware.bin").read_bytes()))
    if not events:
        raise ValueError("No firmware PCR11 measurements")
    for digest in events:
        pcr11 = extend(pcr11, digest)
    pcr15 = bytes.fromhex((run / "tpm-pcr15-initial.txt").read_text().strip())
    prepared = []
    for record in (run / "tpm-userspace.jsonseq").read_text().split("\x1e"):
        if not record.strip():
            continue
        event = json.loads(record)
        pcr = event.get("pcr")
        if pcr != 15:
            raise ValueError("Loader must not extend PCR11 phases or NvPCRs")
        word = event["content"]["string"]
        digests = [d["digest"] for d in event["digests"] if d["hashAlg"] == "sha256"]
        if len(digests) != 1 or digests[0] != hashlib.sha256(word.encode()).hexdigest():
            raise ValueError("Event digest differs from recorded measurement")
        digest = bytes.fromhex(digests[0])
        prepared.append(word)
        pcr15 = extend(pcr15, digest)
    if pcr11.hex() != (run / "tpm-pcr11.txt").read_text().strip().lower():
        raise ValueError("PCR11 differs from measured UKI without loader phases")
    target = (run / "tpm-target.json").read_bytes()
    expected = "zbm-rs:target-prepared:v1:" + hashlib.sha256(target).hexdigest()
    if prepared != [expected] or pcr15.hex() != (run / "tpm-pcr15-prepared.txt").read_text().strip().lower():
        raise ValueError("PCR15 does not replay from the exact verified final plan")
    return {"passed": True, "firmware_pcr11_events": len(events),
            "loader_pcr11_events": 0, "pcr11": pcr11.hex(),
            "pcr15": pcr15.hex(), "target_event": expected}


def main():
    print(json.dumps(verify(Path(sys.argv[1]))))


if __name__ == "__main__":
    main()
