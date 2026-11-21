"""Match executable files and applet links to their owning components."""

from pathlib import PurePosixPath
import posixpath


def verify_inventory(rootfs, components):
    owners = {path: component for component in reversed(components) for path in component["files"]}
    members = {member.name.removeprefix("./"): member for member in rootfs.getmembers()}
    for path in owners:
        if path not in members:
            raise ValueError(f"Guest inventory refers to absent file {path}")

    for path, member in members.items():
        if path in owners or member.isdir():
            continue
        if member.isfile():
            with rootfs.extractfile(member) as contents:
                executable = contents.read(4).startswith((b"\x7fELF", b"#!"))
            if executable or member.mode & 0o111:
                raise ValueError(f"Guest executable has no version and licence: {path}")
        elif member.issym() and in_executable_directory(path):
            target = posixpath.normpath(
                posixpath.join(posixpath.dirname(path), member.linkname)
            ).lstrip("/")
            if target not in owners:
                raise ValueError(f"Guest executable symlink has no inventoried target: {path}")
            owners[target]["files"].append(path)
        elif member.islnk():
            raise ValueError(f"Guest hardlink has no inventoried owner: {path}")

    for component in components:
        component["files"].sort()


def in_executable_directory(path):
    return PurePosixPath(path).parent.parts in {
        ("bin",), ("sbin",), ("usr", "bin"), ("usr", "sbin"), ("usr", "local", "bin"),
        ("usr", "local", "sbin"),
    }
