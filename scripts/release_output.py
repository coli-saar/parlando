"""Report Make release steps and retain their combined output in a per-run log."""

import argparse
from collections import deque
import json
import os
from pathlib import Path
import pty
import re
import shlex
import subprocess
import sys
import tempfile
import time


def registry_output(command):
    """Log registry output and relay prompts immediately, retaining a terminal for interactive clients."""
    console = int(os.environ["PARLANDO_RELEASE_CONSOLE"])
    verbose = os.environ.get("VERBOSE") == "1"

    if os.isatty(0) and os.isatty(console):
        # pty.spawn multiplexes terminal input and output. Return each chunk so
        # it can service input immediately, while its stdout copy goes to the log.
        def read_interactive(fd):
            """Show interactive output before pty.spawn appends it to the step log."""
            chunk = os.read(fd, 65536)
            if chunk and not verbose:
                show_registry(console, chunk)
            return chunk

        return os.waitstatus_to_exitcode(pty.spawn(command, master_read=read_interactive))

    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
        while chunk := child.stdout.read1(65536):
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
            if not verbose:
                show_registry(console, chunk)
        return child.wait()


def show_registry(console, chunk):
    """Display registry prompts, URLs and diagnostics without routine compiler or tarball listings."""
    for line in chunk.splitlines(keepends=True):
        routine = re.match(rb"\s*(?:(?:Compiling|Checking|Packaging|Packaged|Fresh) |npm notice (?:[0-9.]+\s*[kMG]?B\s|Tarball|package size:))", line)
        if routine and line.endswith((b"\r", b"\n")):
            continue
        os.write(console, line)


def run_release(mode, make, steps):
    """Run Make steps in order, retaining full logs and stopping on the first failure."""
    root = Path(__file__).resolve().parents[1]
    parent = root / "target/release-logs"
    parent.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix=time.strftime("%Y%m%d-%H%M%S-"), dir=parent))
    path = directory / f"{mode}.log"
    version = json.loads((root / "js-client/package.json").read_text())["version"]
    print(f"Parlando {version} {mode}\nFull log: {path}\n", flush=True)
    verbose = os.environ.get("VERBOSE") == "1"
    # Make logs ordinary build output; registry clients also need the original console.
    console = os.dup(sys.stdout.fileno())
    env = {**os.environ, "PARLANDO_RELEASE_CONSOLE": str(console),
           "CARGO_TERM_PROGRESS_WHEN": "never", "CARGO_TERM_COLOR": "never"}
    prefix = shlex.join([sys.executable, str(Path(__file__).resolve()), "--registry"])
    with path.open("w+b", buffering=0) as log:
        for label, target in steps:
            start = time.monotonic()
            print(f"{label}…", flush=True)
            log.write(f"\n--- {label} ---\n".encode())
            offset = log.tell()
            command = [make, target]
            if mode == "publish":
                command.append(f"REGISTRY_RUN={prefix}")
            with subprocess.Popen(command, cwd=root, env=env, pass_fds=(console,),
                                  stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
                while chunk := child.stdout.read1(65536):
                    log.write(chunk)
                    if verbose:
                        sys.stdout.buffer.write(chunk)
                        sys.stdout.buffer.flush()
                status = child.wait()
            elapsed = round(time.monotonic() - start)
            duration = f"{elapsed // 60}m {elapsed % 60:02d}s" if elapsed >= 60 else f"{elapsed}s"
            summary = f"{'FAILED' if status else '✓'} {label} ({duration})"
            # In a quiet interactive test run the progress line is still the last line.
            replace = sys.stdout.isatty() and not verbose and mode == "pre-release"
            print(("\033[1A\r\033[2K" if replace else "") + summary, flush=True)
            if status:
                log.seek(offset)
                for line in deque(log, maxlen=20):
                    sys.stderr.buffer.write(line)
                print(f"\nFull log: {path}", file=sys.stderr)
                if mode == "publish" and target.startswith("publish-"):
                    print(f"Check the registry, then retry the pending package with make {target}.", file=sys.stderr)
            log.write(f"\n{summary} [exit {status}]\n".encode())
            if status:
                return status if status > 0 else 128 - status
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["--registry"]:
        sys.exit(registry_output(sys.argv[2:]))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["pre-release", "publish"])
    parser.add_argument("make")
    parser.add_argument("--step", nargs=2, action="append", required=True)
    args = parser.parse_args()
    sys.exit(run_release(args.mode, args.make, args.step))
