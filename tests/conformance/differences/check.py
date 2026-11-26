"""Exercise published ports, bind policy and actual amd64 and arm64 execution."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from urllib.request import ProxyHandler, build_opener

HERE = Path(__file__).resolve().parent
ALPINE = (HERE / "Dockerfile").read_text().splitlines()[0].split()[1]
# The amd64 child manifest of the same pinned Alpine index.
AMD64_BASE = "alpine@sha256:9db2934886712d877c932e07249c75a59e6ca27adfbccc05261987a17bef2d63"
BUSYBOX = "busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec"
BUILDKIT = "moby/buildkit:v0.35.0@sha256:aa632b921ef07411d2cb77292cd34213d4f7b384001d6fceef1e1f25bf9752bb"
ARCH = {"amd64": "x86_64", "arm64": "aarch64"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Fixture:
    def __init__(self, endpoint):
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(("DOCKER_", "BUILDX_"))}
        self.env.update(DOCKER_CONFIG=os.environ["DOCKER_CONFIG"], DOCKER_HOST=endpoint)
        self.cli = os.environ.get("MARSA_TEST_DOCKER", "docker")
        if os.path.isabs(self.cli):
            self.env["PATH"] = os.path.dirname(self.cli) + os.pathsep + self.env["PATH"]
        self.id = "marsa-arch-" + uuid.uuid4().hex[:12]
        self.tags = []

    def docker(self, *args, check=True, endpoint=None):
        env = self.env if endpoint is None else {**self.env, "DOCKER_HOST": endpoint}
        result = subprocess.run([self.cli, *args], env=env, capture_output=True, text=True, timeout=300)
        if check and result.returncode:
            raise RuntimeError(f"docker {' '.join(map(str, args))}\n{result.stdout}\n{result.stderr}")
        return result

    def build(self, tag, platforms, builder):
        self.tags.append(tag)
        self.docker("buildx", "build", "--builder", builder, "--progress=plain", "--platform", platforms,
                    "--provenance=false", "--load", "--build-arg", "PROOF=" + self.id, "-t", tag, HERE)

    def execute(self, tag, arch, implicit=False, endpoint=None):
        platform = [] if implicit else ["--platform", "linux/" + arch]
        output = self.docker("run", "--rm", "--pull=never", *platform, tag, endpoint=endpoint).stdout
        machine = ARCH[arch]
        require(output.splitlines() == [f"{machine} {self.id}", machine], f"{tag}: {output!r}")


def architecture(f):
    for arch, machine in ARCH.items():
        platform = "linux/" + arch
        f.docker("pull", "--platform", platform, ALPINE)
        result = f.docker("run", "--rm", "--pull=never", "--platform", platform, ALPINE, "apk", "--print-arch")
        require(result.stdout.strip() == machine, f"{platform}: {result.stdout}")
        print("PASS pull/run", platform, flush=True)

    foreign = f.id + ":amd64"
    f.build(foreign, "linux/amd64", "default")
    f.execute(foreign, "amd64", implicit=True)
    print("PASS ordinary run of amd64-only image", flush=True)

    ordinary = f.id + ":plain"
    f.tags.append(ordinary)
    with tempfile.TemporaryDirectory() as context:
        dockerfile = (HERE / "Dockerfile").read_text().replace(ALPINE, AMD64_BASE)
        (Path(context) / "Dockerfile").write_text(dockerfile)
        f.docker("build", "--build-arg", "PROOF=" + f.id, "-t", ordinary, context)
    f.execute(ordinary, "amd64", implicit=True)
    print("PASS plain docker build and run with amd64-only base", flush=True)

    multi = f.id + ":multi"
    f.build(multi, "linux/arm64,linux/amd64", "default")
    for arch in ARCH:
        f.execute(multi, arch)
    f.execute(multi, "arm64", implicit=True)
    print("PASS local multi-platform BuildKit RUN and image selection", flush=True)

    # An independent BuildKit daemon exercises the upstream remote driver over Docker exec.
    # It has no published port and only runs in the disposable test guest.
    remote = f.id + "-builder"
    try:
        f.docker("run", "-d", "--privileged", "--name", remote, BUILDKIT)
        f.docker("buildx", "create", "--name", remote, "--driver", "remote", "docker-container://" + remote)
        info = f.docker("buildx", "inspect", "--bootstrap", remote).stdout
        require(any(line.split() == ["Driver:", "remote"] for line in info.splitlines()), info)
        tag = f.id + ":remote"
        f.build(tag, "linux/arm64,linux/amd64", remote)
        for arch in ARCH:
            f.execute(tag, arch)
        print("PASS remote multi-platform BuildKit RUN", flush=True)
    finally:
        f.docker("buildx", "rm", remote, check=False)
        f.docker("rm", "-fv", remote, check=False)

    destination = os.getenv("MARSA_TEST_X64_EXPORT")
    if destination:
        directory = Path(destination)
        directory.mkdir(parents=True, exist_ok=True)
        f.docker("save", "--platform", "linux/amd64", "-o", directory / "amd64.tar", multi)
        (directory / "build-id").write_text(f.id + "\n")

    native = os.getenv("MARSA_TEST_X64_DOCKER_HOST")
    if native:
        machine = f.docker("info", "--format", "{{.Architecture}}", endpoint=native).stdout.strip()
        require(machine in ("x86_64", "amd64"), f"comparison engine is {machine}")
        with tempfile.TemporaryDirectory() as work:
            archive = Path(work) / "multi.tar"
            f.docker("save", "--platform", "linux/amd64", "-o", archive, multi)
            try:
                f.docker("load", "-i", archive, endpoint=native)
                f.execute(multi, "amd64", implicit=True, endpoint=native)
            finally:
                f.docker("image", "rm", multi, endpoint=native, check=False)
        print("PASS native x64 execution of Marsa-built image", flush=True)


def exposure(f):
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory).resolve()
        result = f.docker("run", "--rm", "--platform", "linux/arm64", "--mount",
                          f"type=bind,source={source},target=/unshared", ALPINE, "true", check=False)
        require(result.returncode and "Review the share request in Marsa" in result.stderr, result.stderr)
    print("PASS unshared bind refused with sharing instructions", flush=True)

    opener = build_opener(ProxyHandler({}))
    try:
        f.docker("run", "-d", "--platform", "linux/arm64", "--name", f.id, "-p", "0.0.0.0::8080", BUSYBOX,
                 "sh", "-ec", 'echo "$0" >/tmp/proof; exec busybox httpd -f -h /tmp -p 8080', f.id)
        port = int(f.docker("port", f.id, "8080/tcp").stdout.strip().split(":")[-1])
        deadline = time.monotonic() + 20
        while True:
            try:
                with opener.open(f"http://127.0.0.1:{port}/proof", timeout=2) as response:
                    require(response.read().decode().strip() == f.id, "unexpected port response")
                break
            except OSError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.25)
        # The route lookup selects the Mac's active IPv4 address without sending any packets.
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as route:
            route.connect(("192.0.2.1", 9))
            address = route.getsockname()[0]
        require(not address.startswith("127."), f"LAN address unavailable: {address}")
        try:
            with socket.create_connection((address, port), timeout=2):
                raise AssertionError(f"default published port accepted LAN traffic on {address}:{port}")
        except (ConnectionRefusedError, TimeoutError):
            pass
    finally:
        f.docker("rm", "-f", f.id, check=False)
    print("PASS wildcard publication confined to host loopback", flush=True)


if __name__ == "__main__":
    path = Path(sys.argv[1])
    require(path.is_absolute() and path.is_socket(), "provide the disposable Marsa engine socket")
    fixture = Fixture("unix://" + str(path))
    try:
        exposure(fixture)
        architecture(fixture)
    finally:
        for image in reversed(fixture.tags):
            fixture.docker("image", "rm", image, check=False)
