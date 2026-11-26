import os

from testcontainers.core.container import DockerContainer

with DockerContainer(os.environ["MARSA_SAMPLE_IMAGE"]).with_command("sleep 180") as container:
    code, output = container.exec(["echo", "marsa-python"])
    if code != 0 or output.strip() != b"marsa-python":
        raise RuntimeError(f"Unexpected exec result: {code} {output!r}")
