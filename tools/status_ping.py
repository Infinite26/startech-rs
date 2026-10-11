#!/usr/bin/env python3
"""Status ping a Minecraft server (handshake next-state 1)."""
import json
import socket
import struct
import sys
import time


def varint(n):
    out = b""
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out += bytes([b | 0x80])
        else:
            out += bytes([b])
            return out


def pack_string(s):
    b = s.encode()
    return varint(len(b)) + b


def packet(pid, payload=b""):
    body = varint(pid) + payload
    return varint(len(body)) + body


def read_varint(sock):
    n = 0
    shift = 0
    while True:
        b = sock.recv(1)
        if not b:
            raise EOFError
        n |= (b[0] & 0x7F) << shift
        if not b[0] & 0x80:
            return n
        shift += 7


def read_frame(sock):
    length = read_varint(sock)
    data = b""
    while len(data) < length:
        chunk = sock.recv(length - len(data))
        if not chunk:
            raise EOFError
        data += chunk
    return data


def main():
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566
    s = socket.create_connection((host, port), timeout=10)
    hs = varint(763) + pack_string(host) + struct.pack(">H", port) + varint(1)
    s.sendall(packet(0x00, hs))
    s.sendall(packet(0x00))
    frame = read_frame(s)
    pid = 0
    shift = 0
    i = 0
    while True:
        b = frame[i]
        pid |= (b & 0x7F) << shift
        i += 1
        if not b & 0x80:
            break
        shift += 7
    assert pid == 0x00, hex(pid)
    slen = 0
    shift = 0
    while True:
        b = frame[i]
        slen |= (b & 0x7F) << shift
        i += 1
        if not b & 0x80:
            break
        shift += 7
    payload = json.loads(frame[i : i + slen])
    t0 = time.time()
    s.sendall(packet(0x01, struct.pack(">q", 12345)))
    pong = read_frame(s)
    rtt = (time.time() - t0) * 1000
    s.close()
    print(json.dumps({"status": payload, "pong_ok": pong.startswith(b"\x01"), "rtt_ms": round(rtt, 1)}, indent=2))


if __name__ == "__main__":
    main()
