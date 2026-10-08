"""Security boundary regression checks for build/deployment tools."""
import importlib.util
from pathlib import Path
import tempfile
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


kernel = load("check-security-kernel")
publisher = load("publish-loader")


def record(name, mode, body=b"", links=1):
    fields = [1, mode, 0, 0, links, 0, len(body), 0, 0, 0, 0, len(name.encode())+1, 0]
    data = b"070701" + b"".join(f"{value:08x}".encode() for value in fields) + name.encode() + b"\0"
    data += b"\0" * (-len(data) % 4)
    data += body
    return data + b"\0" * (-len(data) % 4)


class Boundaries(unittest.TestCase):
    def test_kernel_requires_actual_enforcement_and_correct_lsm(self):
        config = "\n".join(f"CONFIG_{name}=y" for name in kernel.REQUIRED) + '\nCONFIG_LSM="ima,lockdown"\n'
        kernel.validate(config)
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
