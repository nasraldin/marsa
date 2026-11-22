"""Validate a Buildx SSH mount against a private fixture on the VM's loopback."""

from contextlib import contextmanager
import base64
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import time
import tomllib
from urllib.request import urlopen


WAIT = 30
IMAGE = "marsa-ssh-fixture:tools"
CLONE_IMAGE = "marsa-ssh-fixture:cloned"
SERVER = "marsa-ssh-fixture-server"
PORT = 2222

SSH_SERVER = r"""
adduser -D fixture
passwd -d fixture
install -d -m 700 -o fixture -g fixture /home/fixture/.ssh
printf '%s\n' "$FIXTURE_PUBLIC_KEY" > /home/fixture/.ssh/authorized_keys
chown fixture:fixture /home/fixture/.ssh/authorized_keys
chmod 600 /home/fixture/.ssh/authorized_keys
git init -q --initial-branch=main /srv/private
printf '%s\n' cloned-from-private-fixture > /srv/private/marker
git -C /srv/private add marker
git -C /srv/private -c user.name=fixture -c user.email=fixture@example.invalid commit -qm fixture
chown -R fixture:fixture /srv/private
install -d -m 700 /run/sshd
ssh-keygen -q -t ed25519 -N '' -f /run/sshd/host_key
cat > /run/sshd/fixture.conf <<'CONFIG'
Port 2222
ListenAddress 127.0.0.1
HostKey /run/sshd/host_key
AllowUsers fixture
AuthenticationMethods publickey
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
PermitRootLogin no
DisableForwarding yes
PermitTTY no
ForceCommand /usr/bin/git-upload-pack /srv/private
LogLevel VERBOSE
CONFIG
exec /usr/sbin/sshd -D -e -f /run/sshd/fixture.conf
"""


def command(args, env, *, seconds=WAIT, ok=True):
    result = subprocess.run(args, env=env, capture_output=True, text=True, timeout=seconds)
    if ok and result.returncode:
        raise AssertionError(f"{Path(args[0]).name} exited {result.returncode}\n{result.stdout}\n{result.stderr}")
    return result


def fetch(url, checksum, output):
    with urlopen(url, timeout=WAIT) as response:
        payload = response.read(8 * 1024 * 1024)
    assert hashlib.sha256(payload).hexdigest() == checksum, f"checksum mismatch for {output.name}"
    output.write_bytes(payload)


def tools_image(docker, env, scratch):
    base = scratch / "tools"
    packages = base / "packages"
    packages.mkdir(parents=True)
    repository = Path(__file__).resolve().parents[4]
    with (repository / "guest/versions.toml").open("rb") as manifest:
        alpine = tomllib.load(manifest)["alpine"]
    version = alpine["version"]
    branch = ".".join(version.split(".")[:2])
    mirror = f"https://dl-cdn.alpinelinux.org/alpine/v{branch}"
    fetch(f"{mirror}/releases/aarch64/alpine-minirootfs-{version}-aarch64.tar.gz",
          alpine["sha256"], base / "alpine.tar.gz")
    lock = Path(__file__).with_name("ssh-build-aarch64.sha256")
    for line in lock.read_text().splitlines():
        checksum, name = line.split()
        assert Path(name).name == name and name.endswith(".apk")
        fetch(f"{mirror}/main/aarch64/{name}", checksum, packages / name)
    (base / "Dockerfile").write_text(
        "FROM scratch\n"
        "ADD alpine.tar.gz /\n"
        "COPY packages /fixture-apks\n"
        "RUN --network=none apk --cache=no --network=no --repositories-file /dev/null "
        "--logfile=no --force-non-repository add /fixture-apks/*.apk\n"
    )
    command([*docker, "build", "-t", IMAGE, str(base)], env, seconds=150)


def wait_until_ready(check):
    deadline = time.monotonic() + WAIT
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError("fixture did not become ready")


