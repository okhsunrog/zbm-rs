"""Security boundary regression checks for build/deployment tools."""
import importlib.util
from pathlib import Path
import tempfile
import tarfile
import io
import unittest
import hashlib
import json
import struct


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


kernel = load("check-security-kernel")
publisher = load("publish-loader")
authorizer = load("authorize-boot")
hibernation = load("protected-hibernation")
linux_publisher = load("authorize-linux")
spec = importlib.util.spec_from_file_location("nixos", Path(__file__).parents[1] / "xtask/fixtures/authorize_nixos.py")
nixos = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nixos)
spec = importlib.util.spec_from_file_location("tpm_replay", Path(__file__).parents[1] / "xtask/fixtures/check_tpm.py")
tpm_replay = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tpm_replay)


def record(name, mode, body=b"", links=1):
    fields = [1, mode, 0, 0, links, 0, len(body), 0, 0, 0, 0, len(name.encode())+1, 0]
    data = b"070701" + b"".join(f"{value:08x}".encode() for value in fields) + name.encode() + b"\0"
    data += b"\0" * (-len(data) % 4)
    data += body
    return data + b"\0" * (-len(data) % 4)


class Boundaries(unittest.TestCase):
    def test_protected_resume_policy_normalizes_only_equivalent_identities(self):
        policy = {"device": {"type": "luks2", "partuuid": "12345678-1234-1234-1234-123456789abc",
                            "luks_uuid": "ca55a30a-636e-4e69-9e68-bb36bf380408", "mapping": "resume-swap"},
                  "credential": {"type": "zfs-unlock", "pool_guid": "42", "encryption_root": "tank/root"}}
        original = hibernation.policy_id(policy)
        self.assertEqual(original, "8047e9f30e938fcbb5a69322090d54090c66b5fd092f0b4c802babfb7b4111e4")
        policy["device"]["partuuid"] = policy["device"]["partuuid"].upper()
        policy["credential"]["pool_guid"] = "00042"
        self.assertEqual(hibernation.policy_id(policy), original)
        policy["credential"]["encryption_root"] = "tank/other"
        self.assertNotEqual(hibernation.policy_id(policy), original)
        policy["role"] = "cancel-only"
        with self.assertRaisesRegex(ValueError, "Rescue"):
            hibernation.policy_id(policy)

    def test_secret_appender_cannot_replace_guard_or_hide_another_archive(self):
        import gzip
        secret = record("./.initrd-secrets", 0o040700) + record(".initrd-secrets/etc", 0o040700)
        secret += record(".initrd-secrets/etc/key", 0o100600, b"disposable-secret")
        trailer = record("TRAILER!!!", 0)
        hibernation.validate_secret_archive(gzip.compress(secret + trailer), ["/etc/key"])
        for malicious in (
            secret + record("etc/systemd/system/zbm-resume.service", 0o100644, b"bypass") + trailer,
            secret + record(".initrd-secrets/etc/key", 0o100600, b"duplicate") + trailer,
            secret + trailer + record("etc/bypass", 0o100644) + trailer,
            record(".initrd-secrets/etc/key", 0o120777, b"/other/key") + trailer,
            secret[:-1],
        ):
            with self.assertRaises(ValueError):
                hibernation.validate_secret_archive(malicious, ["/etc/key"])
        with self.assertRaisesRegex(ValueError, "missing declared secrets"):
            hibernation.validate_secret_archive(secret + trailer, ["/etc/key", "/etc/other"])

    def test_native_resume_unit_cannot_be_skipped_or_imported_after_failure(self):
        gate = "[Unit]\nDefaultDependencies=false\nBefore=initrd-root-fs.target\n[Service]\nType=oneshot\nRemainAfterExit=true\nExecStart=/approved/guard --resume-restore-config /approved/config\n"
        imports = "[Unit]\nRequires=zbm-resume.service\nAfter=zbm-resume.service\n"
        contract = {"gate_unit": gate, "loader": "/approved/guard", "resume_config": "/approved/config", "import_units": [imports]}
        hibernation.validate_units(contract)
        for changed in (gate + "ExecStart=\nExecStart=-/ignored/failure\n", gate + "ExecCondition=/skip\n", gate.replace("Before=", "ConditionPathExists=/optional\nBefore=")):
            with self.assertRaises(ValueError):
                hibernation.validate_units(contract | {"gate_unit": changed})
        with self.assertRaisesRegex(ValueError, "bypass"):
            hibernation.validate_units(contract | {"import_units": [imports + "Requires=\n"]})
        with self.assertRaisesRegex(ValueError, "replaced"):
            hibernation.validate_units(contract | {"resume_config": "/unrelated/policy"})

    def test_linux_publication_is_idempotent_and_rejects_corrupt_existing_signatures(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            key, cert = root / "fixture.key", root / "fixture.pem"
            publisher.command(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                               "-keyout", key, "-out", cert, "-subj", "/CN=disposable/", "-days", "1"])
            source = root / "source" / "artifact"
            source.mkdir(parents=True)
            payload = source / "arguments.json"
            payload.write_text('{"disposable":true}')
            publisher.command(["openssl", "cms", "-sign", "-binary", "-in", payload,
                               "-signer", cert, "-inkey", key, "-outform", "DER",
                               "-out", payload.with_suffix(".cms")])
            output = root / "published"
            linux_publisher.publish(source.parent, output, cert)
            signature = output / "artifact/arguments.cms"
            original = signature.read_bytes()
            # Reuse existing valid signed policy even if a fresh CMS differs.
            payload.with_suffix(".cms").write_bytes(b"unused fresh CMS")
            linux_publisher.publish(source.parent, output, cert)
            self.assertEqual(signature.read_bytes(), original)
            signature.write_bytes(b"invalid CMS")
            with self.assertRaises(RuntimeError):
                linux_publisher.publish(source.parent, output, cert)
            self.assertEqual((output / "artifact/arguments.json").read_bytes(), payload.read_bytes())

    def test_tpm_replay_rejects_loader_phases_and_changed_target_records(self):
        with tempfile.TemporaryDirectory() as temporary:
            run = Path(temporary)
            spec = b"Spec ID Event03\0" + bytes(8) + struct.pack("<IHHB", 1, 0x000b, 32, 0)
            digest = hashlib.sha256(b"measured-loader").digest()
            firmware = struct.pack("<II", 0, 3) + bytes(20) + struct.pack("<I", len(spec)) + spec
            firmware += struct.pack("<IIIH", 11, 0x80000001, 1, 0x000b) + digest + struct.pack("<I", 0)
            (run / "tpm-firmware.bin").write_bytes(firmware)
            (run / "tpm-pcr11.txt").write_text(tpm_replay.extend(bytes(32), digest).hex())
            (run / "tpm-pcr15-initial.txt").write_text(bytes(32).hex())
            target = b'{"version":1,"arguments":["root=ZFS=tank/root"]}\n'
            (run / "tpm-target.json").write_bytes(target)
            word = "zbm-rs:target-prepared:v1:" + hashlib.sha256(target).hexdigest()
            digest = hashlib.sha256(word.encode()).digest()
            event = {"pcr": 15, "digests": [{"hashAlg": "sha256", "digest": digest.hex()}],
                     "content": {"string": word}}
            log = "\x1e" + json.dumps(event)
            (run / "tpm-userspace.jsonseq").write_text(log)
            (run / "tpm-pcr15-prepared.txt").write_text(tpm_replay.extend(bytes(32), digest).hex())
            self.assertTrue(tpm_replay.verify(run)["passed"])
            phase = {"pcr": 11, "digests": [{"hashAlg": "sha256", "digest": hashlib.sha256(b"enter-initrd").hexdigest()}],
                     "content": {"string": "enter-initrd"}}
            (run / "tpm-userspace.jsonseq").write_text(log + "\x1e" + json.dumps(phase))
            with self.assertRaisesRegex(ValueError, "must not extend PCR11"):
                tpm_replay.verify(run)
            (run / "tpm-userspace.jsonseq").write_text(log)
            (run / "tpm-target.json").write_bytes(target.replace(b"tank/root", b"tank/other"))
            with self.assertRaisesRegex(ValueError, "exact verified final plan"):
                tpm_replay.verify(run)

    def test_authorization_index_has_shared_rust_vector_and_keeps_clone_root_variable(self):
        args = ["init=/nix/store/system/init", "root=zbm_fixture/nixos", "rootfstype=zfs", "rw"]
        self.assertEqual(authorizer.argument_id(args), "6cab784c1c6b1032495306942cd53286481046b8e61a47cdd220dd405e63702d")
        self.assertEqual(authorizer.argument_id(args), authorizer.argument_id([args[0], "root=zbm_fixture/clone", *args[2:]]))
        self.assertNotEqual(authorizer.argument_id(args), authorizer.argument_id(["init=/another", *args[1:]]))
        with self.assertRaises(ValueError):
            authorizer.argument_id(args + ["root=another"])
        self.assertEqual(authorizer.argument_id(["rw", "zfs=tank/root"]),
                         authorizer.argument_id(["rw", "zfs=tank/clone"]))
        self.assertEqual(authorizer.argument_id(["rw", "zfs=tank/root"]),
                         "180032b6c4aa9e3028344c95f0fa9f152873278fe66d78c09908b115b625f734")
        with self.assertRaises(ValueError):
            authorizer.argument_id(["root=ZFS=tank/root", "zfs=tank/other"])

    def test_target_tar_never_follows_a_guest_symlink_or_extracts_devices(self):
        for names in [[("../outside", tarfile.REGTYPE, "")],
                      [("link", tarfile.SYMTYPE, "/tmp"), ("link/file", tarfile.REGTYPE, "")],
                      [("device", tarfile.CHRTYPE, "")],
                      [("hardlink", tarfile.LNKTYPE, "file")]]:
            with tempfile.TemporaryDirectory() as temporary:
                temporary = Path(temporary)
                file = temporary / "root.tar"
                with tarfile.open(file, "w") as tar:
                    for name, kind, link in names:
                        entry = tarfile.TarInfo(name)
                        entry.type, entry.linkname = kind, link
                        tar.addfile(entry)
                with self.assertRaises(ValueError):
                    nixos.unpack_tar(file, temporary / "root")
                self.assertFalse((temporary / "outside").exists())
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            file = temporary / "root.tar"
            with tarfile.open(file, "w") as tar:
                entry = tarfile.TarInfo("file")
                entry.size = 4
                tar.addfile(entry, io.BytesIO(b"safe"))
                link = tarfile.TarInfo("guest")
                link.type, link.linkname = tarfile.SYMTYPE, "/file"
                tar.addfile(link)
            nixos.unpack_tar(file, temporary / "root")
            self.assertEqual((temporary / "root/file").read_bytes(), b"safe")
            self.assertEqual((temporary / "root/guest").readlink(), Path("/file"))

    def test_kernel_requires_actual_enforcement_and_correct_lsm(self):
        config = "\n".join(f"CONFIG_{name}=y" for name in kernel.REQUIRED) + '\nCONFIG_LSM="ima,lockdown"\n'
        kernel.validate(config)
        protected = config + "\n" + "\n".join(f"CONFIG_{name}=y" for name in
            "HIBERNATION HIBERNATION_TPM_SIGNATURE TCG_TPM TCG_TPM2_HMAC TCG_TPM_KERNEL_NVINDEX TCG_TPM2_KERNEL_KEY KERNEL_TPM_SECURITY".split())
        kernel.validate(protected, protected_hibernation=True)
        for unsupported in (config, protected + "\nCONFIG_TPM_HIBERNATE_INSECURE=y\n"):
            with self.assertRaises(ValueError):
                kernel.validate(unsupported, protected_hibernation=True)
        for change in [config.replace("CONFIG_KEXEC_SIG_FORCE=y", "CONFIG_KEXEC_SIG_FORCE=n"),
                       config.replace('"ima,lockdown"', '"lockdown"'),
                       config + "CONFIG_IMA_APPRAISE_BOOTPARAM=y\n",
                       config + "CONFIG_IMA_WRITE_POLICY=y\n"]:
            with self.assertRaises(ValueError):
                kernel.validate(change)

    def test_archive_rejects_traversal_symlink_parent_devices_and_duplicates(self):
        for data in [record("../outside", 0o100644, b"bad"),
                     record("link", 0o120777, b"/tmp") + record("link/file", 0o100644, b"bad"),
                     record("device", 0o020600),
                     record("file", 0o100644) * 2,
                     record("file", 0o100644, links=2)]:
            with tempfile.TemporaryDirectory() as temporary:
                temporary = Path(temporary)
                archive = temporary / "image.cpio"
                archive.write_bytes(data + record("TRAILER!!!", 0))
                with self.assertRaises(ValueError):
                    publisher.unpack(archive, temporary / "root")
                self.assertFalse((temporary / "outside").exists())

    def test_archive_accepts_guest_symlink_without_following_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            archive = temporary / "image.cpio"
            archive.write_bytes(record("bin", 0o040555) + record("bin/init", 0o100555, b"executable") +
                                record("init", 0o120777, b"/bin/init") + record("TRAILER!!!", 0))
            publisher.unpack(archive, temporary / "root")
            self.assertEqual((temporary / "root/bin/init").read_bytes(), b"executable")
            self.assertTrue((temporary / "root/init").is_symlink())


if __name__ == "__main__":
    unittest.main()
