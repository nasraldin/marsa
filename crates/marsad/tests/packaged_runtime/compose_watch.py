"""Use the pinned upstream Hugo server to exercise Compose Watch and LiveReload."""

import base64
import hashlib
import io
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time

HUGO_RELEASE = "0.170.1"
HUGO_ARCHIVE = f"hugo_{HUGO_RELEASE}_linux-arm64.tar.gz"
HUGO_SHA256 = "6f57f13bc47c000c0d36c7f5a446d6c06f7956d8e102cc2a13735e0c0225b2d2"
IMAGE = "marsa-compose-watch:fixture"
ADDRESS = "127.0.0.1"
PORT = "1313"


def verify_hugo(archive):
    digest = hashlib.sha256()
    with archive.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    actual = digest.hexdigest()
    assert actual == HUGO_SHA256, f"Hugo archive checksum mismatch: {actual}"


def fetch_hugo(directory):
    directory.mkdir(parents=True, exist_ok=True)
    archive = directory / HUGO_ARCHIVE
    if not archive.exists():
        url = f"https://github.com/gohugoio/hugo/releases/download/v{HUGO_RELEASE}/{HUGO_ARCHIVE}"
        with tempfile.TemporaryDirectory(dir=directory) as stage:
            partial = Path(stage) / HUGO_ARCHIVE
            subprocess.run(
                ["/usr/bin/curl", "--fail", "--silent", "--show-error", "--location",
                 "--retry", "3", "--proto", "=https", "--proto-redir", "=https",
                 "--max-time", "90", "--output", partial, url],
                check=True,
            )
            verify_hugo(partial)
            partial.replace(archive)
    verify_hugo(archive)
    return archive.resolve()


def make_image(output, rootfs, hugo_archive):
    def add(archive, name, contents, mode):
        entry = tarfile.TarInfo(name)
        entry.size = len(contents)
        entry.mode = mode
        archive.addfile(entry, io.BytesIO(contents))

    verify_hugo(hugo_archive)
    with tarfile.open(output, "w") as image, tarfile.open(rootfs) as guest, tarfile.open(hugo_archive) as hugo:
        for name in ("bin/busybox", "lib/ld-musl-aarch64.so.1"):
            add(image, name, guest.extractfile("./" + name).read(), 0o755)
        add(image, "bin/hugo", hugo.extractfile("hugo").read(), 0o755)
        for name in ("stat", "mkdir", "rmdir"):
            entry = tarfile.TarInfo("bin/" + name)
            entry.type = tarfile.SYMTYPE
            entry.linkname = "busybox"
            image.addfile(entry)
        content = tarfile.TarInfo("site/content")
        content.type = tarfile.DIRTYPE
        content.mode = 0o755
        image.addfile(content)
        add(image, "site/hugo.toml", b'baseURL = "http://localhost/"\n', 0o644)
        add(image, "site/layouts/home.html",
            b"<!doctype html><html><body><main>{{ .Content }}</main></body></html>", 0o644)


def expect_eventually(description, check):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.2)
    raise AssertionError(description)


class LiveReload:
    """Read the browser protocol through an interactive, container-local connection."""

    def __init__(self, stream):
        self.stream = stream
        self.buffer = bytearray()

    def read(self, deadline):
        remaining = max(0, deadline - time.monotonic())
        fd = self.stream.stdout.fileno()
        assert select.select([fd], [], [], remaining)[0], "Hugo LiveReload timed out"
        chunk = os.read(fd, 4096)
        assert chunk, "Hugo LiveReload connection closed"
        self.buffer.extend(chunk)

    def take(self, length, deadline):
        while len(self.buffer) < length:
            self.read(deadline)
        result = bytes(self.buffer[:length])
        del self.buffer[:length]
        return result

    def message(self):
        deadline = time.monotonic() + 15
        opcode, length = self.take(2, deadline)
        assert opcode == 0x81 and length < 128, (opcode, length)
        if length == 126:
            length = int.from_bytes(self.take(2, deadline), "big")
        assert length <= 4096, "unexpectedly large LiveReload message"
        return json.loads(self.take(length, deadline))

    def handshake(self):
        key = base64.b64encode(os.urandom(16))
        self.stream.stdin.write(
            b"GET /livereload HTTP/1.1\r\nHost: 127.0.0.1:1313\r\n"
            b"Upgrade: websocket\r\nConnection: Upgrade\r\n"
            b"Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: " + key + b"\r\n\r\n"
        )
        self.stream.stdin.flush()
        deadline = time.monotonic() + 10
        while b"\r\n\r\n" not in self.buffer:
            assert len(self.buffer) < 4096, "oversized WebSocket handshake"
            self.read(deadline)
        length = self.buffer.index(b"\r\n\r\n") + 4
        headers = self.take(length, deadline)
        assert headers.startswith(b"HTTP/1.1 101 "), headers

        hello = b'{"command":"hello","protocols":["http://livereload.com/protocols/official-7"]}'
        mask = os.urandom(4)
        masked = bytes(byte ^ mask[index % 4] for index, byte in enumerate(hello))
        self.stream.stdin.write(bytes([0x81, 0x80 | len(hello)]) + mask + masked)
        self.stream.stdin.flush()
        response = self.message()
        assert response["command"] == "hello" and response["serverName"] == "Hugo", response


