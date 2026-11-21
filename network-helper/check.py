"""Test the helper on a private socketpair using guest Ethernet frames."""

import argparse
import select
import socket
import struct
import subprocess
import time

GUEST = b"\x5a\x94\xef\xe4\x0c\xee"
GATEWAY = b"\x5a\x94\xef\xe4\x0c\xdd"
BROADCAST = b"\xff" * 6
GUEST_IP = "192.168.127.2"
GATEWAY_IP = "192.168.127.1"


def ipv4_checksum(header):
    value = sum(struct.unpack("!10H", header))
    value = (value & 0xFFFF) + (value >> 16)
    return 0xFFFF ^ ((value & 0xFFFF) + (value >> 16))


def udp_frame(source, destination, port, payload, *, source_port=41414, peer=GATEWAY):
    datagram = struct.pack("!HHHH", source_port, port, 8 + len(payload), 0) + payload
    header = struct.pack(
        "!BBHHHBBH4s4s", 0x45, 0, 20 + len(datagram), 1, 0, 64, 17, 0,
        socket.inet_aton(source), socket.inet_aton(destination),
    )
    header = header[:10] + struct.pack("!H", ipv4_checksum(header)) + header[12:]
    return peer + GUEST + b"\x08\x00" + header + datagram


def arp_frame():
    return BROADCAST + GUEST + b"\x08\x06" + struct.pack(
        "!HHBBH6s4s6s4s", 1, 0x0800, 6, 4, 1, GUEST, socket.inet_aton(GUEST_IP),
        bytes(6), socket.inet_aton(GATEWAY_IP),
    )


def receive(peer, matching):
    deadline = time.monotonic() + 5
    while (remaining := deadline - time.monotonic()) > 0:
        if not select.select([peer], [], [], remaining)[0]:
            break
        message = peer.recv(1514)
        if matching(message):
            return message
    raise AssertionError("the helper did not send the expected Ethernet reply")


def udp_payload(message, source_port, destination_port):
    if len(message) < 42 or message[12:14] != b"\x08\x00" or message[23] != 17:
        return b""
    offset = 14 + (message[14] & 15) * 4
    ports = struct.pack("!HH", source_port, destination_port)
    if message[offset:offset + 4] != ports:
        return b""
    return message[offset + 8:]


def dns_query(peer, name, qtype, outcome=0, record=True):
    question = b"".join(bytes([len(label)]) + label for label in name.rstrip(".").encode().split(b"."))
    query = struct.pack("!6H", 0x1234, 0x0100, 1, 0, 0, 0)
    query += question + b"\x00" + struct.pack("!HH", qtype, 1)
    peer.send(udp_frame(GUEST_IP, GATEWAY_IP, 53, query))
    reply = udp_payload(receive(peer, lambda frame: bool(udp_payload(frame, 53, 41414))), 53, 41414)
    identifier, flags, _, answers, _, _ = struct.unpack("!6H", reply[:12])
    assert identifier == 0x1234 and flags & 15 == outcome, (name, qtype, flags & 15)
    if record:
        assert answers > 0, (name, qtype)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("helper")
    parser.add_argument("--scoped-name", help="Optional host-resolved DNS name to test")
    parser.add_argument("--record-type", type=int, default=1, help="Type for --scoped-name, default A (1)")
    options = parser.parse_args()
    # An ordinary pipe or stream cannot be mistaken for the privileged transport.
    invalid = subprocess.run(
        [options.helper], input=b"", capture_output=True, timeout=5, check=False,
    )
    assert invalid.returncode and not invalid.stdout
    assert b"connected Unix datagram socket" in invalid.stderr

    peer, helper = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
    with peer, helper:
        process = subprocess.Popen([options.helper], stdin=helper, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        helper.close()
        try:
            assert select.select([process.stdout], [], [], 10)[0], "helper startup timed out"
            assert process.stdout.readline() == b"READY\n", "helper did not report readiness"
            peer.send(b"not Ethernet")
            wrong_mac = arp_frame()[:6] + b"\x02" * 6 + arp_frame()[12:]
            peer.send(wrong_mac)
            assert not select.select([peer], [], [], 0.25)[0], "foreign frame accepted"
            peer.send(arp_frame())
            arp = receive(peer, lambda message: message[:14] == GUEST + GATEWAY + b"\x08\x06")
            assert arp[20:22] == b"\x00\x02"

            discover = struct.pack(
                "!BBBBIHH4s4s4s4s16s64s128s", 1, 1, 6, 0, 0x12AB3456, 0, 0x8000,
                bytes(4), bytes(4), bytes(4), bytes(4), GUEST, bytes(64), bytes(128),
            )
            discover += b"\x63\x82\x53\x63\x35\x01\x01\xff"
            peer.send(udp_frame("0.0.0.0", "255.255.255.255", 67, discover, source_port=68, peer=BROADCAST))
            offer = udp_payload(receive(peer, lambda frame: bool(udp_payload(frame, 67, 68))), 67, 68)
            assert offer[4:8] == discover[4:8] and offer[16:20] == socket.inet_aton(GUEST_IP)

            dns_query(peer, "localhost.", 1)
            dns_query(peer, "localhost.", 255, outcome=4, record=False)
            if options.scoped_name:
                dns_query(peer, options.scoped_name, options.record_type)

            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as listener:
                listener.bind(("127.0.0.1", 0))
                peer.send(udp_frame(GUEST_IP, "127.0.0.1", listener.getsockname()[1], b"must be blocked"))
                assert not select.select([listener], [], [], 0.5)[0], "guest reached a host loopback listener"
        finally:
            if process.poll() is None:
                process.terminate()
            stdout, stderr = process.communicate(timeout=8)
            assert process.returncode == 0, stderr.decode(errors="replace")
            assert not stdout
    print("socketpair, Ethernet validation, DHCP, system DNS, loopback isolation and shutdown passed")


if __name__ == "__main__":
    main()
