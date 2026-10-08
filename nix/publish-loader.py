"""Sign a fresh output copy. Private key paths are passed only to signing tools."""
import argparse
import gzip
import json
import lzma
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

MEASURED_SECTIONS = ("linux", "osrel", "cmdline", "initrd", "ucode", "splash", "dtb",
                     "uname", "sbat", "pcrpkey", "profile", "dtbauto", "hwids", "efifw")


def pe_sections(file):
    """Read precisely the PE VirtualSize bytes measured by systemd-stub."""
    data = file.read_bytes()
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError("Invalid EFI DOS header")
    offset = struct.unpack_from("<I", data, 60)[0]
    if offset + 24 > len(data) or data[offset:offset+4] != b"PE\0\0":
        raise ValueError("Invalid EFI PE header")
    count, optional_size = struct.unpack_from("<H", data, offset+6)[0], struct.unpack_from("<H", data, offset+20)[0]
    start = offset + 24 + optional_size
    if count > 96 or start + count * 40 > len(data):
        raise ValueError("Invalid EFI section table")
    sections = {}
    for index in range(count):
        header = data[start+index*40:start+(index+1)*40]
        name = header[:8].rstrip(b"\0").decode("ascii")
        virtual, _, raw, position = struct.unpack_from("<IIII", header, 8)
        if name in sections or position + raw > len(data):
            raise ValueError("Duplicate or truncated EFI section")
        if name.startswith(".") and name[1:] in MEASURED_SECTIONS and virtual > raw:
            raise ValueError("Measured EFI section has unmapped zero padding")
        sections[name] = data[position:position+min(virtual, raw)]
    return sections


def sign_pcr_policy(unsigned, output, key, measure, build):
    """Sign exact final section contents, then add the unmeasured .pcrsig."""
    public = output / "pcr-public.pem"
    expected = command(["openssl", "pkey", "-pubin", "-in", public, "-outform", "DER"])
    actual = command(["openssl", "pkey", "-in", key, "-pubout", "-outform", "DER"])
    if expected != actual:
        raise ValueError("PCR signing key differs from packaged public key")
    sections = pe_sections(unsigned)
    signature = output / "pcr-signature.json"
    with tempfile.TemporaryDirectory(prefix="zbm-pcr-sections-") as directory:
        options = []
        for name in MEASURED_SECTIONS:
            if content := sections.get(f".{name}"):
                file = Path(directory) / name
                file.write_bytes(content)
                options.append(f"--{name}={file}")
        encoded = command([measure, "sign", "--bank=sha256", "--phase=enter-initrd",
                           "--policyref=initrd", "--private-key", key, "--public-key", public,
                           "--json=short", *options])
        if len(encoded) > 1024 * 1024 or not json.loads(encoded).get("sha256"):
            raise ValueError("Missing bounded SHA-256 PCR policy signatures")
        signature.write_bytes(encoded)
    joined = output / "pcr-signed.efi"
    # --join-pcrsig fills a pre-existing policy-digest section; it silently adds
    # nothing when that section is absent. Build a fresh section instead.
    command([*build, "--section", f".pcrsig:@{signature}", "--output", joined])
    after = pe_sections(joined)
    if ".pcrsig" not in after or json.loads(after[".pcrsig"]) != json.loads(signature.read_bytes()):
        raise ValueError("Final UKI is missing its signed PCR policy")
    if any(after.get(f".{name}") != sections.get(f".{name}") for name in MEASURED_SECTIONS):
        raise ValueError("Adding PCR signatures changed measured EFI sections")
    unsigned.unlink()
    joined.rename(unsigned)


def command(args, *, output=None, cwd=None, env=None):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd, env=env,
                            stdout=output or subprocess.PIPE, stderr=subprocess.PIPE)
    if result.returncode:
        raise RuntimeError(f"{Path(args[0]).name} failed ({result.returncode}): {result.stderr.decode(errors='replace')}")
    return result.stdout


def public_der(cert):
    return command(["openssl", "x509", "-in", cert, "-outform", "DER"])