def test_watch(root, docker_socket, rootfs, archive):
    docker_path = shutil.which("docker")
    assert docker_path, "put the verified Docker CLI on PATH"
    compose_plugin = Path(docker_path).parent / "cli-plugins/docker-compose"
    assert compose_plugin.is_file(), "put the verified Compose plugin alongside Docker"
    config = root / "docker-config"
    config.mkdir(mode=0o700)
    (config / "config.json").write_text(json.dumps({"cliPluginsExtraDirs": [str(compose_plugin.parent)]}))
    environment = {"HOME": str(root / "home"), "PATH": "/usr/bin:/bin", "LC_ALL": "C"}
    docker = [docker_path, "--config", str(config), "--host", f"unix://{docker_socket}"]
    compose = [*docker, "compose", "-p", "marsahotwatch"]
    project = root / "project"
    source = project / "content"
    source.mkdir(parents=True)

    def run(*args, command=docker, check=True):
        result = subprocess.run([*command, *args], cwd=project, env=environment,
                                capture_output=True, timeout=25, text=True)
        if check:
            assert result.returncode == 0, (args, result.stdout, result.stderr)
        return result.stdout.strip()

    compose_version = run("version", "--short", command=compose)
    assert compose_version.removeprefix("v") == "5.8.0", compose_version
    engine_version = run("version", "--format", "{{.Server.Version}}")
    assert engine_version == "29.10.0", engine_version
    image = root / "watch-image.tar"
    make_image(image, rootfs, archive)
    run("import", str(image), IMAGE)
    (project / "Dockerfile").write_text(f"FROM {IMAGE}\n")
    (project / "compose.yaml").write_text(f"""services:
  site:
    image: {IMAGE}
    build: .
    pull_policy: never
    network_mode: none
    command:
      - /bin/hugo
      - server
      - --source
      - /site
      - --bind
      - {ADDRESS}
      - --port
      - "{PORT}"
      - --renderToMemory
      - --disableFastRender
    develop:
      watch:
        - action: sync
          path: ./content
          target: /site/content
          initial_sync: true
          ignore:
            - ignored.md
""")
    (project / "private-sentinel.txt").write_text("outside the watched directory")
    (root / "outside-project.txt").write_text("outside the project")
    (source / "ignored.md").write_text("excluded from syncing")
    index = source / "_index.md"

    def page(body):
        return f'+++\ntitle = "Fixture"\n+++\n\n{body}\n'

    index.write_text(page("initial-source"))
    run("up", "-d", "--no-build", command=compose)
    container = run("ps", "-q", "site", command=compose)

    def in_container(*args, check=True):
        return run("exec", container, "/bin/busybox", *args, check=check)

    def wait_for_page(text):
        return expect_eventually(f"Hugo did not serve {text}", lambda: text in in_container(
            "wget", "-qO-", f"http://{ADDRESS}:{PORT}/", check=False))

    def check_scope():
        assert in_container("ls", "-A", "/site/content") == "_index.md"
        assert "private-sentinel.txt" not in in_container("ls", "-A", "/site").splitlines()
        assert "outside-project.txt" not in in_container("ls", "-A", "/").splitlines()
        mounts = run("inspect", "-f", "{{json .Mounts}}", container)
        assert json.loads(mounts) == [], mounts

    log_path = root / "watch.log"
    with log_path.open("w") as log:
        watcher = subprocess.Popen([*compose, "watch", "--no-up"],
                                   cwd=project, env=environment, stdout=log, stderr=subprocess.STDOUT)
        try:
            expect_eventually("Compose Watch did not start", lambda: "Watch enabled" in log_path.read_text())
            wait_for_page("initial-source")
            version = run("exec", container, "/bin/hugo", "version")
            assert version.startswith(f"hugo v{HUGO_RELEASE}"), version
            print(f"Docker {engine_version}, Compose {compose_version}, {version}")
            check_scope()
            started = run("inspect", "-f", "{{.State.StartedAt}}", container)

            def reload_after(edit, expected):
                command = [*docker, "exec", "-i", container, "/bin/busybox", "nc", ADDRESS, PORT]
                with subprocess.Popen(command, env=environment, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE) as client:
                    try:
                        browser = LiveReload(client)
                        browser.handshake()
                        edit()
                        received = browser.message()
                        assert received["command"] == "reload", received
                        wait_for_page(expected)
                        print(f"Hugo sent a LiveReload frame and served {expected}: {received}")
                    finally:
                        client.kill()
                        client.wait(timeout=5)

            reload_after(lambda: index.write_text(page("ordinary-save")), "ordinary-save")

            def atomic_save():
                draft = project / "editor-save.tmp"
                draft.write_text(page("atomic-save"))
                os.replace(draft, index)

            reload_after(atomic_save, "atomic-save")
            check_scope()
            assert run("inspect", "-f", "{{.State.StartedAt}} {{.State.Running}}", container) == f"{started} true"
            print("Initial content synced. Hugo stayed running. Ignored and out-of-scope files stayed absent.")
        finally:
            if watcher.poll() is None:
                watcher.terminate()
            try:
                watcher.wait(timeout=5)
            except subprocess.TimeoutExpired:
                watcher.kill()
                watcher.wait(timeout=5)
            print(log_path.read_text())
            print(run("logs", container, check=False))
            run("down", "--timeout", "1", command=compose, check=False)


if __name__ == "__main__":
    if sys.argv[1] == "fetch":
        print(fetch_hugo(Path(sys.argv[2])))
    else:
        scratch, socket, rootfs, archive = map(Path, sys.argv[1:])
        test_watch(scratch, socket, rootfs, archive)
