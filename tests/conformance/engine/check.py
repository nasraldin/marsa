"""Drive the unmodified Docker CLI and plugins against a disposable Marsa Unix socket."""

import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile
import termios
import time
import uuid


HERE = Path(__file__).resolve().parent
ROOTFS = Path(os.environ.get("MARSA_VZ_GUEST_DIR", "target/guest")) / "image-rootfs.tar"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Engine:
    def __init__(self, socket, scratch):
        require(socket.is_absolute() and socket.is_socket(), f"Expected absolute Unix socket: {socket}")
        config = Path(os.environ["DOCKER_CONFIG"])
        require(config.is_absolute() and config.is_dir(), "Set DOCKER_CONFIG to the isolated plugin configuration")
        self.args = [os.environ.get("MARSA_TEST_DOCKER", "docker"), "--config", str(config), "--host", f"unix://{socket}"]
        self.name = "marsa-g4-" + uuid.uuid4().hex[:12]
        self.image = self.name + ":base"
        self.scratch = scratch
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("DOCKER_", "BUILDX_", "COMPOSE_"))}
        self.env.update(DOCKER_CONFIG=str(config), DOCKER_HOST=f"unix://{socket}", MARSA_CONFORMANCE_IMAGE=self.image)

    def run(self, *args, check=True, timeout=90):
        result = subprocess.run([*self.args, *args], env=self.env, text=True, capture_output=True, timeout=timeout)
        if check and result.returncode:
            raise RuntimeError(f"docker {' '.join(map(str, args))}: {result.stdout}\n{result.stderr}")
        return result.stdout.strip()

    def base(self):
        context = self.scratch / "base"
        context.mkdir()
        with tarfile.open(ROOTFS) as archive:
            for path in ("./bin/busybox", "./lib/ld-musl-aarch64.so.1"):
                with (context / Path(path).name).open("wb") as out:
                    shutil.copyfileobj(archive.extractfile(path), out)
        (context / "Dockerfile").write_text(
            "FROM scratch\n"
            "COPY --chmod=755 busybox /bin/busybox\n"
            "COPY --chmod=755 ld-musl-aarch64.so.1 /lib/ld-musl-aarch64.so.1\n"
            'RUN ["/bin/busybox", "--install", "-s", "/bin"]\n'
            'RUN ["mkdir", "-p", "/tmp"]\n'
        )
        self.run("build", "-t", self.image, str(context))


def test_core(d):
    source = d.scratch / "source"
    copied = d.scratch / "copied"
    source.write_bytes(os.urandom(4096))
    container = d.name + "-core"
    since = str(int(time.time()) - 2)
    try:
        ident = d.run("run", "-d", "--name", container, d.image, "sleep", "300")
        d.run("cp", str(source), container + ":/tmp/payload")
        d.run("cp", container + ":/tmp/payload", str(copied))
        require(source.read_bytes() == copied.read_bytes(), "docker cp changed payload")
        data = json.loads(d.run("inspect", container))[0]
        require(data["Id"] == ident and data["State"]["Running"], "inspect returned incorrect state")
        stats = json.loads(d.run("stats", "--no-stream", "--format", "{{json .}}", container))
        require(stats["Name"] == container and "%" in stats["CPUPerc"], f"unexpected Docker stats: {stats}")
        events = d.run("events", "--since", since, "--until", str(int(time.time()) + 1), "--filter", f"container={ident}",
                       "--format", "{{.Action}}").splitlines()
        require("create" in events and "start" in events, f"missing events: {events}")
    finally:
        d.run("rm", "-f", container, check=False)


def test_build(d):
    context = d.scratch / "build"
    context.mkdir()
    secret = d.scratch / "secret"
    secret.write_text("fixture-secret")
    (context / "Dockerfile").write_text(
        f"FROM {d.image}\n"
        "ARG STAGE\n"
        f"RUN --mount=type=cache,id={d.name},sharing=locked,target=/cache "
        "--mount=type=secret,id=proof,required=true "
        'set -e; test "$(cat /run/secrets/proof)" = fixture-secret; '
        'if [ "$STAGE" = seed ]; then echo cache-proof >/cache/token; test -s /cache/token; '
        'else cp /cache/token /proof; fi\n'
    )
    built = d.name + ":cached"
    try:
        for stage in ("seed", "verify"):
            d.run("buildx", "build", "--builder", "default", "--load", "--build-arg",
                  f"STAGE={stage}", "--secret", f"id=proof,src={secret}", "-t", built, str(context))
        proof = d.run("run", "--rm", built, "sh", "-ec", "test ! -e /run/secrets/proof; cat /proof")
        require(proof == "cache-proof", "BuildKit mutable cache did not survive separate builds")
    finally:
        d.run("image", "rm", built, check=False)


