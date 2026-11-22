#!/usr/bin/env python3
"""Run the signed Marsa qualification smoke check in a logged-in Tart guest."""

import argparse
import hashlib
from pathlib import Path, PurePosixPath
import subprocess


TRANSFER_ARCHIVE = "/Volumes/Marsa Qualification/Marsa-VM-Qualification.zip"
SUCCESS = "Marsa first launch: daemon socket ready"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("vm", help="running Tart VM with Guest Agent")
    parser.add_argument("host_archive", type=Path, help="trusted notarized ZIP on the host")
    parser.add_argument("--guest-archive", default=TRANSFER_ARCHIVE, help="ZIP path in the guest")
    args = parser.parse_args()
    if not PurePosixPath(args.guest_archive).is_absolute():
        parser.error("--guest-archive must be an absolute path")

    try:
        with args.host_archive.open("rb") as archive:
            checksum = hashlib.file_digest(archive, "sha256").hexdigest()
        script = Path(__file__).with_name("check-qualification-guest.sh").read_text(encoding="utf-8")
        with subprocess.Popen(
            ["tart", "exec", "-i", "--", args.vm, "/bin/bash", "-s", "--", args.guest_archive, checksum],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        ) as process:
            try:
                output, _ = process.communicate(script, timeout=150)
            except subprocess.TimeoutExpired:
                process.kill()
                output, _ = process.communicate()
                print(output, end="", flush=True)
                parser.exit(1, "Tart did not finish the qualification within 150 seconds.\n")
        print(output, end="", flush=True)
        if process.returncode or SUCCESS not in output.splitlines():
            parser.exit(1, f"Tart qualification did not complete (guest exit {process.returncode}).\n")
    except OSError as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
