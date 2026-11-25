"""Exercise the release Make entry points using local fixture steps and a fake registry."""

import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]


class ReleaseLoggingTests(unittest.TestCase):
    """Run the real release coordinator without building or uploading a package."""

    def setUp(self):
        """Give each test an isolated checkout and substitute the nested Make targets."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        shutil.copy(ROOT / "Makefile", self.root)
        (self.root / "scripts").mkdir()
        shutil.copy(ROOT / "scripts/release_output.py", self.root / "scripts")
        (self.root / "js-client").mkdir()
        (self.root / "js-client/package.json").write_text('{"version":"0.7.0"}')
        self.make = self.root / "fixture-make"
        self.make.write_text('#!/bin/sh\nexec make -s -f fixture.mk "$@"\n')
        self.make.chmod(0o755)
        (self.root / "fixture.mk").write_text(
            'publish-rust-derive:\n\t$(REGISTRY_RUN) "$(PYTHON)" registry.py\n'
            '%:\n\t@echo "fixture output for $@"\n'
            '\t@if test "$@" = "$(FAIL_TARGET)"; then seq 1 25; exit 7; fi\n'
        )
        (self.root / "registry.py").write_text(
            'import sys\n'
            'assert all(s.isatty() for s in (sys.stdin, sys.stdout, sys.stderr))\n'
            'print("Compiling fixture", flush=True)\n'
            'assert input("Approve at https://example.test/approval? ") == "yes"\n'
        )

    def command(self, target, *options):
        """Build a top-level Make invocation whose nested commands use the fixture."""
        return ["make", target, f"MAKE={self.make}", f"PYTHON={sys.executable}", *options]

    def test_quiet_and_verbose(self):
        """Check identical step coverage and durable command output in both modes."""
        for verbose in ("0", "1"):
            result = subprocess.run(self.command("test-release-candidate", f"VERBOSE={verbose}"),
                                    cwd=self.root, capture_output=True, text=True, check=True)
            self.assertIn("✓ Test packaged games and agents", result.stdout)
            self.assertEqual("fixture output for release-rehearsal" in result.stdout, verbose == "1")
            path = Path(result.stdout.split("Full log: ")[1].splitlines()[0])
            self.assertIn("fixture output for release-rehearsal", path.read_text())

    def test_failure_tail(self):
        """Keep earlier output in the log and stop after the failed step."""
        result = subprocess.run(self.command("test-release-candidate", "FAIL_TARGET=test-e2e"),
                                cwd=self.root, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAILED Browser and Prolific scenarios", result.stdout)
        self.assertIn("\n25\n", result.stderr)
        self.assertNotIn("\n5\n", result.stderr)
        self.assertNotIn("Component compatibility", result.stdout)

    def test_interactive_upload(self):
        """Answer a registry prompt before its newline and check the exact package retry."""
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        command = self.command("publish", "FAIL_TARGET=publish-rust-sdk")
        process = subprocess.Popen(command, cwd=self.root, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        output = b""
        answered = False
        try:
            deadline = time.monotonic() + 10
            while select.select([master], [], [], max(0, deadline - time.monotonic()))[0]:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    break
                if not data:
                    break
                output += data
                if not answered and b"https://example.test/approval? " in output:
                    os.write(master, b"yes\n")
                    answered = True
            self.assertTrue(answered, output)
            self.assertNotEqual(process.wait(timeout=1), 0)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        self.assertIn("✓ Publish parlando-game-derive".encode(), output)
        self.assertIn(b"make publish-rust-sdk", output)
        self.assertNotIn(b"Compiling fixture", output)
        log = next(self.root.glob("target/release-logs/*/publish.log")).read_text()
        self.assertIn("Compiling fixture", log)
