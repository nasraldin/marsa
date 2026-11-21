"""Run the M1-04 checks against a fresh scratch-only virtiofs share."""

import argparse
import os
import pathlib
import random
import shutil
import stat
import subprocess
import sys
import time
from contextlib import contextmanager


HERE = pathlib.Path(__file__).resolve().parent
TARGET = HERE / "target"
HOST = TARGET / "release/marsa-vz-spike"
NATIVE = TARGET / "release/marsa-virtiofs-spike"
BINARIES = TARGET / "aarch64-unknown-linux-musl/release"
GUEST = BINARIES / "marsa-vsock-spike-guest"
PROBE = BINARIES / "marsa-virtiofs-spike"


def run(*arguments):
    result = subprocess.run(arguments, check=False, text=True, capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(f"{arguments[0]} exited {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def prepare(root, iso):
    root.mkdir(mode=0o700)
    args = [sys.executable, HERE / "fixtures.py", GUEST, root / "fixtures"]
    if iso is not None:
        args += ["--iso", iso]
    run(*args)
    return root / "fixtures"


def launch(root, fixtures, script, writable=False):
    console = root / "console.log"
    arguments = [HOST]
    if writable:
        arguments.append("--writable-share")
    arguments += ["rust", fixtures, console, script]
    return subprocess.Popen(arguments, cwd=HERE, text=True), console


def stop(process):
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=10)


def wait(process):
    try:
        if process.wait(timeout=100) != 0:
            raise RuntimeError(f"VM runner failed with exit status {process.returncode}")
    finally:
        stop(process)


def observations(console):
    for line in console.read_text(errors="replace").splitlines():
        if line.startswith("VF_"):
            print(line)


def stamp(path):
    attributes = path.lstat()
    print(
        f"VF_HOST stat={path.name} uid={attributes.st_uid} gid={attributes.st_gid} "
        f"mode={stat.S_IMODE(attributes.st_mode):04o} inode={attributes.st_ino} "
        f"nlink={attributes.st_nlink}"
    )


def correctness(root, iso):
    fixtures = prepare(root, iso)
    share = fixtures / "share"
    share.chmod(0o755)
    shutil.copy2(PROBE, share / "fsprobe")
    directory = share / "observations"
    directory.mkdir(mode=0o777)
    directory.chmod(0o777)

    host_file = directory / "host-file"
    host_file.write_bytes(b"host-file\n")
    host_file.chmod(0o640)
    os.link(host_file, directory / "host-hard")
    (directory / "host-relative").symlink_to("host-file")
    (directory / "host-dangling").symlink_to("absent")
    (directory / "MixedCase").touch()
    (directory / "host-edit").write_bytes(b"initial\n")
    (directory / "host-xattr").touch()
    run("/usr/bin/xattr", "-w", "user.marsa.spike", "host-xattr", directory / "host-xattr")
    print(f"VF_CASE host_matches_different_case={(directory / 'mixedcase').is_file()}")
    stamp(host_file)

    vm, console = launch(root, fixtures, HERE / "virtiofs-correctness.sh", writable=True)
    try:
        deadline = time.monotonic() + 25
        while not (directory / "watch-ready").exists():
            if vm.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError(f"guest watcher did not become ready; see {console}")
            time.sleep(0.01)
        with (directory / "host-edit").open("ab") as written:
            written.write(b"host-edit\n")
            written.flush()
            os.fsync(written.fileno())
        temporary = directory / "host-temporary"
        temporary.write_bytes(b"host-rename\n")
        temporary.rename(directory / "host-renamed")
        wait(vm)
    finally:
        stop(vm)

    observations(console)
    for filename in ["guest-root", "guest-unprivileged", "guest-relative", "guest-dangling", "guest-hard"]:
        candidate = directory / filename
        if not candidate.exists() and not candidate.is_symlink():
            print(f"VF_HOST missing={filename}")
        else:
            stamp(candidate)
    assert (directory / "guest-relative").readlink() == pathlib.Path("guest-root")
    assert (directory / "guest-relative").read_bytes() == b"guest-root\n"
    assert (directory / "guest-dangling").readlink() == pathlib.Path("absent")
    assert not (directory / "guest-dangling").exists()
    xattr = subprocess.run(
        ["/usr/bin/xattr", "-p", "user.marsa.spike", directory / "guest-root"],
        check=False,
        text=True,
        capture_output=True,
    )
    print(f"VF_XATTR guest_to_host_matches={xattr.stdout.strip() == 'guest-xattr'}")


@contextmanager
def mounted(image, point, read_only=False):
    point.mkdir(parents=False, exist_ok=True)
    arguments = ["diskutil", "image", "attach", "--nobrowse", "--mountPoint", point]
    if read_only:
        arguments.append("--readOnly")
    run(*arguments, image)
    try:
        yield
    finally:
        run("diskutil", "eject", point)


def datasets(root):
    # Both filesystem instances receive equal workload shapes and deterministic content.
    (root / ".metadata_never_index").touch()
    (root / "fixture").write_bytes(b"marsa-vz-fixture\n")
    small, walk = root / "small", root / "walk"
    small.mkdir()
    walk.mkdir()
    source = random.Random(20261121)
    for number in range(2048):
        (small / f"{number:04}.dat").write_bytes(source.randbytes(4096))
    for number in range(128):
        directory = walk / f"{number:03}"
        directory.mkdir()
        for index in range(128):
            (directory / f"{index:03}.dat").touch()
    with (root / "large.dat").open("wb") as data:
        for _ in range(128):
            data.write(source.randbytes(1024 * 1024))


def benchmark(root, iso, first):
    fixtures = prepare(root, iso)
    native = root / "native"
    share = fixtures / "share"
    images = []
    for name, mount in [("Native", native), ("Guest", share)]:
        image = root / f"{name.lower()}.img"
        run("diskutil", "image", "create", "blank", "--size", "512m", "--fs", "APFS",
            "--volumeName", f"Marsa{name}Scratch", image)
        with mounted(image, mount):
            datasets(mount)
            if name == "Guest":
                shutil.copy2(PROBE, mount / "fsprobe")
        images.append(image)

    def native_run():
        with mounted(images[0], native, read_only=True):
            print("VF_ROUTE native-apfs")
            print(run(NATIVE, "bench", native), end="")

    def guest_run():
        script = root / "benchmark.sh"
        script.write_text(
            "busybox mount -t virtiofs -o ro marsa-spike /mnt/marsa\n"
            "/mnt/marsa/fsprobe bench /mnt/marsa\n"
            "busybox umount /mnt/marsa\n"
        )
        with mounted(images[1], share, read_only=True):
            print("VF_ROUTE guest-virtiofs")
            vm, console = launch(root, fixtures, script)
            wait(vm)
            observations(console)

    if first == "guest":
        guest_run()
        native_run()
    else:
        native_run()
        guest_run()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("correctness", "benchmark"))
    parser.add_argument("root", type=pathlib.Path, help="a new scratch directory")
    parser.add_argument("--iso", type=pathlib.Path, help="use an already downloaded verified Alpine ISO")
    parser.add_argument("--first", choices=("native", "guest"), default="native")
    args = parser.parse_args()
    if args.mode == "correctness":
        correctness(args.root.resolve(), args.iso)
    else:
        benchmark(args.root.resolve(), args.iso, args.first)


if __name__ == "__main__":
    main()