def unpack(archive, root):
    """Reject path traversal, symlink parents, devices and hardlink ambiguity."""
    if archive.stat().st_size > 256 * 1024 * 1024:
        raise ValueError("Oversized loader staging archive")
    data, cursor, entries = archive.read_bytes(), 0, {}
    while cursor < len(data):
        header = data[cursor:cursor+110]
        if len(header) != 110 or header[:6] != b"070701":
            raise ValueError("Expected bounded newc staging archive")
        fields = [int(header[i:i+8], 16) for i in range(6, 110, 8)]
        mode, links, size, namesize = fields[1], fields[4], fields[6], fields[11]
        if not 1 <= namesize <= 4096:
            raise ValueError("Invalid cpio filename")
        start = cursor + 110
        name = data[start:start+namesize]
        if not name.endswith(b"\0"):
            raise ValueError("Unterminated cpio filename")
        name = name[:-1].decode()
        cursor = (start + namesize + 3) & ~3
        body = data[cursor:cursor+size]
        if len(body) != size:
            raise ValueError("Truncated staging file")
        cursor = (cursor + size + 3) & ~3
        if name == "TRAILER!!!":
            break
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or any(ord(char) < 32 for char in name):
            raise ValueError("Unsafe staging path")
        if str(path) == ".":
            continue
        kind = mode & 0o170000
        if kind not in (0o040000, 0o100000, 0o120000) or (kind == 0o100000 and links != 1):
            raise ValueError("Unsupported staging inode type/hardlink")
        if path in entries:
            raise ValueError("Duplicate staging path")
        entries[path] = (kind, mode & 0o777, body)
    else:
        raise ValueError("Missing cpio trailer")
    for path in entries:
        if any(parent in entries and entries[parent][0] != 0o040000 for parent in path.parents):
            raise ValueError("Staging file has a non-directory ancestor")
    for path, (kind, mode, body) in sorted(entries.items(), key=lambda item: len(item[0].parts)):
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        if kind == 0o040000:
            target.mkdir(exist_ok=True)
            target.chmod(mode | 0o700)
        elif kind == 0o120000:
            target.symlink_to(body.decode())
        else:
            target.write_bytes(body)
            target.chmod(mode | 0o200)  # Private owned staging must be writable.


def ima_sign(file, key, cert, tool, environment):
    usage = command(["openssl", "x509", "-in", cert, "-noout", "-ext", "basicConstraints,keyUsage"]).decode()
    if "CA:TRUE" in usage or "Certificate Sign" in usage or "Digital Signature" not in usage:
        raise ValueError("IMA certificate must be non-CA with digitalSignature usage")
    command([tool, "--xattr-user", "-a", "sha256", "-k", key, "--keyid-from-cert", cert,
             "--sigfile", "ima_sign", file], env=environment)
    signature = Path(f"{file}.sig").read_bytes()
    if len(signature) < 9 or signature[:3] != bytes([3, 2, 4]):
        raise ValueError("Expected Linux IMA v2 SHA-256 signature")
    if int.from_bytes(signature[7:9], "big") != len(signature) - 9:
        raise ValueError("Malformed detached IMA signature")
    # Independently verify the signature against the explicit public certificate.
    with tempfile.TemporaryDirectory(prefix="zbm-ima-verify-") as temporary:
        temporary = Path(temporary)
        public = temporary / "public.pem"
        public.write_bytes(command(["openssl", "x509", "-in", cert, "-pubkey", "-noout"]))
        raw = temporary / "signature"
        raw.write_bytes(signature[9:])
        command(["openssl", "dgst", "-sha256", "-verify", public, "-signature", raw, file])


