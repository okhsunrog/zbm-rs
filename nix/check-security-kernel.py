"""Validate effective x86_64 loader configuration, not requested Nix options."""
import argparse
import json
from pathlib import Path

REQUIRED = """EFI EFI_STUB EFIVAR_FS KEXEC_FILE KEXEC_SIG KEXEC_SIG_FORCE
KEXEC_BZIMAGE_VERIFY_SIG MODULE_SIG MODULE_SIG_FORCE SYSTEM_TRUSTED_KEYRING
SECONDARY_TRUSTED_KEYRING SIGNED_PE_FILE_VERIFICATION SECURITY_LOCKDOWN_LSM
SECURITY_LOCKDOWN_LSM_EARLY LOCK_DOWN_KERNEL_FORCE_INTEGRITY INTEGRITY
INTEGRITY_SIGNATURE INTEGRITY_ASYMMETRIC_KEYS INTEGRITY_TRUSTED_KEYRING IMA
IMA_APPRAISE IMA_READ_POLICY IMA_KEYRINGS_PERMIT_SIGNED_BY_BUILTIN_OR_SECONDARY
TMPFS_XATTR CRYPTO_SHA256""".split()


def validate(text):
    values = {}
    for line in text.splitlines():
        if line.startswith("CONFIG_") and "=" in line:
            name, value = line.split("=", 1)
            if name in values:
                raise ValueError(f"Duplicate kernel configuration: {name}")
            values[name] = value
    failures = [f"CONFIG_{name}=y required" for name in REQUIRED
                if values.get(f"CONFIG_{name}") != "y"]
    if values.get("CONFIG_IMA_APPRAISE_BOOTPARAM") == "y":
        failures.append("IMA appraisal must not be weakened by boot parameters")
    if values.get("CONFIG_IMA_WRITE_POLICY") == "y" and values.get(
            "CONFIG_IMA_APPRAISE_REQUIRE_POLICY_SIGS") != "y":
        failures.append("Writable IMA policy requires enforced policy signatures")
    try:
        lsms = json.loads(values.get("CONFIG_LSM", '""')).split(",")
    except (ValueError, AttributeError):
        lsms = []
    if not {"ima", "lockdown"}.issubset(lsms):
        failures.append("Effective LSM list must include ima and lockdown")
    if failures:
        raise ValueError("Unsupported enforced loader kernel: " + "; ".join(failures))
    return {"architecture": "x86_64", "required": REQUIRED, "lsms": lsms,
            "ima_policy_updates": values.get("CONFIG_IMA_WRITE_POLICY") == "y"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("config", type=Path)
    args = parser.parse_args()
    print(json.dumps(validate(args.config.read_text()), indent=2))
