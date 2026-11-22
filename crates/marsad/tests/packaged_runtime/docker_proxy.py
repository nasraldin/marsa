"""Exercise the Docker CLI through the packaged daemon's socket and Linux guest."""

import errno
import os
from pathlib import Path
import pty
import select
import shutil
import socket
import subprocess
import sys
import tarfile
import time


WAIT = 45
STREAM_WAIT = 240
MIB = 1024 * 1024


def run(docker, *args, timeout=WAIT):
    result = subprocess.run(
        [*docker, *args], capture_output=True, text=True, check=False, timeout=timeout
    )
    if result.returncode:
        raise AssertionError(f"docker {args}: {result.stdout}\n{result.stderr}")
    return result.stdout


def interactive(docker, args):
    """Give docker a real terminal and send input after the guest attaches."""
    master, slave = pty.openpty()
    process = None
    try:
        try:
            process = subprocess.Popen([*docker, *args], stdin=slave, stdout=slave, stderr=slave)
        finally:
            os.close(slave)
        deadline = time.monotonic() + WAIT
        data = b""
        sent = False
        while time.monotonic() < deadline:
            readable, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
            if not readable:
                break
            try:
                chunk = os.read(master, 4096)
            except OSError as error:
                if error.errno != errno.EIO:  # PTY slave closed
                    raise
                break
            if not chunk:
                break
            data += chunk
            if not sent and b"marsa-input-ready" in data:
                os.write(master, b"marsa-stdin\n")
                sent = True
        if process.wait(timeout=max(1, deadline - time.monotonic())) or b"received:marsa-stdin" not in data:
            raise AssertionError(f"docker {args}: {data!r}")
    finally:
        os.close(master)
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()


def follow_logs(docker):
    with subprocess.Popen([*docker, "logs", "-f", "worker"], stdout=subprocess.PIPE) as logs:
        try:
            ready, _, _ = select.select([logs.stdout], [], [], WAIT)
            assert ready and logs.stdout.readline() == b"marsa-log-marker\n"
            assert logs.poll() is None, "docker logs -f returned before cancellation"
        finally:
            if logs.poll() is None:
                logs.terminate()
            logs.wait(timeout=WAIT)


def verify_disconnects(socket_path):
    # The guest bridge admits 128 live connections. Each following request has a quiet
    # upstream after its first log line. Exceed that limit with sequential disconnects.
    request = (
        b"GET /v1.57/containers/worker/logs?follow=1&stdout=1 HTTP/1.1\r\n"
        b"Host: docker\r\n\r\n"
    )
    for _ in range(140):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(5)
            client.connect(socket_path)
            client.sendall(request)
            response = b""
            while len(response) < 8192 and b"marsa-log-marker" not in response:
                data = client.recv(1024)
                if not data:
                    break
                response += data
            assert response.startswith(b"HTTP/1.1 200 ") and b"marsa-log-marker" in response
    print("140 canceled follow requests left the guest bridge available")


def resident_bytes(pid):
    return MIB // 1024 * int(
        subprocess.check_output(["/bin/ps", "-o", "rss=", "-p", pid], text=True).strip()
    )


def measure_save(docker, pid):
    # The guest generates incompressible data, so the actual Docker archive exceeds 2 GiB.
    run(docker, "exec", "worker", "/bin/busybox", "dd", "if=/dev/urandom", "of=/payload",
        "bs=1M", "count=2112", timeout=STREAM_WAIT)
    run(docker, "commit", "worker", "marsa-proxy-large", timeout=STREAM_WAIT)

    total = 0
    baseline = resident_bytes(pid)
    samples = [baseline]
    with subprocess.Popen([*docker, "save", "marsa-proxy-large"], stdout=subprocess.PIPE) as save:
        try:
            deadline = time.monotonic() + STREAM_WAIT
            next_sample = time.monotonic()
            while time.monotonic() < deadline:
                readable, _, _ = select.select([save.stdout], [], [], 0.1)
                if readable:
                    chunk = os.read(save.stdout.fileno(), 512 * 1024)
                    if not chunk:
                        break
                    total += len(chunk)
                if time.monotonic() >= next_sample:
                    samples.append(resident_bytes(pid))
                    next_sample = time.monotonic() + 0.05
            assert save.wait(timeout=max(1, deadline - time.monotonic())) == 0
        finally:
            if save.poll() is None:
                save.kill()
                save.wait()

    growth = max(samples) - baseline
    print(f"docker save delivered {total} bytes. marsad RSS grew {growth} bytes across {len(samples)} samples.")
    assert total > 2 * 1024 * MIB, total
    assert len(samples) >= 5, samples
    assert growth < 8 * MIB, samples


def main(scratch, socket, rootfs, daemon_pid):
    # Isolate CLI config, credentials and context from the host developer's Docker setup.
    for key in list(os.environ):
        if key.startswith("DOCKER_") or key.startswith("BUILDX_"):
            del os.environ[key]
    cli = shutil.which("docker")
    buildx = shutil.which("docker-buildx")
    if not cli or not buildx:
        raise AssertionError("Put the Docker CLI and docker-buildx on PATH")
    config = Path(scratch) / "docker-config"
    plugins = config / "cli-plugins"
    plugins.mkdir(parents=True)
    (plugins / "docker-buildx").symlink_to(buildx)
    docker = [cli, "--config", str(config), "--host", f"unix://{socket}"]
    os.environ["DOCKER_BUILDKIT"] = "1"

    version = run(docker, "version", "--format", "{{.Server.Version}} (API {{.Server.APIVersion}})")
    print(f"docker version: {version.strip()}")
    assert version.startswith("29.10.0 (API 1.57)"), version

    context = Path(scratch) / "build"
    context.mkdir()
    with tarfile.open(rootfs) as root:
        for path in ("./bin/busybox", "./lib/ld-musl-aarch64.so.1"):
            with (context / Path(path).name).open("wb") as dst:
                shutil.copyfileobj(root.extractfile(path), dst)
    (context / "Dockerfile").write_text(
        "FROM scratch\n"
        "COPY --chmod=755 busybox /bin/busybox\n"
        "COPY --chmod=755 ld-musl-aarch64.so.1 /lib/ld-musl-aarch64.so.1\n"
    )
    run(docker, "build", "-t", "marsa-proxy-test", str(context), timeout=120)
    print("docker build completed through BuildKit")

    def test_shell(script):
        return ["marsa-proxy-test", "/bin/busybox", "sh", "-c", script]

    script = 'echo marsa-input-ready; read line; printf "received:%s\\n" "$line"'
    interactive(docker, ["run", "--rm", "-it", *test_shell(script)])
    run(docker, "run", "-d", "--name", "worker",
        *test_shell("echo marsa-log-marker; exec /bin/busybox sleep 900"))
    interactive(docker, ["exec", "-it", "worker", "/bin/busybox", "sh", "-c", script])
    follow_logs(docker)

    # The detached container can start before the client attaches. Repeat the prompt until input.
    run(docker, "run", "-dit", "--name", "attach", *test_shell(
        '(while true; do echo marsa-input-ready; /bin/busybox sleep 0.1; done) & '
        'writer=$!; read line; kill "$writer"; printf "received:%s\\n" "$line"'
    ))
    interactive(docker, ["attach", "attach"])
    print("docker run -it, exec -it, logs -f and attach completed")
    verify_disconnects(socket)

    measure_save(docker, daemon_pid)


if __name__ == "__main__":
    main(*sys.argv[1:])
