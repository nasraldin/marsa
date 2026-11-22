"""Exercise live macOS DNS answer changes from two existing Docker containers."""

import io
from pathlib import Path
import re
import select
import shutil
import socket
import subprocess
import sys
import tarfile
import time
import uuid

INITIAL = "192.0.2.64"
UPDATED = "192.0.2.65"
IMAGE = "marsa-fallback-dns:fixture"


def make_image(path, rootfs):
    with tarfile.open(path, "w") as image, tarfile.open(rootfs) as guest:
        for name in ("bin/busybox", "lib/ld-musl-aarch64.so.1"):
            contents = guest.extractfile("./" + name).read()
            entry = tarfile.TarInfo(name)
            entry.size = len(contents)
            entry.mode = 0o755
            image.addfile(entry, io.BytesIO(contents))


def wait_for(description, check):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.2)
    raise AssertionError(description)


def main(scratch, docker_socket, rootfs):
    docker_cli = shutil.which("docker")
    assert docker_cli, "Put the pinned Docker CLI on PATH"
    config = scratch / "docker-config"
    config.mkdir(mode=0o700)
    # Keep the real user's Docker context, credentials and environment out of the fixture.
    docker = [docker_cli, "--config", str(config), "--host", f"unix://{docker_socket}"]
    environment = {
        "HOME": str(scratch / "home"),
        "LC_ALL": "C",
        "PATH": "/usr/bin:/bin",
    }

    def run(*args, check=True):
        return subprocess.run(
            [*docker, *args],
            env=environment,
            check=check,
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.strip()

    image = scratch / "dns-image.tar"
    make_image(image, rootfs)
    run("import", str(image), IMAGE)
    run("network", "create", "fixture_dns")
    containers = [
        run("run", "-d", "--network", network, IMAGE, "/bin/busybox", "sleep", "120")
        for network in ("bridge", "fixture_dns")
    ]

    def container_dns(container, name, address):
        result = run("exec", container, "/bin/busybox", "nslookup", "-type=A", name, check=False)
        addresses = re.findall(r"^Address(?: \d+)?:\s+([^\s#]+)", result, re.M)
        return address in addresses and (UPDATED if address == INITIAL else INITIAL) not in addresses

    def host_dns(name, address):
        try:
            return socket.gethostbyname(name) == address
        except socket.gaierror:
            return False

    def observe(name, address):
        wait_for(f"macOS did not return {address}", lambda: host_dns(name, address))
        for container in containers:
            wait_for(
                f"container {container[:12]} did not return {address}",
                lambda: container_dns(container, name, address),
            )

    name = f"marsa-dns-{uuid.uuid4().hex}.local."
    record = scratch / "dns_record"
    subprocess.run(
        ["/usr/bin/clang", "-Wall", "-Wextra", "-Werror",
         str(Path(__file__).with_name("dns_record.c")), "-o", str(record)],
        check=True,
        timeout=20,
    )
    started = [run("inspect", "-f", "{{.State.StartedAt}}", container) for container in containers]
    with subprocess.Popen(
        [str(record), name], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
    ) as registration:
        def expect(expected):
            assert select.select([registration.stdout], [], [], 5)[0], "DNS fixture timed out"
            assert registration.stdout.readline().strip() == expected, expected

        try:
            expect("registered")
            observe(name, INITIAL)
            print(f"macOS and both Docker network modes resolved the initial local A record {INITIAL}")
            registration.stdin.write("\n")
            registration.stdin.flush()
            expect("updated")
            observe(name, UPDATED)
            print(f"macOS and both running containers resolved the updated local A record {UPDATED}")
        finally:
            registration.stdin.close()
            try:
                registration.wait(timeout=5)
            except subprocess.TimeoutExpired:
                registration.kill()
                registration.wait(timeout=5)
        assert registration.returncode == 0, registration.returncode

    for container, began in zip(containers, started, strict=True):
        assert run("inspect", "-f", "{{.State.StartedAt}} {{.State.Running}}", container) == f"{began} true"
    assert run("version", "--format", "{{.Server.Version}}") == "29.10.0"
    print("The same containers stayed running and Docker Engine 29.10.0 responded after the change")


if __name__ == "__main__":
    scratch, docker_socket, rootfs = sys.argv[1:]
    main(Path(scratch), docker_socket, Path(rootfs))
