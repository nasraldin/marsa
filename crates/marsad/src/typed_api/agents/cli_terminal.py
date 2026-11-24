"""Answer a CLI prompt on a private pseudo-terminal after checking the review."""

import json
import os
import pty
import select
import subprocess
import sys

cli, approval, target, answer = sys.argv[1:]
master, slave = pty.openpty()
with subprocess.Popen(
    [cli, "agents", "approve", approval, "--json"],
    stdin=slave,
    stderr=slave,
    stdout=subprocess.PIPE,
) as child:
    os.close(slave)
    try:
        reviewed = b""
        while b"Type yes and press Enter." not in reviewed:
            if not select.select([master], [], [], 5)[0]:
                raise RuntimeError("the CLI approval prompt did not appear")
            reviewed += os.read(master, 4096)
        assert "\u202e".encode() not in reviewed
        assert b"\\u202e" in reviewed
        details = json.loads(reviewed.split(b"Approve this request once?", 1)[0])
        assert details["id"] == approval, details
        assert details["operation"] == "AGENT_OPERATION_STOP_CONTAINER", details
        assert details["project_path"], details
        assert details["redacted_arguments"]["container_id"] == target, details
        os.write(master, (answer + "\n").encode())
        out, _ = child.communicate(timeout=5)
        result = json.loads(out)
        if answer == "yes":
            assert child.returncode == 0, result
            assert result["data"] == {"decision": "approved", "approval_id": approval}, result
        else:
            assert child.returncode == 3, result
            assert result["code"] == "confirmation_declined", result
    finally:
        if child.poll() is None:
            child.kill()
        os.close(master)
