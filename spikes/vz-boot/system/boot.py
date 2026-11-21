"""Boot the packaged guest on Apple silicon and report its measured boot stages."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib

HELLO_DIGEST = "sha256:5099b89d7666cc2186cad769ddc262ddc7c335b33f5fe79f9ffe50a01282b23e"
HELLO_IMAGE = f"docker.io/library/hello-world@{HELLO_DIGEST}"
BUDGETS = {"kernel_ms": 300, "init_to_hello_ms": 300, "dockerd_to_ready_ms": 1500}
INIT_STAGES = {"mount_system", "data_disk", "shared_folders", "network", "agent", "containerd", "dockerd"}
HERE = Path(__file__).resolve().parent


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def prepare(source, fixture):
    """Copy and verify the two boot files and provision a private sparse data disk."""
    with (HERE.parents[2] / "guest/versions.toml").open("rb") as file:
        pins = tomllib.load(file)
    manifest = json.loads((source / "guest-manifest.json").read_text())
    if manifest["architecture"] != "aarch64" or manifest["rootfs_format"] != "erofs":
        raise ValueError("expected an arm64 EROFS guest image")
    for component in ("marsa_init", "marsa_agent"):
        if manifest["components"][component] != manifest["image_version"]:
            raise ValueError(f"missing expected {component} release")
    for name, key in (("Image", "kernel_sha256"), ("rootfs.img", "rootfs_sha256")):
        destination = fixture / name
        shutil.copyfile(source / name, destination)
        if sha256(destination) != manifest[key]:
            raise ValueError(f"guest checksum mismatch for {name}")
    with (fixture / "data.img").open("xb") as disk:
        disk.truncate(512 * 1024 * 1024)

    share = fixture / "share"
    share.mkdir(mode=0o700)
    docker = pins["docker"]
    archive = source / f'docker-{docker["version"]}.tgz'
    if sha256(archive) != docker["sha256"]:
        raise ValueError("Docker CLI source archive checksum mismatch")
    with tarfile.open(archive) as upstream, (share / "docker").open("xb") as binary:
        with upstream.extractfile("docker/docker") as original:
            shutil.copyfileobj(original, binary)
    (share / "docker").chmod(0o755)
    subprocess.run(["docker", "pull", "--platform", "linux/arm64", HELLO_IMAGE], check=True)
    subprocess.run(
        ["docker", "save", "--platform", "linux/arm64", "--output", share / "hello-world.tar", HELLO_IMAGE],
        check=True,
    )
    (share / "hello-world.id").write_text(HELLO_DIGEST + "\n")


def check_ext4_shutdown(path):
    """Check the ext4 clean state and that replay of the journal is unnecessary."""
    with path.open("rb") as disk:
        disk.seek(1024)
        superblock = disk.read(1024)
    magic, state = struct.unpack_from("<HH", superblock, 0x38)
    incompatible, = struct.unpack_from("<I", superblock, 0x60)
    if magic != 0xEF53 or state != 1 or incompatible & 0x04:
        raise ValueError("guest data disk was not cleanly unmounted")


def report(host_log, guest_log):
    """Check the workload and convert host and guest measurements to an artifact."""
    metrics = re.findall(r"^MARSA_BOOT_METRICS (.+)$", host_log, re.MULTILINE)
    if len(metrics) != 1:
        raise ValueError("expected one boot metrics record")
    values = {name: int(value) for name, value in (item.split("=", 1) for item in metrics[0].split())}
    if values.keys() != BUDGETS.keys() | {"start_to_ready_ms"}:
        raise ValueError("incomplete host boot metrics")

    stages = re.findall(r"^MARSA_PID1_STAGE (\w+)=(\d+)$", guest_log, re.MULTILINE)
    if {stage for stage, _ in stages} != INIT_STAGES or len(stages) != len(INIT_STAGES):
        raise ValueError("incomplete PID 1 stage timings")
    if "MARSA_HELLO_WORLD_OK" not in guest_log.splitlines():
        raise ValueError("hello-world container success was not observed")

    return {
        "measurements": {
            name: {
                "duration_ms": values[name],
                "budget_ms": budget,
                "within_budget": values[name] < budget,
            }
            for name, budget in BUDGETS.items()
        },
        "start_to_ready_ms": values["start_to_ready_ms"],
        "pid1_stages_ms": {stage: int(duration) for stage, duration in stages},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("guest_output", type=Path)
    parser.add_argument("runner", type=Path)
    parser.add_argument("results", type=Path)
    args = parser.parse_args()
    args.results.mkdir(parents=True)
    with tempfile.TemporaryDirectory(prefix="marsa-boot-") as temp:
        fixture = Path(temp)
        prepare(args.guest_output, fixture)
        serial = fixture / "console.log"
        result = subprocess.run(
            [args.runner, "--boot-test", "rust", fixture, serial, HERE / "check.sh"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            check=False,
        )
        print(result.stdout, end="")
        (args.results / "runner.log").write_text(result.stdout)
        if serial.is_file():
            shutil.copyfile(serial, args.results / "console.log")
        result.check_returncode()
        check_ext4_shutdown(fixture / "data.img")
        measurements = report(result.stdout, serial.read_text())
        data = json.dumps(measurements, indent=2) + "\n"
        (args.results / "boot-timings.json").write_text(data)
        print(data, end="")


if __name__ == "__main__":
    main()
