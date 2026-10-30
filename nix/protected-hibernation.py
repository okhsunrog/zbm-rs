"""Validate trusted Nix-built native-resume inputs before owner authorization.

The installer supplies this contract from its immutable configuration, never
from a discovered boot entry or prepared.json. The runtime broker trusts only
the resulting CMS-signed authorization and exact artifact hashes.
"""
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import resource
import shutil
import struct
import subprocess
import tempfile


def kernel_payload(path):
    data = bytearray(path.read_bytes())
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError("Expected a protected EFI kernel")
    pe = struct.unpack_from("<I", data, 60)[0]
    if pe + 24 + 68 > len(data) or data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("Malformed EFI kernel")
    # PE checksum is deliberately rewritten by the signing tools; it is not
    # covered by Authenticode. Everything else must match the approved image.
    checksum = pe + 24 + 64
    data[checksum:checksum + 4] = bytes(4)
    data.extend(bytes(-len(data) % 8))
    return data


def validate_kernel(path, approved):
    reference = kernel_payload(approved)
    if kernel_payload(path) == reference:
        return
    with tempfile.TemporaryDirectory(prefix="zbm-kernel-contract-") as temporary:
        detached = Path(temporary) / "kernel"
        shutil.copyfile(path, detached)
        # The Nix kernel may already carry a public signature. sbattach removes
        # one certificate at a time, while sbsign appends another certificate.
        for _ in range(16):
            size = detached.stat().st_size
            result = subprocess.run(["sbattach", "--remove", str(detached)],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if result.returncode:
                break
            if kernel_payload(detached) == reference:
                return
            if detached.stat().st_size >= size:
                break
        raise ValueError("Target kernel differs from the approved protected build")


def policy_id(config):
    if config.get("role", "resume") != "resume":
        raise ValueError("Rescue cannot authorize protected hibernation")
    device = config.get("device")
    legacy = config.get("swap_partuuid")
    if device is not None and legacy is not None:
        raise ValueError("Ambiguous resume device")
    device = device or ({"type": "partition", "partuuid": legacy} if legacy else {})
    kind = device.get("type")
    fields = [kind]
    for name in (["partuuid"] if kind == "partition" else ["partuuid", "luks_uuid"] if kind == "luks2" else []):
        value = device.get(name)
        if not isinstance(value, str) or not re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", value):
            raise ValueError("Invalid resume UUID")
        fields.append(value.lower())
    if kind == "luks2":
        mapping = device.get("mapping", "")
        if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_-]{0,62}", mapping) or mapping == "control":
            raise ValueError("Invalid resume mapping")
        fields.append(mapping)
    elif kind != "partition":
        raise ValueError("Protected hibernation requires a dedicated resume device")
    source = config.get("credential")
    if source is None and kind == "partition":
        fields.append("none")
    elif kind == "luks2" and isinstance(source, dict) and source.get("type") == "zfs-unlock":
        guid, root = source.get("pool_guid"), source.get("encryption_root")
        if not isinstance(guid, str) or not re.fullmatch(r"\+?[0-9]+", guid) or not 0 < int(guid) < 2**64:
            raise ValueError("Invalid credential pool GUID")
        if not isinstance(root, str) or not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.:-]*(/[A-Za-z0-9_.:-]+)*", root):
            raise ValueError("Invalid credential encryption root")
        fields.extend(["zfs-unlock", str(int(guid)), root])
    else:
        raise ValueError("Resume device and credential source are incompatible")
    digest = hashlib.sha256(b"zbm-rs:resume-policy:v1\0")
    for field in fields:
        encoded = field.encode()
        digest.update(len(encoded).to_bytes(4, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def validate_secret_archive(data, expected):
    """Only the declared regular secrets may follow the immutable base initrd."""
    limit = 64 * 1024 * 1024
    if len(data) > limit:
        raise ValueError("Oversized appended secret archive")
    if data.startswith(b"\x1f\x8b"):
        import io
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
            data = stream.read(limit + 1)
    elif data.startswith(b"\x28\xb5\x2f\xfd"):
        with tempfile.TemporaryFile() as output:
            def bounded_output():
                resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))
            result = subprocess.run(["zstd", "-dq", "-c"], input=data, stdout=output,
                                    stderr=subprocess.DEVNULL, preexec_fn=bounded_output)
            if result.returncode:
                raise ValueError("Invalid or oversized compressed secret archive")
            output.seek(0)
            data = output.read(limit + 1)
    if len(data) > limit:
        raise ValueError("Oversized uncompressed secret archive")
    expected = {".initrd-secrets/" + name.lstrip("/") for name in expected}
    found, seen, cursor = set(), set(), 0
    while cursor < len(data):
        header = data[cursor:cursor + 110]
        if len(header) != 110 or header[:6] != b"070701":
            raise ValueError("Expected one newc secret archive")
        fields = [int(header[i:i + 8], 16) for i in range(6, 110, 8)]
        mode, links, size, namesize = fields[1], fields[4], fields[6], fields[11]
        if not 1 <= namesize <= 4096:
            raise ValueError("Invalid secret archive name")
        start = cursor + 110
        name = data[start:start + namesize]
        if len(name) != namesize or not name.endswith(b"\0"):
            raise ValueError("Truncated secret archive name")
        name = name[:-1].decode()
        cursor = (start + namesize + 3) & ~3
        end = cursor + size
        if end > len(data):
            raise ValueError("Truncated secret archive body")
        cursor = (end + 3) & ~3
        if name == "TRAILER!!!":
            if any(data[cursor:]) or found != expected:
                raise ValueError("Unexpected trailing data or missing declared secrets")
            return
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or any(ord(char) < 32 for char in name):
            raise ValueError("Unsafe appended secret path")
        normalized = str(path)
        if normalized in seen:
            raise ValueError("Duplicate appended secret path")
        seen.add(normalized)
        kind = mode & 0o170000
        if kind == 0o040000 and size == 0 and any(Path(secret).is_relative_to(path) for secret in expected):
            continue
        if kind != 0o100000 or links != 1 or normalized not in expected:
            raise ValueError("Appender changed a non-secret initramfs path")
        found.add(normalized)
    raise ValueError("Missing secret archive trailer")


