"""Tests use disposable directory trees and never access host boot state."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("prune", Path(__file__).with_name("prune-nixos.py"))
prune = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prune)


class Prune(unittest.TestCase):
    def test_retains_profiles_booted_system_and_specialisation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            store, profiles, run = (root / n for n in ("store", "profiles", "run"))
            for path in (store, profiles, run):
                path.mkdir()
            names = [f"{'a' * 31}{letter}-nixos-system-{letter}" for letter in "abcd"]
            for name in names:
                (store / name).mkdir()
            (store / names[0] / "specialisation").mkdir()
            (store / names[0] / "specialisation/niri").symlink_to(store / names[1])
            (profiles / "system-1-link").symlink_to(store / names[0])
            (profiles / "system").symlink_to("system-1-link")
            (profiles / "system-2-link").symlink_to(store / names[2])
            (run / "current-system").symlink_to(store / names[0])
            (run / "booted-system").symlink_to(store / names[3])
            self.assertEqual(prune.retained_systems(profiles, run, store), set(names))
            (run / "booted-system").unlink()
            with self.assertRaises(ValueError):
                prune.retained_systems(profiles, run, store)

    def test_retains_booted_root_after_profile_link_deletion(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            store, profiles, run, roots = (root / n for n in ("store", "profiles", "run", "roots"))
            for path in (store, profiles, run, roots):
                path.mkdir()
            names = [f"{'a' * 31}{letter}-nixos-system-{letter}" for letter in "abc"]
            for name in names:
                (store / name).mkdir()
            # Booted from the niri child of a generation whose link Nix deleted.
            (store / names[0] / "specialisation").mkdir()
            (store / names[0] / "specialisation/niri").symlink_to(store / names[1])
            (roots / "system-4-link").symlink_to(store / names[0])
            (profiles / "system-7-link").symlink_to(store / names[2])
            (profiles / "system").symlink_to("system-7-link")
            (run / "current-system").symlink_to(store / names[2])
            (run / "booted-system").symlink_to(store / names[1])
            self.assertEqual(prune.retained_systems(profiles, run, store), set(names[1:]))
            self.assertEqual(prune.retained_systems(profiles, run, store, roots), set(names))
            self.assertEqual(prune.retained_systems(profiles, run, store, root / "absent"), set(names[1:]))

    def test_plan_preserves_unknown_and_shared_entries(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", base / "bundles", gcroots, store):
                path.mkdir(parents=True)
            name = "a" * 32 + "-nixos-system-old"
            bundle = base / "bundles" / (name + ".random")
            bundle.mkdir()
            (base / "nixos" / name).symlink_to(Path("../bundles") / bundle.name)
            (gcroots / name).symlink_to(store / name)
            self.assertEqual(len(prune.plan(base, gcroots, set(), store)), 1)
            self.assertEqual(prune.plan(base, gcroots, {name}, store), [])
            (base / "nixos" / "foreign").symlink_to(Path("../bundles") / bundle.name)
            self.assertEqual(prune.plan(base, gcroots, set(), store), [])
            (base / "nixos" / "foreign").unlink()
            (gcroots / name).unlink()
            (gcroots / name).symlink_to("/foreign")
            self.assertEqual(prune.plan(base, gcroots, set(), store), [])

    def test_interrupted_cleanup_resumes(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", base / "bundles", gcroots, store):
                path.mkdir(parents=True)
            name = "a" * 32 + "-nixos-system-old"
            bundle = base / "bundles" / (name + ".random")
            bundle.mkdir()
            (gcroots / name).symlink_to(store / name)
            journal = base / ".cleanup.json"
            journal.write_text(json.dumps({"system": name, "bundle": bundle.name}))
            prune.clean(base, gcroots, store, set(), [])
            self.assertFalse(journal.exists())
            self.assertFalse((gcroots / name).is_symlink())
            self.assertFalse(bundle.exists())

    def test_republished_or_retained_journal_system_never_blocks_cleanup(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", base / "bundles", gcroots, store):
                path.mkdir(parents=True)
            name = "a" * 32 + "-nixos-system-old"
            old, new = (base / "bundles" / (name + suffix) for suffix in (".old", ".new"))
            old.mkdir()
            new.mkdir()
            entry = base / "nixos" / name
            entry.symlink_to(Path("../bundles") / new.name)
            (gcroots / name).symlink_to(store / name)
            journal = base / ".cleanup.json"
            # Published again after the interrupted cleanup of the old bundle.
            journal.write_text(json.dumps({"system": name, "bundle": old.name}))
            prune.clean(base, gcroots, store, set(), [])
            self.assertFalse(journal.exists())
            self.assertFalse(old.exists())
            self.assertTrue(new.is_dir() and entry.is_symlink() and (gcroots / name).is_symlink())
            # Retained while the journalled bundle is still published: keep it.
            journal.write_text(json.dumps({"system": name, "bundle": new.name}))
            prune.clean(base, gcroots, store, {name}, [])
            self.assertFalse(journal.exists())
            self.assertTrue(new.is_dir() and entry.is_symlink() and (gcroots / name).is_symlink())

    def test_plan_collects_unreferenced_bundles(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", base / "bundles", gcroots, store):
                path.mkdir(parents=True)
            name = "a" * 32 + "-nixos-system-live"
            live, replaced, stage = (base / "bundles" / (name + s) for s in (".live", ".old", ".stage"))
            for bundle in (live, replaced, stage):
                bundle.mkdir()
            (base / "bundles" / "unknown").mkdir()
            (base / "nixos" / name).symlink_to(Path("../bundles") / live.name)
            (gcroots / name).symlink_to(store / name)
            removals = prune.plan(base, gcroots, {name}, store)
            self.assertEqual(sorted(bundle.name for _, _, bundle in removals), [replaced.name, stage.name])
            prune.clean(base, gcroots, store, {name}, removals)
            self.assertEqual(sorted(p.name for p in (base / "bundles").iterdir()), [live.name, "unknown"])
            self.assertTrue((base / "nixos" / name).is_symlink() and (gcroots / name).is_symlink())

    def test_interruption_during_deletion_is_recoverable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", base / "bundles", gcroots, store):
                path.mkdir(parents=True)
            name = "a" * 32 + "-nixos-system-old"
            bundle = base / "bundles" / (name + ".random")
            bundle.mkdir()
            (base / "nixos" / name).symlink_to(Path("../bundles") / bundle.name)
            (gcroots / name).symlink_to(store / name)
            with patch.object(prune.shutil, "rmtree", side_effect=OSError("interrupted")):
                with self.assertRaises(OSError):
                    prune.clean(base, gcroots, store, set(), prune.plan(base, gcroots, set(), store))
            self.assertTrue((base / ".cleanup.json").exists())
            prune.clean(base, gcroots, store, set(), [])
            self.assertFalse((base / ".cleanup.json").exists())
            self.assertFalse(bundle.exists())

    def test_refuses_symlinked_publication_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            base, gcroots, store = root / "boot", root / "gcroots", root / "store"
            for path in (base / "nixos", gcroots, store, root / "foreign"):
                path.mkdir(parents=True)
            (base / "bundles").symlink_to(root / "foreign")
            with self.assertRaisesRegex(ValueError, "publication directories"):
                prune.plan(base, gcroots, set(), store)


if __name__ == "__main__":
    unittest.main()