@contextmanager
def fixture_agent(scratch, env):
    directory = scratch / "agent"
    directory.mkdir(mode=0o700)
    key = directory / "client"
    sock = directory / "agent.sock"
    command(["/usr/bin/ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)], env)
    with subprocess.Popen(
        ["/usr/bin/ssh-agent", "-D", "-a", str(sock)], env=env, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ) as agent:
        try:
            wait_until_ready(lambda: sock.exists() or agent.poll() is not None)
            assert agent.poll() is None, "disposable ssh-agent exited"
            socket_info = sock.lstat()
            assert stat.S_ISSOCK(socket_info.st_mode) and socket_info.st_uid == os.geteuid()
            scoped = {**env, "SSH_AUTH_SOCK": str(sock)}
            command(["/usr/bin/ssh-add", "-t", "300", str(key)], scoped)
            public = key.with_suffix(".pub")
            fingerprint = command(["/usr/bin/ssh-keygen", "-lf", str(public)], env).stdout.split()[1]
            yield scoped, public.read_text().strip(), fingerprint
        finally:
            if agent.poll() is None:
                agent.terminate()
                agent.wait(timeout=WAIT)


def git_fixture(docker, env, public_key, context):
    command([
        *docker, "run", "-d", "--name", SERVER, "--network", "host",
        "--env", f"FIXTURE_PUBLIC_KEY={public_key}", IMAGE, "/bin/sh", "-eu", "-c", SSH_SERVER,
    ], env)

    def listening():
        logs = command([*docker, "logs", SERVER], env)
        return f"Server listening on 127.0.0.1 port {PORT}." in logs.stderr

    try:
        wait_until_ready(listening)
    except AssertionError:
        logs = command([*docker, "logs", SERVER], env)
        raise AssertionError(f"private Git server did not listen\n{logs.stdout}\n{logs.stderr}") from None
    key = command([*docker, "exec", SERVER, "cat", "/run/sshd/host_key.pub"], env).stdout.split()
    assert key[0] == "ssh-ed25519" and len(base64.b64decode(key[1], validate=True)) == 51
    (context / "known_hosts").write_text(f"[127.0.0.1]:{PORT} {key[0]} {key[1]}\n")


def verify_build(docker, env, scratch):
    context = scratch / "build"
    context.mkdir()
    with fixture_agent(scratch, env) as (scoped, public, fingerprint):
        git_fixture(docker, env, public, context)
        (context / "Dockerfile").write_text(
            f"FROM {IMAGE}\n"
            "COPY known_hosts /fixture-known-hosts\n"
            "RUN --mount=type=ssh,required=true,target=/run/marsa-test-agent "
            "test -S \"$SSH_AUTH_SOCK\" && "
            "test \"$(stat -c '%a:%u:%g' \"$SSH_AUTH_SOCK\")\" = 600:0:0 && ! ssh-add -D && "
            "GIT_SSH_COMMAND='ssh -F /dev/null -o BatchMode=yes -o StrictHostKeyChecking=yes "
            "-o UserKnownHostsFile=/fixture-known-hosts -o GlobalKnownHostsFile=/dev/null "
            "-o IdentityFile=none' "
            f"git clone ssh://fixture@127.0.0.1:{PORT}/private /checkout && "
            "grep -qx cloned-from-private-fixture /checkout/marker\n"
        )
        build = [*docker, "build", "--allow", "network.host", "--network", "host", "--no-cache"]
        negative = command([*build, str(context)], scoped, seconds=90, ok=False)
        missing = ("unset ssh forward key default", 'no SSH key "default"')
        assert negative.returncode != 0 and any(msg in negative.stderr for msg in missing), negative.stderr
        command([*build, "--ssh", "default", "-t", CLONE_IMAGE, str(context)], scoped, seconds=90)
        server = command([*docker, "logs", SERVER], env)
        assert "Accepted publickey for fixture" in server.stderr and fingerprint in server.stderr
        assert fingerprint in command(["/usr/bin/ssh-add", "-l"], scoped).stdout
        command([*docker, "run", "--rm", "--user", "65534:65534", CLONE_IMAGE, "sh", "-ec",
                 'test -z "${SSH_AUTH_SOCK:-}" && test ! -S /run/marsa-test-agent'], scoped)
        print("Private Git clone authenticated with the disposable host agent. "
              "A build without --ssh and a regular container had no agent access.")


def main(scratch, socket):
    scratch = Path(scratch)
    cli = shutil.which("docker")
    buildx = shutil.which("docker-buildx")
    assert cli and buildx, "Put the Docker CLI and docker-buildx on PATH."
    home = scratch / "ssh-client-home"
    config = home / "docker"
    plugins = config / "cli-plugins"
    plugins.mkdir(parents=True)
    (plugins / "docker-buildx").symlink_to(buildx)
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "LC_ALL": "C",
        "DOCKER_BUILDKIT": "1",
        "APPLE_SSH_ADD_BEHAVIOR": "openssh",
    }
    docker = [cli, "--config", str(config), "--host", f"unix://{socket}"]
    tools_image(docker, env, scratch)
    verify_build(docker, env, scratch)


if __name__ == "__main__":
    main(*sys.argv[1:])
