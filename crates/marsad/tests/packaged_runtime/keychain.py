"""Qualify the pinned CLI and helper with one disposable Keychain login."""

import http.server
import json
import os
from pathlib import Path
import secrets
import socketserver
import subprocess
import sys
import tempfile
import threading


class LocalEngine(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_HEAD(self):
        self.ping()

    def do_GET(self):
        self.ping()

    def ping(self):
        if self.path != "/_ping":
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("API-Version", "1.57")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        if not self.path.endswith("/auth") or not 0 < length <= 4096:
            self.send_error(400)
            return
        data = json.loads(self.rfile.read(length))
        if data.get("serveraddress") != self.server.registry:
            self.send_error(400)
            return
        body = b'{"Status":"Login Succeeded","IdentityToken":""}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def run(tools, config):
    registry = f"marsa-keychain-{secrets.token_hex(12)}.invalid"
    username = "marsa-keychain-fixture"
    credential = secrets.token_hex(32)
    helper = tools / "docker-credential-osxkeychain"
    env = {"HOME": str(Path.home()), "PATH": f"{tools}:/usr/bin:/bin", "LC_ALL": "C"}
    with tempfile.TemporaryDirectory(prefix="marsa-auth-", dir="/tmp") as scratch:
        socket = str(Path(scratch) / "engine.sock")
        with socketserver.ThreadingUnixStreamServer(socket, LocalEngine) as engine:
            engine.registry = registry
            worker = threading.Thread(target=engine.serve_forever, daemon=True)
            worker.start()

            def command(program, *args, input_data=None):
                return subprocess.run(
                    [program, *args],
                    input=input_data,
                    env=env,
                    capture_output=True,
                    timeout=15,
                    check=False,
                )

            fixture_owned = False
            try:
                lookup = command(helper, "get", input_data=registry.encode())
                if lookup.returncode == 0:
                    raise RuntimeError("disposable Keychain item already exists")
                fixture_owned = True
                result = command(
                    tools / "docker",
                    "--config", str(config),
                    "--host", f"unix://{socket}",
                    "login", "--username", username, "--password-stdin", registry,
                    input_data=(credential + "\n").encode(),
                )
                if result.returncode or b"Login Succeeded" not in result.stdout:
                    raise RuntimeError("pinned Docker CLI could not store the fixture credential")
                stored = command(helper, "get", input_data=registry.encode())
                if stored.returncode:
                    raise RuntimeError("helper could not look up the disposable item")
                record = json.loads(stored.stdout)
                if record.get("Username") != username or record.get("Secret") != credential:
                    raise RuntimeError("the Keychain fixture did not store the expected entry")
                raw = (config / "config.json").read_bytes()
                if credential.encode() in raw:
                    raise RuntimeError("plain credential reached the Docker config")
                auths = json.loads(raw).get("auths", {})
                if any(auths.get(registry, {}).values()):
                    raise RuntimeError("a credential field reached the Docker config")
            finally:
                engine.shutdown()
                worker.join(timeout=5)
                if fixture_owned and command(helper, "get", input_data=registry.encode()).returncode == 0:
                    erased = command(helper, "erase", input_data=registry.encode())
                    if erased.returncode or command(helper, "get", input_data=registry.encode()).returncode == 0:
                        raise RuntimeError("could not ensure disposable Keychain item cleanup")
    print("Docker login stored the synthetic credential in macOS Keychain and cleanup verified.")


if __name__ == "__main__":
    if len(sys.argv) != 3 or os.geteuid() == 0:
        sys.exit("Usage as normal user: keychain.py VERIFIED_UPSTREAM_BIN PRIVATE_DOCKER_CONFIG")
    run(*(Path(argument).resolve() for argument in sys.argv[1:]))
