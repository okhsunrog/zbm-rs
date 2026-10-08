"""Validate public IMA certificate constraints before image packaging."""
import argparse
from pathlib import Path
import subprocess


def validate(path):
    data = path.read_bytes()
    if len(data) > 65536 or b"PRIVATE KEY" in data:
        raise ValueError("Expected a public X.509 certificate without private material")
    result = subprocess.run(["openssl", "x509", "-in", str(path), "-noout", "-ext",
                             "basicConstraints,keyUsage"], capture_output=True, check=True)
    usage = result.stdout.decode()
    if "CA:TRUE" in usage or "Certificate Sign" in usage or "Digital Signature" not in usage:
        raise ValueError("IMA certificate must be non-CA with digitalSignature usage")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("certificate", type=Path)
    validate(parser.parse_args().certificate)