def test_compose(d):
    since = str(int(time.time()) - 2)
    compose = ["compose", "-p", d.name, "-f", str(HERE / "compose.yaml")]
    try:
        d.run(*compose, "up", "-d", "--wait", "--wait-timeout", "45")
        active = set(d.run(*compose, "ps", "--services", "--status", "running").splitlines())
        require(active == {"client", "upstream"}, f"default Compose services: {active}")
        require("client-reached-upstream" in d.run(*compose, "logs", "client"), "missing dependent service log")
        ident = d.run(*compose, "ps", "-q", "upstream")
        require(d.run("inspect", "-f", "{{.State.Health.Status}}", ident) == "healthy", "upstream unhealthy")
        client = d.run(*compose, "ps", "-q", "client")
        events = d.run("events", "--since", since, "--until", str(int(time.time()) + 1),
                       "--filter", f"label=com.docker.compose.project={d.name}", "--filter", "type=container",
                       "--format", "{{json .}}")
        sequence = [(event["Actor"]["ID"], event["Action"]) for event in map(json.loads, events.splitlines())]
        require(sequence.index((ident, "health_status: healthy")) < sequence.index((client, "start")),
                "Compose started the client before its dependency became healthy")
        d.run(*compose, "--profile", "extra", "up", "-d", "--wait", "--wait-timeout", "45")
        active = set(d.run(*compose, "--profile", "extra", "ps", "--services", "--status", "running").splitlines())
        require(active == {"client", "upstream", "optional"}, f"profile service absent: {active}")
    finally:
        d.run(*compose, "--profile", "extra", "down", "--timeout", "1")
    require(not d.run("ps", "-aq", "--filter", f"label=com.docker.compose.project={d.name}"), "Compose left containers")
    require(not d.run("network", "ls", "-q", "--filter", f"label=com.docker.compose.project={d.name}"), "Compose left network")


def test_storage(d):
    volume = d.run("volume", "create", d.name)
    mount = ["--mount", f"type=volume,source={volume},target=/data"]
    try:
        d.run("run", "--rm", *mount, d.image, "sh", "-ec", "echo volume-proof >/data/token")
        output = d.run("run", "--rm", *mount, "--tmpfs", "/ram", d.image, "sh", "-ec",
                       "grep ' /ram tmpfs ' /proc/mounts >/dev/null; echo ram-proof >/ram/token; cat /ram/token /data/token")
        require(output.splitlines() == ["ram-proof", "volume-proof"], f"storage data mismatch: {output}")
        d.run("run", "--rm", "--tmpfs", "/ram", d.image, "test", "!", "-e", "/ram/token")
    finally:
        d.run("volume", "rm", volume)


def test_streams(d):
    master, slave = pty.openpty()
    container = d.name + "-tty"
    def resize(rows, cols):
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def receive(marker):
        output = bytearray()
        deadline = time.monotonic() + 25
        while marker not in output:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([master], [], [], remaining)[0]:
                raise AssertionError(f"PTY did not emit {marker!r}: {bytes(output)!r}")
            output.extend(os.read(master, 8192))

    process = None
    try:
        resize(24, 80)
        script = ('trap \'echo received-interrupt; exit 23\' INT; echo tty-ready; '
                  'while read -r input; do stty size; done')
        process = subprocess.Popen([*d.args, "run", "-it", "--name", container, d.image, "sh", "-c", script],
                                   stdin=slave, stdout=slave, stderr=slave, env=d.env)
        receive(b"tty-ready")
        resize(41, 107)
        process.send_signal(signal.SIGWINCH)
        # Reissue the command until Docker's asynchronous resize reaches the guest PTY.
        for _ in range(12):
            os.write(master, b"size\n")
            time.sleep(0.2)
            if select.select([master], [], [], 1)[0] and b"41 107" in os.read(master, 8192):
                break
        else:
            raise AssertionError("guest PTY did not adopt 41 rows and 107 columns")
        os.write(master, b"\x03")
        receive(b"received-interrupt")
        require(process.wait(timeout=15) == 23, "container signal handler did not set the expected exit code")
    finally:
        d.run("rm", "-f", container, check=False)
        if process and process.poll() is None:
            process.kill()
            process.wait()
        os.close(slave)
        os.close(master)


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: check.py /absolute/path/to/isolated/docker.sock")
    with tempfile.TemporaryDirectory(prefix="marsa-engine-") as scratch:
        d = Engine(Path(sys.argv[1]), Path(scratch))
        try:
            d.base()
            for check in (test_core, test_build, test_compose, test_storage, test_streams):
                check(d)
                print(f"PASS engine/{check.__name__.removeprefix('test_')}", flush=True)
        finally:
            d.run("image", "rm", d.image, check=False)


if __name__ == "__main__":
    main()