def verify_module(file, cert):
    data = file.read_bytes()
    marker = b"~Module signature appended~\n"
    if not data.endswith(marker):
        raise ValueError(f"Unsigned module: {file.name}")
    header = data[-len(marker)-12:-len(marker)]
    size = struct.unpack(">I", header[8:])[0]
    if header[2] != 2 or header[3:8] != b"\0" * 5 or size > len(data) - len(marker) - 12:
        raise ValueError(f"Unsupported module signature: {file.name}")
    boundary = len(data) - len(marker) - 12 - size
    with tempfile.TemporaryDirectory(prefix="zbm-module-verify-") as temporary:
        temporary = Path(temporary)
        (temporary / "content").write_bytes(data[:boundary])
        (temporary / "signature").write_bytes(data[boundary:boundary+size])
        command(["openssl", "cms", "-verify", "-binary", "-inform", "DER",
                 "-noverify", "-nointern", "-certfile", cert,
                 "-in", temporary / "signature", "-content", temporary / "content",
                 "-out", os.devnull])


def sign_modules(root, key, cert, sign_file):
    signed = []
    with tempfile.TemporaryDirectory(prefix="zbm-module-sign-") as temporary:
        raw = Path(temporary) / "module.ko"
        for file in sorted((root / "usr/lib/modules").rglob("*")):
            if not any(file.name.endswith(suffix) for suffix in (".ko", ".ko.gz", ".ko.xz", ".ko.zst")):
                continue
            if file.is_symlink() or not file.resolve().is_relative_to(root.resolve()):
                raise ValueError("Staged module must be an owned regular file")
            data = file.read_bytes()
            if file.suffix == ".gz":
                data = gzip.decompress(data)
            elif file.suffix == ".xz":
                data = lzma.decompress(data)
            elif file.suffix == ".zst":
                data = command(["zstd", "-q", "-d", "-c", file])
            raw.write_bytes(data)
            command([sign_file, "sha256", key, cert, raw])
            verify_module(raw, cert)
            if file.suffix == ".gz":
                file.write_bytes(gzip.compress(raw.read_bytes(), mtime=0))
            elif file.suffix == ".xz":
                file.write_bytes(lzma.compress(raw.read_bytes(), check=lzma.CHECK_CRC32))
            elif file.suffix == ".zst":
                with file.open("wb") as output:
                    command(["zstd", "-q", "-19", "-T1", "-c", raw], output=output)
            else:
                shutil.copyfile(raw, file)
            signed.append(str(file.relative_to(root)))
    if not signed:
        raise ValueError("No preboot modules found")
    return signed


