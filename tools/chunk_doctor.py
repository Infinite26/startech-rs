#!/usr/bin/env python3
"""Strict structural decoder for our Level Chunk With Light packets (0x24).

Reparses the Data segment and light fields exactly as the vanilla 1.20.1
client does and reports the first inconsistency. Run: chunk_doctor.py [host] [port]
"""
import socket
import struct
import sys
import time
import zlib
from collections import Counter

COMPRESSION = -1


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
        if i >= len(buf):
            raise EOFError("varint past end")
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


# ---- minimal NBT reader ----
def nbt_read(buf, i):
    tag, i = buf[i], i + 1
    if tag == 0:
        return None, i
    nlen = struct.unpack_from(">H", buf, i)[0]
    i += 2
    i += nlen  # skip root name
    return nbt_payload(buf, i, tag)


def nbt_payload(buf, i, tag):
    if tag == 1:
        return buf[i], i + 1
    if tag == 2:
        return struct.unpack_from(">h", buf, i)[0], i + 2
    if tag == 3:
        return struct.unpack_from(">i", buf, i)[0], i + 4
    if tag == 4:
        return struct.unpack_from(">q", buf, i)[0], i + 8
    if tag == 5:
        return struct.unpack_from(">f", buf, i)[0], i + 4
    if tag == 6:
        return struct.unpack_from(">d", buf, i)[0], i + 8
    if tag == 7:
        n = struct.unpack_from(">i", buf, i)[0]
        return buf[i + 4 : i + 4 + n], i + 4 + n
    if tag == 8:
        n = struct.unpack_from(">H", buf, i)[0]
        return buf[i + 2 : i + 2 + n].decode("utf-8", "replace"), i + 2 + n
    if tag == 9:
        elem = buf[i]
        n = struct.unpack_from(">i", buf, i + 1)[0]
        i += 5
        out = []
        for _ in range(n):
            v, i = nbt_payload(buf, i, elem)
            out.append(v)
        return out, i
    if tag == 10:
        out = {}
        while True:
            t = buf[i]
            i += 1
            if t == 0:
                return out, i
            nlen = struct.unpack_from(">H", buf, i)[0]
            name = buf[i + 2 : i + 2 + nlen].decode("utf-8", "replace")
            i += 2 + nlen
            v, i = nbt_payload(buf, i, t)
            out[name] = v
    if tag == 11:
        n = struct.unpack_from(">i", buf, i)[0]
        return list(struct.unpack_from(f">{n}i", buf, i + 4)), i + 4 + 4 * n
    if tag == 12:
        n = struct.unpack_from(">i", buf, i)[0]
        return list(struct.unpack_from(f">{n}q", buf, i + 4)), i + 4 + 8 * n
    raise ValueError(f"bad nbt tag {tag}")


class Fail(Exception):
    pass


def check(cond, msg):
    if not cond:
        raise Fail(msg)


