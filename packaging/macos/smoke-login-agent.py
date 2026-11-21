#!/usr/bin/env python3
"""Verify real Service Management registration with an isolated ad-hoc app."""

import argparse
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import tempfile
import time
import uuid


APPLE_TOOLS = Path("/usr/bin")
LAUNCHCTL = Path("/bin/launchctl")
AGENT = Path(__file__).with_name("LaunchAgents") / "io.github.nasraldin.marsa.marsad.plist"


def run(*args: str | Path) -> subprocess.CompletedProcess:
    return subprocess.run(args, capture_output=True, text=True, timeout=12)


def checked(*args: str | Path) -> str:
    result = run(*args)
    if result.returncode:
        raise RuntimeError(f"{Path(args[0]).name}: {result.stderr.strip()}")
    if result.stderr:
        print(result.stderr.strip(), flush=True)
    return result.stdout.strip()


def signed_app(binary: Path, fixture: Path, identifier: str) -> Path:
    contents = fixture / "Marsa Login Validation.app" / "Contents"
    executable_dir = contents / "MacOS"
    agent_dir = contents / "Library" / "LaunchAgents"
    executable_dir.mkdir(parents=True)
    agent_dir.mkdir(parents=True)
    app = {
        "CFBundleIdentifier": identifier,
        "CFBundleExecutable": "marsa-gui",
        "CFBundleName": "Marsa Login Validation",
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": "1",
        "CFBundleShortVersionString": "0.1.0",
        "LSMinimumSystemVersion": "27.0",
        "LSUIElement": True,
    }
    (contents / "Info.plist").write_bytes(plistlib.dumps(app))
    agent = plistlib.loads(AGENT.read_bytes())
    agent["Label"] = f"{identifier}.marsad"
    home = fixture / "home"
    home.mkdir(mode=0o700)
    agent["EnvironmentVariables"] = {"HOME": str(home)}
    (agent_dir / f"{agent['Label']}.plist").write_bytes(plistlib.dumps(agent))
    helper = executable_dir / "marsad"
    shutil.copy2(binary, helper)
    # Match the production layout while exercising commands from the helper.
    shutil.copy2(binary, executable_dir / "marsa-gui")
    codesign = APPLE_TOOLS / "codesign"
    checked(codesign, "--force", "--sign", "-", "--timestamp=none", helper)
    checked(codesign, "--force", "--sign", "-", "--timestamp=none", contents.parent)
    checked(codesign, "--verify", "--deep", "--strict", contents.parent)
    return helper


def job_process(job: str, home: Path) -> int | None:
    result = run(LAUNCHCTL, "print", job)
    if result.returncode:
        return None
    match = re.search(r"^\s*pid = (\d+)\s*$", result.stdout, re.MULTILINE)
    try:
        run_dir = home / ".marsa" / "run"
        pid = int((run_dir / "marsad.pid").read_text().strip())
        mode = (run_dir / "marsad.sock").lstat().st_mode
    except FileNotFoundError:
        return None
    if match is not None and int(match[1]) == pid and stat.S_ISSOCK(mode):
        return pid
    return None


def wait_for_restart(job: str, home: Path, previous: int | None = None) -> int:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        pid = job_process(job, home)
        if pid is not None and pid != previous:
            return pid
        time.sleep(0.2)
    raise RuntimeError(f"the fixture daemon did not become ready: {job}")


def disabled(helper: Path, job: str) -> None:
    if checked(helper, "login", "status") != "disabled":
        raise RuntimeError("the fixture agent remained registered")
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if run(LAUNCHCTL, "print", job).returncode:
            return
        time.sleep(0.2)
    raise RuntimeError(f"the fixture agent remained loaded: {job}")


def verify(binary: Path) -> int:
    if os.getuid() == 0 or checked(LAUNCHCTL, "managername") != "Aqua":
        raise RuntimeError("run this smoke test from the logged-in macOS desktop")
    fixture = Path(tempfile.mkdtemp(prefix="marsa-login-", dir="/tmp")).resolve()
    identifier = f"io.github.nasraldin.marsa.fixture-{uuid.uuid4().hex}"
    job = f"gui/{os.getuid()}/{identifier}.marsad"
    helper = None
    verified = False
    try:
        helper = signed_app(binary, fixture, identifier)
        disabled(helper, job)
        status = checked(helper, "login", "enable")
        if status == "requires-approval":
            print(f"System Settings approval required for the disposable fixture {identifier}", flush=True)
            return 77
        if status != "enabled" or checked(helper, "login", "enable") != "enabled":
            raise RuntimeError("enabling the fixture agent was not idempotent")
        first = wait_for_restart(job, fixture / "home")
        checked(LAUNCHCTL, "kill", "SIGKILL", job)
        replacement = wait_for_restart(job, fixture / "home", first)
        print(f"Registered {job} and verified restart {first} -> {replacement}", flush=True)
        verified = True
    finally:
        if helper is not None:
            try:
                checked(helper, "login", "disable")
                disabled(helper, job)
            except Exception:
                print(f"Fixture retained for manual cleanup: {fixture}", flush=True)
                raise
        shutil.rmtree(fixture)
    if verified:
        print("Unregistered the fixture and verified launchd removed its job", flush=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("marsad", type=Path)
    args = parser.parse_args()
    return verify(args.marsad.resolve(strict=True))


if __name__ == "__main__":
    raise SystemExit(main())
