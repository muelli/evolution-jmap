# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""A small HTTP proxy that resets one connection to simulate a transport
failure (ROADMAP item 182(b), "offline-replay-transport-error").

A killed jmap-mockd cannot reproduce `jmap-mail::transfer`'s `Err(other)`
arm through the real GUI: a dead mock fails the reconnect itself, so the
account never gets to the point where one more-specific call can fail
(`notes/OFFLINE-REPLAY-TRANSPORT-ERROR-GUI-GAP-2026-11-15.md`). This proxy
sits in front of a live mock instead, forwarding every request/response
pair unchanged until `trigger` first appears in a request body, then
closes both ends of that connection without forwarding the request that
carried it: a connection reset mid-request, not a connect failure. After
that first hit it forwards normally for the rest of its life, so a retry
of the same call succeeds.

It must frame requests and responses, not just relay bytes blindly: the
mock's own session response (`/.well-known/jmap`) names its `apiUrl`,
`eventSourceUrl`, `downloadUrl` and `uploadUrl` with the address the mock
itself was started on, not the address the client dialled (the same
nuance `STALWART_URL` has against a live server). Left unrewritten, the
client would read the mock's own host:port from the session and call it
directly from then on, straight past this proxy, and the mock's own port
has no fault to give.
An EventSource response (`Content-Type: text/event-stream`, chunked, no
`Content-Length`) is a different shape: a long-lived stream with no
framed end. Once its headers pass, the connection falls back to a blind
byte relay for the rest of its life; no further request/response framing
applies to it, and nothing in it needs rewriting.

Run standalone: `fault_proxy.py <listen-port> <target-port> [trigger]`.
A scenario starts it detached in its own "setup" step (before Evolution
runs) and keeps its pid to stop it at the end of "run".
"""

import re
import socket
import sys
import threading

BUFSIZE = 65536
_CONTENT_LENGTH_RE = re.compile(rb"(?im)^content-length:\s*\d+\r?\n")


def _shutdown(sock):
    try:
        sock.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    try:
        sock.close()
    except OSError:
        pass


class _Buffered:
    """A socket with a small read-ahead buffer, so a header line read with
    `readline` never consumes bytes of the body or of the next message.
    """

    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def _fill(self):
        chunk = self.sock.recv(BUFSIZE)
        if not chunk:
            return False
        self.buf += chunk
        return True

    def readline(self):
        """One line, `\\r\\n` or `\\n` included, or b"" at EOF."""
        while b"\n" not in self.buf:
            if not self._fill():
                line, self.buf = self.buf, b""
                return line
        index = self.buf.index(b"\n") + 1
        line, self.buf = self.buf[:index], self.buf[index:]
        return line

    def read_exact(self, size):
        while len(self.buf) < size:
            if not self._fill():
                break
        data, self.buf = self.buf[:size], self.buf[size:]
        return data

    def drain(self):
        data, self.buf = self.buf, b""
        return data


def _read_message(buffered):
    """Read one HTTP request or response: the start line, every header
    line, the blank line that ends them, and the body `Content-Length`
    names (empty if the header is absent). Return (header_bytes, headers,
    body), or None at EOF before a start line arrives. `headers` maps a
    lowercased header name to its raw value, for the one header this
    proxy itself reads (`Content-Length`).
    """
    start_line = buffered.readline()
    if not start_line:
        return None
    header_lines = [start_line]
    headers = {}
    while True:
        line = buffered.readline()
        header_lines.append(line)
        if not line or line in (b"\r\n", b"\n"):
            break
        if b":" in line:
            name, value = line.split(b":", 1)
            headers[name.strip().lower()] = value.strip()
    content_length = headers.get(b"content-length")
    body = buffered.read_exact(int(content_length)) if content_length is not None else b""
    return b"".join(header_lines), headers, body


def _with_content_length(header_block, new_length):
    replacement = f"Content-Length: {new_length}\r\n".encode()
    return _CONTENT_LENGTH_RE.sub(replacement, header_block, count=1)


def _raw_pump(src, dst):
    try:
        while True:
            data = src.recv(BUFSIZE)
            if not data:
                return
            dst.sendall(data)
    except OSError:
        return


def _blind_relay(client, upstream):
    """Fall back to an unframed byte relay for the rest of this
    connection's life, draining each side's read-ahead buffer first so no
    byte already read by `_read_message` is lost.
    """
    leftover = client.drain()
    if leftover:
        upstream.sock.sendall(leftover)
    leftover = upstream.drain()
    if leftover:
        client.sock.sendall(leftover)
    to_upstream = threading.Thread(
        target=_raw_pump, args=(client.sock, upstream.sock), daemon=True
    )
    to_client = threading.Thread(target=_raw_pump, args=(upstream.sock, client.sock), daemon=True)
    to_upstream.start()
    to_client.start()
    to_upstream.join()
    to_client.join()


def _handle(conn, target_host, target_port, listen_port, trigger, fired):
    try:
        upstream_sock = socket.create_connection((target_host, target_port))
    except OSError:
        _shutdown(conn)
        return
    client = _Buffered(conn)
    upstream = _Buffered(upstream_sock)
    mock_authority = f"{target_host}:{target_port}".encode()
    proxy_authority = f"{target_host}:{listen_port}".encode()
    try:
        while True:
            request = _read_message(client)
            if request is None:
                return
            header_block, _headers, body = request
            if trigger is not None and not fired.is_set() and trigger in body:
                fired.set()
                return
            upstream_sock.sendall(header_block + body)

            response = _read_message(upstream)
            if response is None:
                return
            resp_header_block, resp_headers, resp_body = response
            if mock_authority in resp_body:
                resp_body = resp_body.replace(mock_authority, proxy_authority)
                resp_header_block = _with_content_length(resp_header_block, len(resp_body))
            conn.sendall(resp_header_block + resp_body)

            if b"content-length" not in resp_headers:
                # A streaming response (EventSource): no framed end, so no
                # further request can share this connection either.
                _blind_relay(client, upstream)
                return
    except OSError:
        return
    finally:
        _shutdown(conn)
        _shutdown(upstream_sock)


def serve(listen_port, target_port, trigger, target_host="127.0.0.1"):
    fired = threading.Event()
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", listen_port))
    server.listen(16)
    while True:
        conn, _ = server.accept()
        threading.Thread(
            target=_handle,
            args=(conn, target_host, target_port, listen_port, trigger, fired),
            daemon=True,
        ).start()


if __name__ == "__main__":
    listen_port = int(sys.argv[1])
    target_port = int(sys.argv[2])
    trigger = sys.argv[3].encode() if len(sys.argv) > 3 else None
    serve(listen_port, target_port, trigger)