def decode_container(buf, i, entries, indirect_min, indirect_max, direct_bits, what):
    bpe = buf[i]
    i += 1
    if bpe == 0:
        val, i = read_varint_from(buf, i)
        dlen, i = read_varint_from(buf, i)
        check(dlen == 0, f"{what}: single-value has data len {dlen}")
        check(val < (1 << 20), f"{what}: single value {val} insane")
        return i, "single"
    epl = 64 // bpe
    want_longs = -(-entries // epl)
    if indirect_min <= bpe <= indirect_max:
        plen, i = read_varint_from(buf, i)
        check(plen > 0, f"{what}: empty indirect palette")
        check(plen <= (1 << bpe), f"{what}: palette len {plen} > 2^{bpe}")
        for _ in range(plen):
            v, i = read_varint_from(buf, i)
            check(v < 24135, f"{what}: palette state {v} out of range")
        dlen, i = read_varint_from(buf, i)
        check(
            dlen == want_longs,
            f"{what}: indirect bpe={bpe} data longs {dlen} != expected {want_longs}",
        )
        return i + 8 * dlen, f"indirect bpe={bpe} pal={plen}"
    if bpe == direct_bits:
        dlen, i = read_varint_from(buf, i)
        check(
            dlen == want_longs,
            f"{what}: direct bpe={bpe} data longs {dlen} != expected {want_longs}",
        )
        return i + 8 * dlen, "direct"
    raise Fail(f"{what}: illegal bpe {bpe}")


def decode_chunk(payload):
    cx = struct.unpack_from(">i", payload, 0)[0]
    cz = struct.unpack_from(">i", payload, 4)[0]
    hm, off = nbt_read(payload, 8)
    check(isinstance(hm, dict), "heightmaps not a compound")
    for key in ("MOTION_BLOCKING", "WORLD_SURFACE"):
        if key in hm:
            arr = hm[key]
            check(isinstance(arr, list) and len(arr) == 37, f"{key}: {len(arr) if isinstance(arr, list) else type(arr)} longs, want 37")
            check(all(0 <= v <= 511 for v in arr), f"{key}: values out of 9-bit range")
    size, off = read_varint_from(payload, off)
    data = payload[off : off + size]
    check(len(data) == size, f"Size says {size}, only {len(data)} bytes left")
    off += size
    i = 0
    for s in range(24):
        if i + 2 > len(data):
            raise Fail(f"section {s}: truncated at {i}/{len(data)}")
        count = struct.unpack_from(">h", data, i)[0]
        i += 2
        check(0 <= count <= 4096, f"section {s}: block count {count} out of range")
        i, how_b = decode_container(data, i, 4096, 4, 8, 15, f"section {s} blocks")
        i, how_m = decode_container(data, i, 64, 1, 3, 6, f"section {s} biomes")
    check(i == len(data), f"trailing {len(data) - i} bytes inside Data after 24 sections")
    # Block entity count is a SEPARATE packet field after Data (doc §7 fields 6-7)
    bcount, off = read_varint_from(payload, off)
    check(bcount == 0, f"unexpected {bcount} block entities")
    # light
    masks = []
    for name in ("sky_mask", "block_mask", "empty_sky_mask", "empty_block_mask"):
        nlongs, off = read_varint_from(payload, off)
        longs = list(struct.unpack_from(f">{nlongs}q", payload, off))
        off += 8 * nlongs
        bits = sum(bin(l & (1 << b) != 0).count("1") for l in longs for b in range(64))
        masks.append((nlongs, bits))
    for arrname, maskname in (("sky arrays", "sky_mask"), ("block arrays", "block_mask")):
        n, off = read_varint_from(payload, off)
        expect = masks[["sky_mask", "block_mask"].index(maskname)][1]
        check(n == expect, f"{arrname}: {n} arrays but mask has {expect} bits set")
        for k in range(n):
            ln, off = read_varint_from(payload, off)
            check(ln == 2048, f"{arrname}[{k}]: length {ln}, want 2048")
            off += ln
    check(off == len(payload), f"{len(payload) - off} trailing bytes after light")
    return cx, cz, size, how_b, how_m, masks


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
        if len(raw) != dlen:
            raise Fail(f"frame decompressed to {len(raw)}, header said {dlen}")
        return raw

    def send(self, data):
        self.sock.sendall(data)


def main():
    global COMPRESSION
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 25566
    sock = socket.create_connection((host, port), timeout=15)
    sock.settimeout(20)
    c = Conn(sock)
    c.send(packet(0x00, varint(763) + pack_string(host) + struct.pack(">H", port) + varint(2)))
    c.send(packet(0x00, pack_string("ChunkDoctor") + b"\x00" * 16))

    tally = Counter()
    bad = 0
    decoded = 0
    deadline = time.time() + 30
    try:
        while time.time() < deadline:
            body = c.read_frame()
            pid, i = read_varint_from(body, 0)
            tally[pid] += 1
            if pid == 0x03:
                COMPRESSION, _ = read_varint_from(body, i)
            elif pid == 0x3C:
                tid, _ = read_varint_from(body, i)
                c.send(packet(0x00, varint(tid)))
            elif pid == 0x23:
                c.send(packet(0x12, body[i : i + 8]))
            elif pid == 0x24:
                try:
                    cx, cz, size, how_b, how_m, masks = decode_chunk(body[i:])
                    decoded += 1
                    if decoded <= 3:
                        print(f"chunk ({cx},{cz}): size={size} blocks={how_b} biomes={how_m} light_masks={masks}")
                except Fail as e:
                    bad += 1
                    cx = struct.unpack_from(">i", body, i)[0]
                    cz = struct.unpack_from(">i", body, i + 4)[0]
                    print(f"BAD chunk ({cx},{cz}): {e}")
                    if bad >= 5:
                        break
            if decoded + bad >= 40:
                break
    except (EOFError, OSError) as e:
        print(f"connection ended after {decoded} decoded, {bad} bad: {type(e).__name__}: {e}")
    finally:
        sock.close()
    print(f"RESULT: decoded_ok={decoded} bad={bad} tally={dict(sorted(tally.items()))}")
    print("DOCTOR:", "PASS (all sampled chunks structurally valid)" if bad == 0 and decoded > 0 else "FAIL")


if __name__ == "__main__":
    main()
