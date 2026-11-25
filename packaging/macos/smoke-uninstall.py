#!/usr/bin/env python3
"""Verify packaged uninstall in a disposable, logged-in macOS environment."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

BUNDLE_ID = "io.github.nasraldin.marsa"
AGENT_LABEL = f"{BUNDLE_ID}.marsad"
AGENT_FILE = Path(__file__).parent / "LaunchAgents" / f"{AGENT_LABEL}.plist"
SYSTEM_SOCKET = Path("/var/run/docker.sock")


def checked(*args: str | Path, **options) -> str:
    result = subprocess.run(
        args, capture_output=True, text=True, timeout=40, **options
    )
    if result.returncode:
        raise RuntimeError(f"{Path(args[0]).name}: {result.stderr.strip()}")
    return result.stdout.strip()


def stage_app(app: Path, daemon: Path, docker: Path, home: Path) -> Path:
    executable = app / "Contents/MacOS"
    executable.mkdir(parents=True)
    resources = app / "Contents/Resources/bin"
    resources.mkdir(parents=True)
    agents = app / "Contents/Library/LaunchAgents"
    agents.mkdir(parents=True)
    plist = {
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleExecutable": "marsad",
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": "1",
        "LSUIElement": True,
    }
    (app / "Contents/Info.plist").write_bytes(plistlib.dumps(plist))
    agent = plistlib.loads(AGENT_FILE.read_bytes())
    agent["EnvironmentVariables"] = {
        "HOME": str(home),
        "DOCKER_CONFIG": str(home / ".docker"),
    }
    (agents / AGENT_FILE.name).write_bytes(plistlib.dumps(agent))
    binary = executable / "marsad"
    shutil.copy2(daemon, binary)
    shutil.copy2(docker, resources / "docker")
    for path in (binary, resources / "docker", app):
        checked("/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", path)
    checked("/usr/bin/codesign", "--verify", "--deep", "--strict", app)
    return binary


def launchd_has_job(job: str) -> bool:
    return subprocess.run(
        ["/bin/launchctl", "print", job], capture_output=True, timeout=12
    ).returncode == 0


def wait_until(predicate, description: str) -> None:
    for _ in range(100):
        if predicate():
            return
        time.sleep(0.2)
    raise RuntimeError(description)


def link_target(path: Path) -> str | None:
    return os.readlink(path) if path.is_symlink() else None


def verify(daemon: Path, docker: Path) -> None:
    real_home = Path.home()
    if os.getuid() == 0 or checked("/bin/launchctl", "managername") != "Aqua":
        raise RuntimeError("run from the desktop session of a disposable macOS VM")
    job = f"gui/{os.getuid()}/{AGENT_LABEL}"
    if launchd_has_job(job) or any(
        path.exists()
        for path in (Path("/Applications/Marsa.app"), real_home / "Applications/Marsa.app")
    ):
        raise RuntimeError("the disposable test machine must have no Marsa installation")

    with tempfile.TemporaryDirectory(prefix="marsa-uninstall-", dir="/tmp") as scratch:
        root = Path(scratch).resolve()
        home = root / "home"
        home.mkdir(mode=0o700)
        app = root / "Applications/Marsa.app"
        binary = stage_app(app, daemon, docker, home)
        docker_config = home / ".docker"
        socket = home / ".marsa/run/docker.sock"
        environment = dict(
            os.environ, HOME=str(home), DOCKER_CONFIG=str(docker_config), LC_ALL="en_US.UTF-8"
        )
        environment.pop("DOCKER_CONTEXT", None)
        environment.pop("DOCKER_HOST", None)

        def run(*args, **options):
            return checked(*args, env=environment, **options)

        def contexts():
            return set(run(docker, "context", "ls", "--format", "{{.Name}}").splitlines())

        assert shutil.which("marsa", path=environment["PATH"]) is None
        assert run(binary, "login", "status") == "disabled"
        try:
            assert run(binary, "login", "enable") == "enabled"
            wait_until(lambda: socket.is_socket() and launchd_has_job(job), "daemon did not start")
            run(docker, "context", "create", "marsa", "--description", "Marsa", "--docker", f"host=unix://{socket}")
            run(docker, "context", "create", "retained-fixture", "--docker", "host=ssh://fixture.invalid")
            run(docker, "context", "use", "marsa")
            refusal = subprocess.run(
                [binary, "uninstall"], input="UNINSTALL MARSA\n", env=environment,
                capture_output=True, text=True, timeout=12,
            )
            assert refusal.returncode and "different context" in refusal.stderr
            assert socket.is_socket() and launchd_has_job(job)
            assert "marsa" in contexts()

            run(docker, "context", "use", "retained-fixture")
            previous_contexts = contexts()
            assert "marsa" in previous_contexts
            config = (docker_config / "config.json").read_bytes()
            own_cache = home / f"Library/Caches/{BUNDLE_ID}"
            own_cache.mkdir(parents=True)
            (own_cache / "fixture").write_text("temporary")
            foreign_cache = own_cache.with_name("example.other-app")
            foreign_cache.mkdir()
            (foreign_cache / "fixture").write_text("keep")
            before_socket = link_target(SYSTEM_SOCKET)

            uninstalled = run(binary, "uninstall", input="UNINSTALL MARSA\n")
            assert "Remove the app" in uninstalled
            wait_until(lambda: not launchd_has_job(job), "login agent remained loaded")
            assert run(binary, "login", "status") == "disabled"
            assert not own_cache.exists()
            assert not (home / ".marsa").exists()
            assert (foreign_cache / "fixture").read_text() == "keep"
            assert contexts() == previous_contexts - {"marsa"}
            assert (docker_config / "config.json").read_bytes() == config
            assert link_target(SYSTEM_SOCKET) == before_socket

            run(binary, "uninstall", input="UNINSTALL MARSA\n")
            assert not (home / ".marsa").exists()
            shutil.rmtree(app)
            assert not app.exists()
            assert shutil.which("marsa", path=environment["PATH"]) is None
        finally:
            if app.exists():
                run(binary, "login", "disable")
                wait_until(lambda: not launchd_has_job(job), "fixture login agent remained loaded")
    print("Marsa uninstall removed the app fixture, login job, owner state and matching Docker context")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("marsad", type=Path)
    parser.add_argument("docker", type=Path)
    args = parser.parse_args()
    verify(args.marsad.resolve(strict=True), args.docker.resolve(strict=True))
