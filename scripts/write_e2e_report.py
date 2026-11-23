#!/usr/bin/env python3
"""Combine deterministic and end-to-end test layers into one release report."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
COMBINED_REPORT = ROOT / "target" / "e2e-report.md"


def prolific_report_from_log(log_path: Path) -> Path | None:
    """Resolve only the report announced by this run's Prolific process."""
    matches = re.findall(r"^JSON report: (.+)$", log_path.read_text(), re.MULTILINE)
    if not matches:
        return None
    path = Path(matches[-1].strip())
    if not path.is_absolute():
        path = ROOT / "rust-server-tests" / path
    return path


def prolific_markdown(path: Path | None) -> str:
    """Render the standalone Prolific JSON artifact as a human-readable table."""
    if path is None:
        return "## Prolific integration\n\nNo Prolific report was produced.\n"
    report = json.loads(path.read_text())
    lines = [
        "## Prolific integration",
        "",
        f"Result: **{report['passed']} passed, {report['failed']} failed, {report['skipped']} skipped** "
        f"in {report['elapsed_ms']} ms.",
        "",
        "| ID | Phase | Status | Time | Scenario | Detail |",
        "|---|---|---:|---:|---|---|",
    ]
    for scenario in report["scenarios"]:
        detail = (scenario.get("detail") or "").replace("|", "\\|").replace("\n", " ")
        lines.append(
            f"| {scenario['id']} | {scenario['phase']} | {scenario['status']} | "
            f"{scenario['elapsed_ms']} ms | {scenario['name']} | {detail} |"
        )
    lines.extend(["", f"Machine-readable report: `{path}`", ""])
    return "\n".join(lines)


def main() -> int:
    """Save one immutable run report and refresh the latest report with links to layer logs."""
    if len(sys.argv) != 7:
        raise SystemExit(
            "usage: write_e2e_report.py RUN_DIRECTORY JS_STATUS RUST_STATUS CONTRACT_STATUS "
            "BROWSER_STATUS PROLIFIC_STATUS"
        )
    run_dir = Path(sys.argv[1]).resolve()
    layers = [
        ("JavaScript client unit, rendering, coverage, and package tests", "javascript"),
        ("Rust server unit and integration tests", "rust"),
        ("Live client/server contract tests", "contracts"),
        ("Real-browser state-machine matrix", "browser"),
        ("Prolific process-boundary matrix", "prolific"),
    ]
    statuses = dict(zip((key for _, key in layers), map(int, sys.argv[2:])))
    browser_report = run_dir / "browser-report.md"
    browser_text = (
        browser_report.read_text()
        if statuses["browser"] == 0 and browser_report.is_file()
        else "# Parlando browser end-to-end report\n\nNo successful browser report was produced by this run.\n"
    )
    overall = "PASSED" if all(status == 0 for status in statuses.values()) else "FAILED"
    summary = [
        f"- {name}: **{'passed' if statuses[key] == 0 else 'failed'}** "
        f"(exit {statuses[key]}) — [full log]({run_dir / (key + '.log')})"
        for name, key in layers
    ]
    text = "\n".join(
        [
            "# Parlando end-to-end release report",
            "",
            f"Overall result: **{overall}**",
            "",
            f"Report interpreter: Python {sys.version.split()[0]} (`{sys.executable}`)",
            "",
            *summary,
            "",
            browser_text.replace("# Parlando browser end-to-end report", "## Real-browser state machine", 1),
            prolific_markdown(prolific_report_from_log(run_dir / "prolific.log")),
        ]
    )
    COMBINED_REPORT.parent.mkdir(parents=True, exist_ok=True)
    (run_dir / "report.md").write_text(text)
    (run_dir / "statuses.json").write_text(json.dumps(statuses, indent=2) + "\n")
    COMBINED_REPORT.write_text(text)
    print(f"Run report and logs: {run_dir / 'report.md'}")
    print(f"Combined human-readable report: {COMBINED_REPORT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
