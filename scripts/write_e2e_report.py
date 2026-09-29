#!/usr/bin/env python3
"""Combine deterministic and end-to-end test layers into one release report."""

from __future__ import annotations

import json
from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent.parent
BROWSER_REPORT = ROOT / "client-server-tests" / "target" / "browser-e2e" / "report.md"
PROLIFIC_ARTIFACTS = ROOT / "rust-server-tests" / "target" / "prolific-tests"
COMBINED_REPORT = ROOT / "target" / "e2e-report.md"


def latest_prolific_report() -> Path | None:
    """Return the newest durable Prolific report available after the matrix run."""
    reports = list(PROLIFIC_ARTIFACTS.glob("*/report.json"))
    return max(reports, key=lambda path: path.stat().st_mtime) if reports else None


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
    """Write the combined report from every exit status supplied by the shell coordinator."""
    if len(sys.argv) != 6:
        raise SystemExit(
            "usage: write_e2e_report.py JS_STATUS RUST_STATUS CONTRACT_STATUS "
            "BROWSER_STATUS PROLIFIC_STATUS"
        )
    statuses = {
        "JavaScript client unit, rendering, coverage, and package tests": int(sys.argv[1]),
        "Rust server unit and integration tests": int(sys.argv[2]),
        "Live client/server contract tests": int(sys.argv[3]),
        "Real-browser state-machine matrix": int(sys.argv[4]),
        "Prolific process-boundary matrix": int(sys.argv[5]),
    }
    browser_status = statuses["Real-browser state-machine matrix"]
    prolific_status = statuses["Prolific process-boundary matrix"]
    browser_text = (
        BROWSER_REPORT.read_text()
        if BROWSER_REPORT.is_file()
        else "# Parlando browser end-to-end report\n\nNo browser report was produced.\n"
    )
    overall = "PASSED" if all(status == 0 for status in statuses.values()) else "FAILED"
    summary = [
        f"- {name}: {'passed' if status == 0 else 'failed'}"
        for name, status in statuses.items()
    ]
    text = "\n".join(
        [
            "# Parlando end-to-end release report",
            "",
            f"Overall result: **{overall}**",
            "",
            *summary,
            "",
            browser_text.replace("# Parlando browser end-to-end report", "## Real-browser state machine", 1),
            prolific_markdown(latest_prolific_report()),
        ]
    )
    COMBINED_REPORT.parent.mkdir(parents=True, exist_ok=True)
    COMBINED_REPORT.write_text(text)
    print(f"Combined human-readable report: {COMBINED_REPORT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
