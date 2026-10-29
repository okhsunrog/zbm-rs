"""NixOS variant publication regression checks without host ZFS mutations."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("installer", Path(__file__).with_name("install-nixos.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class Installation(unittest.TestCase):
    def test_parent_declaration_must_match_the_linked_child(self):
        with tempfile.TemporaryDirectory() as temporary:
            parent = Path(temporary) / "parent"
            child = Path(temporary) / "child"
            (parent / "specialisation").mkdir(parents=True)
            child.mkdir()
            (parent / "specialisation/niri").symlink_to(child)
            child_document = {"org.nixos.bootspec.v1": {"toplevel": str(child)}}
            parent_document = {"org.nixos.specialisation.v1": {"niri": child_document}}
            with patch.object(installer, "read_system", side_effect=[({}, b"parent", parent_document), ({}, b"child", child_document)]):
                systems = installer.installation_set(parent)
                self.assertEqual([s[0] for s in systems], [parent, child])
            with patch.object(installer, "read_system", side_effect=[({}, b"parent", parent_document), ({}, b"changed", {})]):
                with self.assertRaisesRegex(ValueError, "parent declaration"):
                    installer.installation_set(parent)
            for name in ("../escape", "/absolute", ".", "", "bad\nname", "bad\u0085name"):
                document = {"org.nixos.specialisation.v1": {name: child_document}}
                with patch.object(installer, "read_system", return_value=({}, b"parent", document)):
                    with self.assertRaisesRegex(ValueError, "name"):
                        installer.installation_set(parent)

    def test_variant_invocation_publishes_the_installed_family(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            parent, child, sibling = (root / name for name in ("parent", "child", "sibling"))
            (parent / "specialisation").mkdir(parents=True)
            child.mkdir()
            sibling.mkdir()
            (parent / "specialisation/niri").symlink_to(child)
            (parent / "specialisation/other").symlink_to(sibling)
            profiles = root / "profiles"
            profiles.mkdir()
            (profiles / "system").symlink_to(parent)
            family = [(parent, {}, b"parent"), (child, {}, b"child"), (sibling, {}, b"sibling")]
            with patch.object(installer, "installation_set", return_value=family) as collect:
                self.assertEqual(installer.installation_root(child, profiles), parent)
                collect.assert_called_once_with(parent)
            # Explicit links also work before a generation enters the profile.
            (profiles / "system").unlink()
            with patch.object(installer, "installation_set", return_value=family):
                self.assertEqual(installer.installation_root(parent / "specialisation/niri", profiles), parent)
            (profiles / "system-63-link").symlink_to(parent)
            with patch.object(installer, "installation_set", return_value=family):
                self.assertEqual(installer.installation_root(child, profiles), parent)
                self.assertEqual(installer.installation_root(parent, profiles), parent)
                self.assertEqual(installer.installation_root(root, profiles), root)
            with patch.object(installer, "installation_set", side_effect=ValueError("parent declaration mismatch")):
                with self.assertRaisesRegex(ValueError, "parent declaration"):
                    installer.installation_root(child, profiles)
            with patch.object(installer, "installation_set", return_value=[family[0]]):
                with self.assertRaisesRegex(ValueError, "missing from parent Bootspec"):
                    installer.installation_root(child, profiles)

    def test_resolved_child_collects_parent_and_all_variants(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            parent, child = root / "parent", root / "child"
            (parent / "specialisation").mkdir(parents=True)
            child.mkdir()
            (parent / "specialisation/niri").symlink_to(child)
            profiles = root / "profiles"
            profiles.mkdir()
            (profiles / "system").symlink_to(parent)
            child_document = {"org.nixos.bootspec.v1": {"toplevel": str(child)}}
            parent_document = {"org.nixos.specialisation.v1": {"niri": child_document}}
            documents = {parent: ({}, b"parent", parent_document), child: ({}, b"child", child_document)}
            with patch.object(installer, "read_system", side_effect=lambda path: documents[path]):
                resolved = installer.installation_root(child, profiles)
                family = installer.installation_set(resolved)
                self.assertEqual([entry[0] for entry in family], [parent, child])

    def test_failed_child_restores_parent_bundle_and_gcroot(self):
        for previous in (None, "old-bundle"):
            with self.subTest(previous=previous), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                destination, gcroot, stage = root / "prepared", root / "gcroot", root / "stage"
                stage.mkdir()
                destination.symlink_to("new-bundle")
                gcroot.symlink_to("new-system")
                receipt = destination, previous, stage, gcroot, previous
                with patch.object(installer, "install", side_effect=[receipt, RuntimeError("child signing failed")]):
                    with self.assertRaisesRegex(RuntimeError, "child signing failed"):
                        installer.install_set({}, [("parent", {}, b""), ("child", {}, b"")], "pool/root", root)
                self.assertFalse(stage.exists())
                for link in (destination, gcroot):
                    if previous is None:
                        self.assertFalse(link.is_symlink())
                    else:
                        self.assertEqual(str(link.readlink()), previous)

    def test_non_store_system_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(ValueError, "toplevel"):
                installer.read_system(Path(temporary))


if __name__ == "__main__":
    unittest.main()
