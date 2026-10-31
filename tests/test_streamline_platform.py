"""Guard the Cargo boundary used by renderer/server code on every platform.

These manifest checks run without cross targets; Rust CI still compiles the
actual platform stubs and client on their native hosts.
"""
from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]


def manifest(crate):
    return tomllib.loads((ROOT / "crates" / crate / "Cargo.toml").read_text(encoding="utf-8"))


class StreamlinePlatformTests(unittest.TestCase):
    def test_renderer_can_name_the_tick_guard_on_every_target(self):
        dependency = manifest("horizon-client-render")["dependencies"]["horizon-streamline"]
        self.assertEqual(dependency["path"], "../horizon-streamline")
        self.assertFalse(dependency.get("optional", False))

    def test_native_dependencies_remain_windows_only(self):
        data = manifest("horizon-streamline")
        self.assertFalse(data.get("dependencies"))
        targets = data["target"]
        self.assertEqual(set(targets), {'cfg(target_os = "windows")'})
        self.assertTrue({"libloading", "ash", "wgpu"} <= set(next(iter(targets.values()))["dependencies"]))


if __name__ == "__main__":
    unittest.main()
