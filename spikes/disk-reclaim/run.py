"""Measure sparse RAW reclaim with the direct Rust VZ boot harness."""

import argparse
import json
import pathlib
import re
import shutil
import subprocess
import time

from packages import prepare_packages

HERE = pathlib.Path(__file__).resolve().parent
IMAGE_BYTES = 8 * 1024**3
RELEASED_BYTES = 4 * 1024**3
CACHE_MODES = ("automatic", "cached", "uncached")
CHECKPOINTS = ("formatted", "filled", "deleted", "trimmed")


def sparse_image(path: pathlib.Path) -> None:
    with path.open("xb") as image:
        image.truncate(IMAGE_BYTES)


def allocated(path: pathlib.Path) -> int:
    return path.stat().st_blocks * 512


def fixture(path: pathlib.Path, base: pathlib.Path, packages: pathlib.Path) -> None:
    path.mkdir(mode=0o700)
    for name in ("Image", "initramfs", "modules.img"):
        shutil.copyfile(base / name, path / name)
    share = path / "share"
    share.mkdir(mode=0o700)
    shutil.copytree(packages, share / "packages")
    shutil.copyfile(HERE / "tools.sh", share / "tools.sh")
    (share / "fixture").write_text("marsa-vz-fixture\n")
    sparse_image(path / "data.img")


def transcript(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def require_match(pattern: str, text: str) -> str:
    match = re.search(pattern, text, flags=re.MULTILINE)
    if match is None:
        raise ValueError(f"expected guest record missing: {pattern}")
    return match[1]


def trim_trial(harness: pathlib.Path, path: pathlib.Path, mode: str) -> dict:
    (path / "disk-cache").write_text(f"{mode}\n")
    image = path / "data.img"
    log = path / "trim.log"
    sizes = {"blank": allocated(image)}
    argv = [harness, "rust", path, log, HERE / "trim.sh"]
    with subprocess.Popen(argv) as vm:
        deadline = time.monotonic() + 95
        try:
            for phase in CHECKPOINTS:
                line = f"DISK_PHASE:{phase}\n"
                while line not in transcript(log).splitlines(keepends=True):
                    if vm.poll() is not None:
                        raise RuntimeError(f"guest exited before {phase}; see {log}")
                    if time.monotonic() >= deadline:
                        raise TimeoutError(f"guest did not reach {phase}; see {log}")
                    time.sleep(0.05)
                sizes[phase] = allocated(image)
                print(f"{mode} {phase}: {sizes[phase]} allocated bytes", flush=True)
                (path / "share" / f"ack-{phase}").touch(exist_ok=False)
            if vm.wait(timeout=max(1, deadline - time.monotonic())) != 0:
                raise RuntimeError(f"guest failed while stopping; see {log}")
        except BaseException:
            if vm.poll() is None:
                vm.terminate()
            raise

    sizes["stopped"] = allocated(image)
    text = transcript(log)
    return {
        "cache_mode": mode,
        "logical_bytes": image.stat().st_size,
        "allocated_bytes": sizes,
        "discard_granularity": int(require_match(r"^DISCARD_GRANULARITY=(\d+)$", text)),
        "discard_max_bytes": int(require_match(r"^DISCARD_MAX_BYTES=(\d+)$", text)),
        "trim_status": int(require_match(r"^TRIM_STATUS:(\d+)$", text)),
        "retained_sha256": require_match(r"^RETAINED_SHA256:([0-9a-f]{64})$", text),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("harness", type=pathlib.Path)
    parser.add_argument("fixtures", type=pathlib.Path)
    parser.add_argument("iso", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path, help="new directory on APFS")
    args = parser.parse_args()
    output = args.output.absolute()
    output.mkdir(mode=0o700)
    packages = output / "packages"
    prepare_packages(args.iso.resolve(), packages)
    harness = args.harness.resolve()
    trials = []
    for mode in CACHE_MODES:
        path = output / mode
        fixture(path, args.fixtures.resolve(), packages)
        trials.append(trim_trial(harness, path, mode))

    results = {"released_bytes": RELEASED_BYTES, "trim": trials}
    result = json.dumps(results, indent=2)
    (output / "measurements.json").write_text(result + "\n")
    print(result)


if __name__ == "__main__":
    main()
