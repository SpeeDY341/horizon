"""Exercise the Windows developer launcher without starting a game or reading player data."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


POWERSHELL = shutil.which("powershell.exe") if os.name == "nt" else None
REPO = Path(__file__).resolve().parents[1]
ENV_KEYS = (
    "HORIZON_HOME", "HORIZON_LAUNCHER_HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME",
    "HORIZON_LAUNCHER_TRASH", "HORIZON_LAUNCHER_CLIPBOARD", "HORIZON_LAUNCHER_DESKTOP",
    "HORIZON_LAUNCHER_NO_SELF_UPDATE", "HORIZON_NO_AUDIO", "HORIZON_STREAMLINE_SDK",
    "HORIZON_DLSS_FORCE_ERROR", "HORIZON_GRAPHICS_API",
)


@unittest.skipUnless(POWERSHELL, "requires Windows PowerShell")
class PlayDlssTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="horizon-launcher-contract-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in ("tools", "target/fast", ".local/dlss/native",
                          ".local/horizon-client/26.3", "runs"):
            (self.root / directory).mkdir(parents=True, exist_ok=True)
        self.script = self.root / "tools/play_dlss_test.ps1"
        shutil.copyfile(REPO / "tools/play_dlss_test.ps1", self.script)
        (self.root / "target/fast/horizon-client.exe").touch()
        (self.root / ".local/horizon-client/26.3/horizon-resources.json").write_text("{}")
        for dll in ("sl.interposer", "sl.common", "sl.dlss", "nvngx_dlss", "sl.dlss_g", "nvngx_dlssg"):
            (self.root / f".local/dlss/native/{dll}.dll").touch()
        self.driver = self.root / "driver.ps1"
        self.driver.write_text(r'''
param([string]$Launcher, [string]$Capture, [switch]$Smoke, [string]$GraphicsSettingsFrom)
$ErrorActionPreference = 'Stop'
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$NoNewWindow, $RedirectStandardOutput,
        $RedirectStandardError, [switch]$Wait, [switch]$PassThru)
    $environment = @{}
    foreach ($key in @('HORIZON_HOME', 'HORIZON_LAUNCHER_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
        'HORIZON_LAUNCHER_TRASH', 'HORIZON_LAUNCHER_CLIPBOARD', 'HORIZON_LAUNCHER_DESKTOP',
        'HORIZON_LAUNCHER_NO_SELF_UPDATE', 'HORIZON_NO_AUDIO', 'HORIZON_STREAMLINE_SDK',
        'HORIZON_DLSS_FORCE_ERROR', 'HORIZON_GRAPHICS_API')) {
        $value = [Environment]::GetEnvironmentVariable($key)
        if ($null -ne $value) { $environment[$key] = $value }
    }
    [IO.File]::WriteAllText($Capture, (@{
        arguments = $ArgumentList; environment = $environment
        stdout = $RedirectStandardOutput; stderr = $RedirectStandardError
    } | ConvertTo-Json -Depth 4))
    Set-Content -LiteralPath $RedirectStandardOutput -Value 'mock stdout'
    Set-Content -LiteralPath $RedirectStandardError -Value 'mock stderr'
    Start-Sleep -Milliseconds 300
    [pscustomobject]@{ ExitCode = 0 }
}
$parameters = @{ Smoke = $Smoke }
if ($GraphicsSettingsFrom) { $parameters.GraphicsSettingsFrom = $GraphicsSettingsFrom }
& $Launcher @parameters
''', encoding="utf-8")

    def start(self, name, *arguments):
        capture = self.root / f"{name}.json"
        environment = os.environ.copy()
        environment.update(TEMP=str(self.root / "runs"), TMP=str(self.root / "runs"),
                           HORIZON_DLSS_FORCE_ERROR="1", HORIZON_GRAPHICS_API="dx12")
        process = subprocess.Popen(
            [POWERSHELL, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(self.driver),
             "-Launcher", str(self.script), "-Capture", str(capture), *arguments],
            env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        return process, capture

    def finish(self, launched):
        process, capture = launched
        stdout, stderr = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 0, stdout + stderr)
        return json.loads(capture.read_text(encoding="utf-8-sig"))

    def scratch(self, result):
        arguments = [argument.strip('"') for argument in result["arguments"]]
        return Path(arguments[arguments.index("--game-dir") + 1])

    def test_parallel_title_and_smoke_have_independent_homes_and_logs(self):
        title = self.start("title")
        smoke = self.start("smoke", "-Smoke")
        results = [self.finish(title), self.finish(smoke)]
        paths = [self.scratch(result) for result in results]
        self.assertNotEqual(paths[0], paths[1])
        for index, result in enumerate(results):
            scratch = paths[index]
            self.assertEqual(scratch.parent, self.root / "runs")
            environment = result["environment"]
            arguments = [argument.strip('"') for argument in result["arguments"]]
            self.assertEqual(arguments[arguments.index("--home") + 1], environment["HORIZON_HOME"])
            for key in ENV_KEYS[:7]:
                self.assertEqual(Path(environment[key]).parent, scratch)
            self.assertEqual(environment["HORIZON_NO_AUDIO"], "1")
            self.assertEqual(environment["HORIZON_LAUNCHER_NO_SELF_UPDATE"], "1")
            self.assertEqual(Path(environment["HORIZON_STREAMLINE_SDK"]), self.root / ".local/dlss/native")
            self.assertNotIn("HORIZON_DLSS_FORCE_ERROR", environment)
            self.assertNotIn("HORIZON_GRAPHICS_API", environment)
            self.assertEqual(Path(result["stdout"]), scratch / "stdout.log")
            self.assertEqual(Path(result["stderr"]), scratch / "game.log")
            self.assertTrue((scratch / "horizon-graphics.txt").read_text().startswith("graphicsApi:vulkan"))
        self.assertNotIn('"--world"', results[0]["arguments"])
        self.assertIn('"--world"', results[1]["arguments"])
        self.assertIn('"' + str(paths[1] / "report.json") + '"', results[1]["arguments"])

    def test_restart_copies_only_explicit_graphics_file(self):
        first = self.finish(self.start("first"))
        old = self.scratch(first)
        settings = old / "horizon-graphics.txt"
        settings.write_text("graphicsApi:dx12\ndlss:balanced\n")
        (old / "saves").mkdir()
        (old / "saves/player-world").touch()
        (old / "options.txt").write_text("maxFps:17\n")
        result = self.finish(self.start("restart", "-GraphicsSettingsFrom", str(settings)))
        fresh = self.scratch(result)
        self.assertNotEqual(old, fresh)
        self.assertEqual((fresh / "horizon-graphics.txt").read_bytes(), settings.read_bytes())
        self.assertFalse((fresh / "saves").exists())
        self.assertIn("maxFps:120", (fresh / "options.txt").read_text())

    def test_missing_settings_fails_before_launch(self):
        process, capture = self.start("invalid", "-GraphicsSettingsFrom", str(self.root / "missing.txt"))
        process.communicate(timeout=30)
        self.assertNotEqual(process.returncode, 0)
        self.assertFalse(capture.exists())


if __name__ == "__main__":
    unittest.main()
