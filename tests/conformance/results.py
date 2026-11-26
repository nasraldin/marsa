"""Publish test outcomes from Rust's serial test harness without bundling daemon logs."""
import json
import os
from pathlib import Path
import re
import sys

directory = Path(sys.argv[1])
directory.mkdir(parents=True, exist_ok=True)
reports = {}
for suite in ("keychain", "packaged"):
    log = directory / f"{suite}.log"
    lines = log.read_text(errors="replace").splitlines() if log.exists() else []
    tests = {}
    completed = False
    for line in lines:
        result = re.fullmatch(r"test ([a-zA-Z_][a-zA-Z0-9_:]*) \.\.\. (ok|FAILED)", line)
        if result:
            tests[result[1]] = result[2]
        if line.startswith("test result: ok.") and tests:
            completed = True
    reports[suite] = {"passed": completed and all(outcome == "ok" for outcome in tests.values()), "tests": tests}
(directory / "results.json").write_text(json.dumps({"commit": os.getenv("GITHUB_SHA"), "suites": reports}, indent=2) + "\n")
