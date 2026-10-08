"""Security boundary regression checks for build/deployment tools."""
import importlib.util
from pathlib import Path
import tempfile
import tarfile
import io
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


kernel = load("check-security-kernel")
publisher = load("publish-loader")
authorizer = load("authorize-boot")
spec = importlib.util.spec_from_file_location("nixos", Path(__file__).parents[1] / "xtask/fixtures/authorize_nixos.py")
nixos = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nixos)


def record(name, mode, body=b"", links=1):
    fields = [1, mode, 0, 0, links, 0, len(body), 0, 0, 0, 0, len(name.encode())+1, 0]
    data = b"070701" + b"".join(f"{value:08x}".encode() for value in fields) + name.encode() + b"\0"
    data += b"\0" * (-len(data) % 4)
    data += body
    return data + b"\0" * (-len(data) % 4)


class Boundaries(unittest.TestCase):
    def test_authorization_index_has_shared_rust_vector_and_keeps_clone_root_variable(self):
        args = ["init=/nix/store/system/init", "root=zbm_fixture/nixos", "rootfstype=zfs", "rw"]
        self.assertEqual(authorizer.argument_id(args), "6cab784c1c6b1032495306942cd53286481046b8e61a47cdd220dd405e63702d")
        self.assertEqual(authorizer.argument_id(args), authorizer.argument_id([args[0], "root=zbm_fixture/clone", *args[2:]]))
        self.assertNotEqual(authorizer.argument_id(args), authorizer.argument_id(["init=/another", *args[1:]]))
        with self.assertRaises(ValueError):
            authorizer.argument_id(args + ["root=another"])

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
