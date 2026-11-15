# SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
# SPDX-License-Identifier: GPL-3.0-or-later
"""A blind TCP relay: run.sh --live's loopback stand-in for jmap-mockd.

ci/gui-scenario/run.sh --live writes an account pointed at
127.0.0.1:<port>, the same shape jmap-mockd always answers on, so nothing
else in a scenario needs to know whether a mock or a real server is behind
it. This relay forwards raw bytes both ways to the real server named on the
command line. It does not parse HTTP, so a redirect, a chunked EventSource
stream and a session document that advertises an address the container
cannot reach (a server's own configured public hostname, unrelated to the
address this relay was told to dial) all pass through unexamined. The real
server's advertised apiUrl/downloadUrl/uploadUrl/eventSourceUrl therefore
never point back here on their own; run.sh sets JMAP_LIVE_SERVER_REBASE_URLS
for a --live run, so the client falls back to the dialled origin's own
scheme and authority, keeping only the path, the same mechanism the
`--features live-server` harness already uses for a server behind a tunnel.

Run standalone: `live_proxy.py <listen-port> <target-host> <target-port>`.
"""

import socket
import sys
import threading

from fault_proxy import _raw_pump, _shutdown


def _handle(conn, target_host, target_port):
    try:
        upstream = socket.create_connection((target_host, target_port))
    except OSError:
        _shutdown(conn)
        return
    to_upstream = threading.Thread(target=_raw_pump, args=(conn, upstream), daemon=True)
    to_client = threading.Thread(target=_raw_pump, args=(upstream, conn), daemon=True)
    to_upstream.start()
    to_client.start()
    to_upstream.join()
    to_client.join()
    _shutdown(conn)
    _shutdown(upstream)


def serve(listen_port, target_host, target_port):
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", listen_port))
    server.listen(16)
    while True:
        conn, _ = server.accept()
        threading.Thread(target=_handle, args=(conn, target_host, target_port), daemon=True).start()


if __name__ == "__main__":
    serve(int(sys.argv[1]), sys.argv[2], int(sys.argv[3]))
