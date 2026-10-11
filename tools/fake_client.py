#!/usr/bin/env python3
"""Scripted offline-mode 1.20.1 (protocol 763) client: full join sequence test.

Exits 0 if: login completes, Login(play) received, >=1 chunk received,
teleport confirmed. Prints a packet tally and chunk stats.
"""
import socket
import struct
import sys
import time
import zlib
from collections import Counter

COMPRESSION = -1  # -1 = disabled


def varint(n):
    out = b""
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out += bytes([b | 0x80])
        else:
            return out + bytes([b])


def read_varint_from(buf, i):
    n = 0
    shift = 0
    while True:
        b = buf[i]
        i += 1
        n |= (b & 0x7F) << shift
        if not b & 0x80:
            return n, i
        shift += 7


def pack_string(s):
    b = s.encode()
    return varint(len(b)) + b


def packet(pid, payload=b""):
    body = varint(pid) + payload
    if COMPRESSION < 0:
        return varint(len(body)) + body
    if len(body) < COMPRESSION:
        inner = varint(0) + body
        return varint(len(inner)) + inner
    compressed = zlib.compress(body)
    inner = varint(len(body)) + compressed
    return varint(len(inner)) + inner


class Conn:
    def __init__(self, sock):
        self.sock = sock
        self.buf = b""

    def _need(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise EOFError
            self.buf += chunk

    def read_frame(self):
        i = 0
        while True:
            self._need(i + 1)
            if not self.buf[i] & 0x80:
                break
            i += 1
        length, off = read_varint_from(self.buf, 0)
        self._need(off + length)
        data = self.buf[off : off + length]
        self.buf = self.buf[off + length :]
        if COMPRESSION < 0:
            return data
        dlen, j = read_varint_from(data, 0)
        if dlen == 0:
            return data[j:]
        raw = zlib.decompress(data[j:])
        assert len(raw) == dlen
        return raw

    def send(self, data):
        self.sock.sendall(data)


def main():
    global COMPRESSION
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566
    name = sys.argv[3] if len(sys.argv) > 3 else "RustTest"
    start = time.time()
    sock = socket.create_connection((host, port), timeout=15)
    sock.settimeout(30)
    c = Conn(sock)

    hs = varint(763) + pack_string(host) + struct.pack(">H", port) + varint(2)
    c.send(packet(0x00, hs))
    c.send(packet(0x00, pack_string(name) + b"\x00" * 16))  # Login Start: name + uuid

    tally = Counter()
    chunks = 0
    chunk_bytes = 0
    teleported = False
    joined = False
    deadline = time.time() + 25

    try:
        while time.time() < deadline:
            body = c.read_frame()
            pid, i = read_varint_from(body, 0)
            tally[pid] += 1

            if pid == 0x03 and not joined:  # Set Compression (login state)
                thr, _ = read_varint_from(body, i)
                COMPRESSION = thr
                continue
            if pid == 0x04 and not joined:  # Login Plugin Request (login state)
                # A vanilla client does not know any custom-login channel (Forge
                # uses fml:loginwrapper) and answers every such query with
                # successful=false, echoing the transaction id. Servers with
                # startech-forge-native=true wait for exactly this.
                tid, _ = read_varint_from(body, i)
                c.send(packet(0x02, varint(tid) + b"\x00"))
                continue
            if pid == 0x02 and not joined:  # Login Success
                joined = True
                continue
            if pid == 0x3C and not teleported:  # Synchronize Player Position -> Confirm Teleportation
                tid, _ = read_varint_from(body, i)
                c.send(packet(0x00, varint(tid)))
                teleported = True
                continue
            if pid == 0x24:  # Level Chunk With Light
                chunks += 1
                chunk_bytes += len(body)
            if pid == 0x23:  # Keep Alive -> reply
                keep_id = body[i : i + 8]
                c.send(packet(0x12, keep_id))
            if pid == 0x1A:  # Disconnect (play)
                print("DISCONNECTED BY SERVER:", body[i:].decode("utf-8", "replace"))
                break
            # Done once joined+teleported+chunks AND one keep-alive round-trip.
            if teleported and chunks > 0 and tally.get(0x23, 0) > 0:
                break
    except (EOFError, OSError) as e:
        print(f"connection ended early after {time.time() - start:.1f}s: {type(e).__name__}: {e}")
        print("tally so far:", dict(sorted(tally.items())))

    sock.close()
    ok = joined and chunks > 0 and teleported
    print(f"joined={joined} teleport_confirmed={teleported} chunks={chunks} chunk_bytes={chunk_bytes}")
    print("packet tally (id: count):", dict(sorted(tally.items())))
    print("JOIN TEST:", "PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
