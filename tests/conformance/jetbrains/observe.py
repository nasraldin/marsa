"""Verify that the IDE can find and stop one disposable container on the selected socket."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("socket", type=Path, help="absolute socket of the disposable Marsa daemon")
    parser.add_argument("image", help="local arm64 test image with sleep on PATH")
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    if not args.socket.is_absolute() or not args.socket.is_socket():
        parser.error("expected an absolute Unix socket path")
    config = Path(os.environ["DOCKER_CONFIG"])
    if not config.is_absolute() or not config.is_dir():
        parser.error("select the private Docker fixture configuration with DOCKER_CONFIG")
    name = f"marsa-ide-{uuid.uuid4().hex[:12]}"
    environment = {k: v for k, v in os.environ.items() if not k.startswith("DOCKER_")}
    endpoint = f"unix://{args.socket}"
    cli = [os.environ.get("MARSA_TEST_DOCKER", "docker"), "--host", endpoint, "--config", str(config)]

    def docker(*argv):
        return subprocess.check_output([*cli, *argv], env=environment, text=True, timeout=30).strip()

    since = str(int(time.time()) - 2)
    ident = docker("run", "-d", "--pull", "never", "--name", name, args.image,
                   "sleep", str(args.timeout + 120))
    try:
        print(f"Connect the JetBrains Docker plugin to {endpoint}", flush=True)
        print(f"In Services > Docker > Containers, select {name} and choose Stop.", flush=True)
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            state = json.loads(docker("inspect", "--format", "{{json .State}}", ident))
            if state["Status"] == "exited":
                if state["OOMKilled"] or state["Error"] or state["ExitCode"] not in (137, 143):
                    raise RuntimeError(f"container exited unexpectedly: {state}")
                actions = docker("events", "--since", since, "--until", str(int(time.time()) + 2),
                                 "--filter", f"container={ident}", "--format", "{{.Action}}").splitlines()
                if "stop" not in actions:
                    raise RuntimeError(f"engine did not record the IDE stop action: {actions}")
                print(f"PASS JetBrains Docker Stop observed on {name} ({ident[:12]})")
                return
            time.sleep(1)
        raise TimeoutError("the IDE did not stop the selected container")
    finally:
        docker("rm", "-f", ident)


if __name__ == "__main__":
    main()