def unit_settings(text):
    section, result = "", {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith(("#", ";")):
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if "=" not in line or line.endswith("\\"):
            raise ValueError("Unsupported protected unit syntax")
        key, value = line.split("=", 1)
        if not value:
            result[(section, key)] = []
        else:
            result.setdefault((section, key), []).append(value)
    return result


def validate_units(contract):
    gate = unit_settings(contract["gate_unit"])
    required = {
        ("Unit", "DefaultDependencies"): ["false"],
        ("Service", "Type"): ["oneshot"],
        ("Service", "RemainAfterExit"): ["true"],
        ("Service", "ExecStart"): [contract["loader"] + " --resume-restore-config " + contract["resume_config"]],
    }
    for setting, expected in required.items():
        if gate.get(setting) != expected:
            raise ValueError("Native resume guard was disabled or replaced")
    if "initrd-root-fs.target" not in " ".join(gate.get(("Unit", "Before"), [])).split():
        raise ValueError("Native resume guard is not before root mounts")
    for (section, key), values in gate.items():
        if (section == "Unit" and key.startswith(("Condition", "Assert"))) or (
                section == "Service" and key in ("ExecCondition", "ExecStartPre", "ExecStartPost", "SuccessExitStatus") and values):
            raise ValueError("Native resume guard has an unsupported conditional or override")
    if not contract["import_units"]:
        raise ValueError("Missing native ZFS import dependencies")
    for text in contract["import_units"]:
        settings = unit_settings(text)
        for key in ("Requires", "After"):
            if "zbm-resume.service" not in " ".join(settings.get(("Unit", key), [])).split():
                raise ValueError("A ZFS import can bypass the native resume guard")


def validate(contract, kernel, initramfs, arguments):
    """Kernel/initramfs references are supplied by the owner install hook."""
    if contract.get("version") != 1 or contract.get("record_protocol") != 2:
        raise ValueError("Unsupported protected-hibernation build contract")
    validate_units(contract)
    for secret in contract["secret_paths"]:
        path = Path(secret)
        forbidden = ["/nix", "/bin", "/sbin", "/usr", "/lib", "/lib64", "/init",
                     "/etc/zbm-rs", "/etc/systemd/system", "/etc/systemd/system-generators"]
        if not path.is_absolute() or ".." in path.parts or any(
                path.is_relative_to(prefix) or Path(prefix).is_relative_to(path) for prefix in forbidden):
            raise ValueError("A declared secret would replace protected initramfs code or policy")
    spec = importlib.util.spec_from_file_location("kernel_validator", Path(__file__).with_name("check-security-kernel.py"))
    validator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validator)
    validator.validate(Path(contract["kernel_config"]).read_text(), protected_hibernation=True)
    validate_kernel(kernel, Path(contract["kernel"]))
    base = Path(contract["initramfs"]).read_bytes()
    prepared = initramfs.read_bytes()
    if not prepared.startswith(base):
        raise ValueError("Target initramfs differs from the validated native-resume build")
    appended = prepared[len(base):]
    if appended:
        validate_secret_archive(appended, contract["secret_paths"])
    elif contract["secret_paths"]:
        raise ValueError("Missing required initramfs secrets")
    relevant = [arg for arg in arguments if arg.startswith("sysctl.kernel.kexec_load_disabled=")]
    if relevant != ["sysctl.kernel.kexec_load_disabled=1"]:
        raise ValueError("Protected target must prohibit kexec before initramfs")
    if any(arg in ("nohibernate", "noresume") or arg.startswith(("resume=", "resume_offset=")) for arg in arguments):
        raise ValueError("Protected target bypasses the native resume guard")
    for argument in arguments:
        if argument.startswith("lsm=") and not {"ima", "lockdown"}.issubset(argument[4:].split(",")):
            raise ValueError("Protected target disables required security modules")
        if argument.startswith("rdinit=") or argument.startswith("initcall_blacklist="):
            raise ValueError("Protected target bypasses the approved initramfs or kernel initialization")
    config = json.loads(Path(contract["resume_config"]).read_text())["resume"]
    return {"version": 1, "backend": "linux-tpm-signature-v1", "record_protocol": 2,
            "resume_policy_sha256": policy_id(config)}
