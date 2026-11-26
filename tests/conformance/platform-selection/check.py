"""Verify the bundled Docker command with a fresh client profile and a real Marsa engine."""
import os
from pathlib import Path
import subprocess
import tempfile
import uuid


AMD64_INDEX = "amd64/alpine:3.22"


def main():
    cli = Path(os.environ["MARSA_TEST_BUNDLED_DOCKER"]).resolve(strict=True)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("DOCKER_", "BUILDX_", "BUILDKIT_"))}
    env["DOCKER_HOST"] = os.environ["DOCKER_HOST"]
    tag = "marsa-platform-" + uuid.uuid4().hex[:12]

    def docker(*args, check=True):
        return subprocess.run([cli, *args], env=env, text=True, capture_output=True,
                              timeout=180, check=check)

    def arch(image, *options):
        return docker("run", "--rm", *options, image, "apk", "--print-arch").stdout.strip()

    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        config = root / "client"
        config.mkdir()
        env["DOCKER_CONFIG"] = str(config)
        context = root / "context"
        context.mkdir()
        (context / "Dockerfile").write_text(
            f"FROM {AMD64_INDEX}\nRUN apk --print-arch >/built-for\nCMD [\"cat\",\"/built-for\"]\n"
        )
        try:
            # Pull and run through the command without preloading the amd64 child.
            docker("pull", AMD64_INDEX)
            assert arch(AMD64_INDEX, "--pull=always") == "x86_64"
            # A fresh Mac can use the bundled command without selecting a socket.
            if not Path("/var/run/docker.sock").exists():
                endpoint = env.pop("DOCKER_HOST")
                try:
                    assert arch(AMD64_INDEX) == "x86_64"
                finally:
                    env["DOCKER_HOST"] = endpoint
            docker("build", "-t", tag, str(context))
            assert docker("run", "--rm", tag).stdout.strip() == "x86_64"
            # A scratch final stage still depends on the amd64-only builder.
            (context / "Dockerfile").write_text(
                f"FROM {AMD64_INDEX} AS builder\nRUN apk --print-arch >/built-for\n"
                "FROM scratch\nCOPY --from=builder /built-for /built-for\n"
            )
            docker("build", "-t", tag, str(context))
            assert docker("image", "inspect", "--format", "{{.Architecture}}", tag).stdout.strip() == "amd64"
            assert arch("alpine:3.22") == "aarch64"
            assert arch("alpine:3.22", "--platform", "linux/amd64") == "x86_64"
            assert docker("pull", "--platform", "linux/arm64", AMD64_INDEX, check=False).returncode
            print("PASS bundled Docker selected amd64-only index, built and ran its output, "
                  "preferred arm64 and respected explicit platforms")
        except subprocess.CalledProcessError as error:
            raise RuntimeError(f"{error.cmd} exited {error.returncode}\n{error.stderr}") from error
        finally:
            docker("image", "rm", tag, check=False)


if __name__ == "__main__":
    main()
