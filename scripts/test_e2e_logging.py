"""Exercise the real Make coordinator with deterministic substitute test processes."""

from pathlib import Path
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent


class E2ELoggingTests(unittest.TestCase):
    """Check durable logs and exit-status propagation without launching the full suites."""

    def run_coordinator(self, contract_status: int) -> tuple[int, dict[str, str], dict]:
        """Run Make against temporary fixtures and return its saved artifacts."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for directory in ["scripts", "rust-server", "client-server-tests/browser", "rust-server-tests", "bin"]:
                (root / directory).mkdir(parents=True)
            shutil.copy(ROOT / "scripts/write_e2e_report.py", root / "scripts")
            fake = root / "bin/fake-process"
            fake.write_text(
                f"#!{sys.executable}\n"
                "import os, pathlib, sys\n"
                "print('stdout: ' + ' '.join(sys.argv[1:]), flush=True)\n"
                "print('stderr diagnostic', file=sys.stderr, flush=True)\n"
                "if pathlib.Path.cwd().name == 'client-server-tests' and sys.argv[1:] == ['test']:\n"
                "    print('test intermittent_contract ... FAILED', file=sys.stderr)\n"
                "    print('failures: assertion evidence', file=sys.stderr)\n"
                "    sys.exit(int(os.environ['CONTRACT_STATUS']))\n"
                "if '--test' in sys.argv and 'browser_e2e' in sys.argv:\n"
                "    report = pathlib.Path('target/browser-e2e/report.md')\n"
                "    report.parent.mkdir(parents=True, exist_ok=True)\n"
                "    report.write_text('# Parlando browser end-to-end report\\n\\nCurrent browser report\\n')\n"
            )
            fake.chmod(0o755)
            for name in ["cargo", "npm"]:
                (root / "bin" / name).symlink_to(fake)
            # A stale Prolific artifact must never be selected when this run announces none.
            stale = root / "rust-server-tests/target/prolific-tests/old/report.json"
            stale.parent.mkdir(parents=True)
            stale.write_text('{"stale": true}')
            completed = subprocess.run(
                ["make", "-f", str(ROOT / "Makefile"), "-o", "test-e2e-logging", "test-e2e",
                 f"PARLANDO_DIR={root}", f"MAKE={fake}", f"PYTHON={sys.executable}"],
                cwd=root,
                env={**os.environ, "PATH": f"{root / 'bin'}:{os.environ['PATH']}",
                     "CONTRACT_STATUS": str(contract_status)},
                text=True, capture_output=True,
            )
            self.assertTrue((root / "target/e2e-runs").is_dir(), completed.stdout + completed.stderr)
            run_dirs = list((root / "target/e2e-runs").iterdir())
            self.assertEqual(len(run_dirs), 1, completed.stderr)
            run_dir = run_dirs[0]
            files = {path.name: path.read_text() for path in run_dir.iterdir()}
            self.assertTrue("report.md" in files, completed.stdout + completed.stderr)
            self.assertEqual(files["report.md"], (root / "target/e2e-report.md").read_text())
            self.assertIn(f"Python {sys.version.split()[0]} (`{sys.executable}`)", files["report.md"])
            for layer in ["javascript", "rust", "contracts", "browser", "prolific"]:
                self.assertIn("stdout:", files[f"{layer}.log"])
                self.assertIn("stderr diagnostic", files[f"{layer}.log"])
                self.assertIn(str(run_dir / f"{layer}.log"), files["report.md"])
            self.assertIn("Current browser report", files["report.md"])
            self.assertIn("No Prolific report was produced", files["report.md"])
            return completed.returncode, files, json.loads(files["statuses.json"])

    def test_failure_survives_tee_and_later_layers_still_run(self):
        """A contract failure retains its diagnostics and fails the overall Make target."""
        status, files, statuses = self.run_coordinator(101)
        self.assertNotEqual(status, 0)
        self.assertEqual(statuses["contracts"], 101)
        self.assertEqual(statuses["prolific"], 0)
        self.assertIn("failures: assertion evidence", files["contracts.log"])
        self.assertIn("Overall result: **FAILED**", files["report.md"])

    def test_success_retains_logs_and_returns_zero(self):
        """A successful coordinator records every layer and returns success."""
        status, files, statuses = self.run_coordinator(0)
        self.assertEqual(status, 0)
        self.assertTrue(all(value == 0 for value in statuses.values()))
        self.assertIn("Overall result: **PASSED**", files["report.md"])


if __name__ == "__main__":
    unittest.main()
