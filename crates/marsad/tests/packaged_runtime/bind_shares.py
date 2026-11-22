"""Drive the pinned Docker CLI against a private signed Marsa fixture."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile


def main(scratch, socket, rootfs, mode, source):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("DOCKER_", "BUILDX_"))}
    cli = shutil.which("docker")
    buildx = shutil.which("docker-buildx")
    assert cli and buildx, "put the pinned Docker CLI and Buildx on PATH"
    config = Path(scratch) / "docker-config"
    docker = [cli, "--config", str(config), "--host", f"unix://{socket}"]
    image = "marsa-bind-fixture"
    if mode == "build":
        plugins = config / "cli-plugins"
        plugins.mkdir(parents=True)
        (plugins / "docker-buildx").symlink_to(buildx)
        context = Path(scratch) / "build"
        context.mkdir()
        with tarfile.open(rootfs) as archive:
            for name in ("bin/busybox", "lib/ld-musl-aarch64.so.1"):
                with (context / Path(name).name).open("wb") as output:
                    shutil.copyfileobj(archive.extractfile(f"./{name}"), output)
        (context / "Dockerfile").write_text(
            "FROM scratch\n"
            "COPY --chmod=755 busybox /bin/busybox\n"
            "COPY --chmod=755 ld-musl-aarch64.so.1 /lib/ld-musl-aarch64.so.1\n"
        )
        subprocess.run([*docker, "build", "-t", image, str(context)], check=True,
                       env=env, timeout=60)
        return
    if mode == "guest-socket":
        subprocess.run([*docker, "run", "--rm", "-v", "/var/run/docker.sock:/dev/engine.sock",
                        image, "/bin/busybox", "test", "-S", "/dev/engine.sock"],
                       check=True, env=env, timeout=30)
        return
    assert mode in ("allow", "deny")
    result = subprocess.run(
        [*docker, "run", "--rm", "--user", "12345:12345", "-v", f"{source}:/fixture",
         image, "/bin/busybox", "sh", "-c", "echo marsa-bind-owner > /fixture/created-from-guest"],
        env=env, capture_output=True, text=True, timeout=30
    )
    if mode == "deny":
        assert result.returncode and "outside the active Marsa shares" in result.stderr, result
    else:
        assert result.returncode == 0, result


if __name__ == "__main__":
    main(*sys.argv[1:])
