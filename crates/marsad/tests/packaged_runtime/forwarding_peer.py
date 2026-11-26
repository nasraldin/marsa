"""Run inside the separate Tart guest. Positive replies must contain the fixture nonce."""
import errno
import http.client
import socket
import sys

address, port, protocol, nonce, expectation = sys.argv[1:]
port = int(port)
family = socket.AF_INET6 if ":" in address else socket.AF_INET
kind = socket.SOCK_STREAM if protocol == "tcp" else socket.SOCK_DGRAM
with socket.socket(family, kind) as client:
    client.settimeout(3 if expectation == "blocked" else 12)
    try:
        client.connect((address, port))
        if protocol == "udp":
            client.send(nonce.encode())
            data = client.recv(1024)
    except OSError as error:
        if expectation != "blocked" or not (
            isinstance(error, socket.timeout) or error.errno == errno.ECONNREFUSED
        ):
            raise
    else:
        assert expectation == "reachable", "peer reached a private publication"
        if protocol == "tcp":
            request = f"GET /{nonce} HTTP/1.0\r\nHost: localhost\r\n\r\n"
            client.sendall(request.encode())
            response = http.client.HTTPResponse(client)
            response.begin()
            assert response.status == 200, response.status
            data = response.read(1024)
        assert data == nonce.encode(), (expectation, data)
