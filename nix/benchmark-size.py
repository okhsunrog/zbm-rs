"""Reproducible size experiments; runs outside image derivations with uv run."""
import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument("mode", choices=["compression", "rust"])
parser.add_argument("--image", type=Path, default=ROOT / "result")
parser.add_argument("--output", type=Path, default=ROOT / "target/size-bench")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)

if args.mode == "compression":
    encoded_input = (args.image / "initramfs.img").read_bytes()
    if encoded_input.startswith(b"\x1f\x8b"):
        blob = gzip.decompress(encoded_input)
    elif encoded_input.startswith(b"\xfd7zXZ\x00"):
        import lzma
        blob = lzma.decompress(encoded_input)
    elif encoded_input.startswith(b"\x28\xb5\x2f\xfd"):
        blob = subprocess.run(["zstd", "-q", "-d", "-c"], input=encoded_input,
            check=True, capture_output=True).stdout
    else:
        raise ValueError("Unknown initramfs compression")
    rows = []
    with tempfile.TemporaryDirectory(prefix="zbm-compress-") as folder:
        source = Path(folder) / "initramfs.cpio"
        source.write_bytes(blob)
        for name, command in [
            ("gzip-9", ["gzip", "-n", "-9", "-c"]),
            ("xz-6", ["xz", "--check=crc32", "-6", "-c"]),
            ("zstd-3", ["zstd", "-q", "-T1", "-3", "-c"]),
            ("zstd-10", ["zstd", "-q", "-T1", "-10", "-c"]),
            ("zstd-15", ["zstd", "-q", "-T1", "-15", "-c"]),
            ("zstd-19", ["zstd", "-q", "-T1", "-19", "-c"]),
        ]:
            start = time.monotonic()
            result = subprocess.run([*command, str(source)], check=True, capture_output=True)
            encoded = result.stdout
            elapsed = time.monotonic() - start
            start = time.monotonic()
            if name.startswith("gzip"):
                decoded = gzip.decompress(encoded)
            elif name.startswith("xz"):
                import lzma
                decoded = lzma.decompress(encoded)
            else:
                decoded = subprocess.run(["zstd", "-q", "-d", "-c"], input=encoded, check=True, capture_output=True).stdout
            assert decoded == blob
            rows.append({"name": name, "bytes": len(encoded), "compress_secs": round(elapsed, 3),
                "decompress_secs": round(time.monotonic() - start, 3)})
            print(json.dumps(rows[-1]), flush=True)
    (args.output / "compression.json").write_text(json.dumps({"cpio_bytes": len(blob), "tools": {name: subprocess.check_output([name, "--version"], text=True).splitlines()[0] for name in ["gzip", "xz", "zstd"]}, "rows": rows}, indent=2) + "\n")
else:
    variants = [("baseline", "3", "thin", "16"), ("thin-cgu1", "3", "thin", "1"),
        ("fat-cgu1", "3", "fat", "1"), ("s-fat-cgu1", "s", "fat", "1"), ("z-fat-cgu1", "z", "fat", "1")]
    rows = []
    for name, opt, lto, units in variants:
        env = os.environ.copy()
        env.update(CARGO_PROFILE_RELEASE_OPT_LEVEL=opt, CARGO_PROFILE_RELEASE_LTO=lto,
            CARGO_PROFILE_RELEASE_CODEGEN_UNITS=units, CARGO_TARGET_DIR=str(args.output / "cargo"))
        env.pop("ZBM_RS_CONFIG", None)
        start = time.monotonic()
        with (args.output / f"{name}.log").open("w") as log:
            subprocess.run(["cargo", "build", "--release", "--locked", "-p", "zbm-rs"], cwd=ROOT,
                env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        binary = args.output / "cargo/release/zbm-rs"
        config = subprocess.check_output([str(binary), "--print-config"], env=env)
        assert json.loads(config)["manager"]["restart_limit"] == 2
        destination = args.output / f"zbm-rs-{name}"
        shutil.copyfile(binary, destination)
        destination.chmod(0o755)
        blob = binary.read_bytes()
        rows.append({"name": name, "opt_level": opt, "lto": lto, "codegen_units": int(units),
            "elf_bytes": len(blob), "gzip_bytes": len(gzip.compress(blob, compresslevel=9, mtime=0)),
            "build_secs": round(time.monotonic() - start, 3)})
        print(json.dumps(rows[-1]), flush=True)
        (args.output / "rust.json").write_text(json.dumps({"compiler": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "source_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "rows": rows}, indent=2) + "\n")