def archive(root, output):
    names = sorted(str(path.relative_to(root)) for path in root.rglob("*"))
    # The input is the trusted canonical staging tree, never an installed OS.
    with output.open("wb") as stream:
        subprocess.run(["cpio", "--null", "--quiet", "-o", "-H", "newc",
                        "--owner=0:0", "--reproducible"], cwd=root,
                       input=b"\0".join(name.encode() for name in names) + b"\0",
                       stdout=stream, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("output", type=Path)
    for role in ("efi", "ima", "module"):
        parser.add_argument(f"--{role}-key", required=True, type=Path)
        parser.add_argument(f"--{role}-cert", required=True, type=Path)
    parser.add_argument("--sign-file", required=True, type=Path)
    parser.add_argument("--evmctl", required=True, type=Path)
    parser.add_argument("--ukify", default=shutil.which("ukify") or "/usr/lib/systemd/ukify", type=Path)
    parser.add_argument("--pcr-key", type=Path)
    parser.add_argument("--systemd-measure", type=Path)
    parser.add_argument("--allow-fixture", action="store_true")
    parser.add_argument("--fixture-directory", type=Path)
    args = parser.parse_args()
    image, output = args.image.resolve(), args.output.resolve()
    if output.is_relative_to(Path("/nix/store")) or output.exists():
        raise ValueError("Output must be a fresh directory outside the Nix store")
    for key in (args.efi_key, args.ima_key, args.module_key, args.pcr_key):
        if key is not None and key.resolve().is_relative_to(Path("/nix/store")):
            raise ValueError("Private signing keys must stay outside the Nix store")
    pcr = (image / "pcr-public.pem").is_file()
    if pcr != (args.pcr_key is not None and args.systemd_measure is not None):
        raise ValueError("PCR public key requires an external PCR key and v262 systemd-measure")
    manifest = json.loads((image / "manifest.json").read_text())
    if manifest["test_ssh"] and not args.allow_fixture:
        raise ValueError("Refusing to publish a production-signed test/SSH image")
    if args.fixture_directory and not (manifest["test_ssh"] and args.allow_fixture):
        raise ValueError("Fixture payloads require explicit disposable test deployment")
    if not (image / "kernel-capabilities.json").is_file():
        raise ValueError("Enforced kernel capability report is missing")
    destination = output
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".zbm-publish-", dir=output.parent) as temporary:
        output = Path(temporary) / "deployment"
        root = Path(temporary) / "root"
        root.mkdir()
        unpack(image / "root.cpio", root)
        if (root / "etc/zbm-rs/trust/ima.der").read_bytes() != public_der(args.ima_cert):
            raise ValueError("IMA signing certificate differs from image trust")
        if args.fixture_directory:
            for file in args.fixture_directory.rglob("*"):
                if file.suffix == ".key" or file.is_symlink():
                    raise ValueError("Fixture payload must not contain signing keys or symlinks")
            shutil.copytree(args.fixture_directory, root / "fixtures")
        environment = os.environ.copy()
        # Local installations may carry the tool's own libimaevm beside its ELF.
        environment["LD_LIBRARY_PATH"] = str(args.evmctl.resolve().parent)
        ima_sign(root / "etc/zbm-rs/security/ima-policy", args.ima_key.resolve(),
                 args.ima_cert.resolve(), args.evmctl.resolve(), environment)
        signed = sign_modules(root, args.module_key.resolve(), args.module_cert.resolve(), args.sign_file.resolve())
        shutil.copytree(image, output, symlinks=False)
        output.chmod(0o700)
        for file in output.rglob("*"):
            if file.is_file():
                file.chmod(0o600)
            elif file.is_dir():
                file.chmod(0o700)
        archive(root, output / "root.cpio")
        with (output / "initramfs.img").open("wb") as stream:
            command(["zstd", "-q", "-T1", "-19", "-c", output / "root.cpio"], output=stream)
        unsigned = output / "unsigned.efi"
        build = [args.ukify, "build", "--linux", output / "vmlinuz", "--initrd", output / "initramfs.img",
                 "--uname", manifest["kernel"], "--cmdline", f"@{output / 'cmdline'}",
                 "--os-release", f"@{output / 'os-release'}", "--stub", output / "stub.efi",
                 *(["--pcrpkey", output / "pcr-public.pem"] if pcr else [])]
        command([*build, "--output", unsigned])
        if pcr:
            sign_pcr_policy(unsigned, output, args.pcr_key.resolve(), args.systemd_measure.resolve(), build)
        efi = output / "esp/EFI/BOOT/BOOTX64.EFI"
        command(["sbsign", "--key", args.efi_key.resolve(), "--cert", args.efi_cert.resolve(), "--output", efi, unsigned])
        command(["sbverify", "--cert", args.efi_cert.resolve(), efi])
        unsigned.unlink()
        manifest["deployment"] = {"efi_signed": True, "ima_policy_signed": True,
                                  "pcr_policy_signed": pcr,
                                  "modules_verified": signed, "disposable_fixture": args.allow_fixture}
        import hashlib
        for name in ("kernel", "initramfs"):
            file = output / ("vmlinuz" if name == "kernel" else "initramfs.img")
            manifest[f"{name}_sha256"] = hashlib.sha256(file.read_bytes()).hexdigest()
        manifest["sizes"] = {"efi": efi.stat().st_size, "kernel": (output / "vmlinuz").stat().st_size,
                             "initramfs": (output / "initramfs.img").stat().st_size}
        if manifest["sizes"]["efi"] > (80 if manifest["test_ssh"] else 64) * 1024 * 1024:
            raise ValueError("Signed EFI exceeds deployment size budget")
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        (output / "sizes.json").write_text(json.dumps(manifest["sizes"], indent=2) + "\n")
        output.rename(destination)
    print(json.dumps({"output": str(destination), "signed_modules": len(signed), "fixture": args.allow_fixture}))


if __name__ == "__main__":
    main()
