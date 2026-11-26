"""Replace only the DHCP applet symlink in a disposable copy of the packaged guest."""
import io
from pathlib import Path
import sys
import tarfile

launcher = Path(__file__).with_name("udhcpc").read_bytes()
count = 0
with tarfile.open(sys.argv[1]) as guest, tarfile.open(sys.argv[2], "x") as image:
    for entry in guest:
        if entry.name == "./sbin/udhcpc":
            if not entry.issym() or entry.linkname != "/bin/busybox":
                raise ValueError("expected packaged BusyBox udhcpc applet")
            replacement = tarfile.TarInfo(entry.name)
            replacement.mode = 0o755
            replacement.size = len(launcher)
            image.addfile(replacement, io.BytesIO(launcher))
            count += 1
        else:
            image.addfile(entry, guest.extractfile(entry) if entry.isreg() else None)
if count != 1:
    raise ValueError("expected one DHCP applet")
