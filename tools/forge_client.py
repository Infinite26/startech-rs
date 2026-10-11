#!/usr/bin/env python3
"""Scripted Forge 47.x client: verifies the native FML login handshake.

Independently implements the *client* half of the handshake described in
docs/FORGE-PROTOCOL.md, so agreeing with src/forge.rs is a real check of the
encode/decode symmetry rather than a restatement of it.

Exit codes
    0  the server completed the FML negotiation and logged us in
    1  the handshake did not match the expected Forge wire format
    2  the join failed after a correct handshake
"""
import socket
import struct
import sys
import time
import zlib

COMPRESSION = -1  # -1 = disabled
DECLINE = False  # --decline: behave like a vanilla client

FML_NET_VERSION = 3
NET_VERSION = "FML3"
CH_LOGIN_WRAPPER = "fml:loginwrapper"
CH_HANDSHAKE = "fml:handshake"
CH_PLAY = "fml:play"
CH_REGISTER = "minecraft:register"
CH_UNREGISTER = "minecraft:unregister"

# Discriminators, i.e. the `int id` given to `messageBuilder` in
# NetworkInitialization.getHandshakeChannel().
MSG_S2C_MOD_LIST = 1
MSG_C2S_MOD_LIST_REPLY = 2
MSG_S2C_MOD_DATA = 5


class ProtocolError(Exception):
    pass


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
        if shift > 35:
            raise ProtocolError("varint too long")


def pack_string(s):
    b = s.encode()
    return varint(len(b)) + b


def read_string_from(buf, i):
    length, i = read_varint_from(buf, i)
    return buf[i : i + length].decode(), i + length


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
        if len(raw) != dlen:
            raise ProtocolError("decompressed size mismatch")
        return raw

    def read_packet(self):
        data = self.read_frame()
        pid, off = read_varint_from(data, 0)
        return pid, data[off:]

    def send(self, data):
        self.sock.sendall(data)


def mod_list_reply_body(client_mods, client_channels):
    """C2SModListReply (HandshakeMessages.java:213-229)."""
    out = varint(len(client_mods))
    for m in client_mods:
        out += pack_string(m)
    out += varint(len(client_channels))
    for name, version in client_channels:
        out += pack_string(name) + pack_string(version)
    out += varint(0)  # registry hashes: Forge sends this empty too
    return out


def wrap(target, message_id, body):
    """LoginWrapper.wrapPacket + IndexedMessageCodec.tryEncode."""
    inner = bytes([message_id]) + body
    return pack_string(target) + varint(len(inner)) + inner


def parse_login_query(payload):
    """Decode a Login Plugin Request body: id, channel, then the FML envelope."""
    i = 0
    message_id, i = read_varint_from(payload, i)
    channel, i = read_string_from(payload, i)
    if channel != CH_LOGIN_WRAPPER:
        raise ProtocolError(f"expected {CH_LOGIN_WRAPPER}, got {channel}")
    target, i = read_string_from(payload, i)
    length, i = read_varint_from(payload, i)
    block = payload[i : i + length]
    if len(block) != length:
        raise ProtocolError("fml block truncated")
    if not block:
        raise ProtocolError("fml block empty")
    if i + length != len(payload):
        raise ProtocolError("trailing bytes after fml block")
    return message_id, target, block[0], block[1:]


def parse_mod_list(body):
    i = 0
    n, i = read_varint_from(body, i)
    mods = []
    for _ in range(n):
        s, i = read_string_from(body, i)
        mods.append(s)
    n, i = read_varint_from(body, i)
    channels = []
    for _ in range(n):
        name, i = read_string_from(body, i)
        version, i = read_string_from(body, i)
        channels.append((name, version))
    n, i = read_varint_from(body, i)
    registries = []
    for _ in range(n):
        s, i = read_string_from(body, i)
        registries.append(s)
    n, i = read_varint_from(body, i)
    datapacks = []
    for _ in range(n):
        s, i = read_string_from(body, i)
        datapacks.append(s)
    if i != len(body):
        raise ProtocolError("trailing bytes after S2CModList")
    return mods, channels, registries, datapacks


def parse_mod_data(body):
    i = 0
    n, i = read_varint_from(body, i)
    mods = []
    for _ in range(n):
        mid, i = read_string_from(body, i)
        display, i = read_string_from(body, i)
        version, i = read_string_from(body, i)
        mods.append((mid, display, version))
    if i != len(body):
        raise ProtocolError("trailing bytes after S2CModData")
    return mods


def check(cond, msg):
    if not cond:
        raise ProtocolError(msg)


def main():
    global COMPRESSION, DECLINE
    args = [a for a in sys.argv[1:] if a != "--decline"]
    DECLINE = "--decline" in sys.argv[1:]
    host = args[0] if len(args) > 0 else "127.0.0.1"
    port = int(args[1]) if len(args) > 1 else 25599
    name = args[2] if len(args) > 2 else "ForgeTester"

    sock = socket.create_connection((host, port), timeout=15)
    sock.settimeout(30)
    conn = Conn(sock)

    # 1. Handshake, next_state = 2 (login).
    conn.send(
        packet(0x00, varint(763) + pack_string(host) + struct.pack(">H", port) + varint(2))
    )
    # 2. Login Start.
    conn.send(packet(0x00, pack_string(name) + b"\x00" * 16))

    # 3. Set Compression, then everything after it is compressed.
    pid, body = conn.read_packet()
    check(pid == 0x03, f"expected SetCompression 0x03, got 0x{pid:02x}")
    # The threshold is a VarInt (256 encodes as 0x80 0x02); reading a single byte
    # would yield 128 and silently mis-frame any payload in [128, 256).
    COMPRESSION, _ = read_varint_from(body, 0)

    # 4. The FML handshake: expect S2CModData then S2CModList, one login query
    #    each, carrying a distinct transaction id.
    seen = {}
    for _ in range(2):
        pid, body = conn.read_packet()
        check(
            pid == 0x04,
            f"expected LoginPluginRequest 0x04, got 0x{pid:02x} "
            f"(forge doc §3: all FML login traffic rides this packet)",
        )
        message_id, target, discriminator, payload = parse_login_query(body)
        check(
            target == CH_HANDSHAKE,
            f"expected wrapper target {CH_HANDSHAKE}, got {target}",
        )
        check(
            message_id not in seen,
            f"login index {message_id} reused; each query needs a distinct id",
        )
        seen[message_id] = (discriminator, payload)
        print(f"  login query #{message_id}: {CH_HANDSHAKE} discriminator={discriminator}")

    # Registration order is S2CModData (5) then S2CModList (1).
    check(0 in seen and 1 in seen, f"expected login indices 0 and 1, got {sorted(seen)}")
    check(
        seen[0][0] == MSG_S2C_MOD_DATA,
        f"pos 0 must be S2CModData ({MSG_S2C_MOD_DATA}), got {seen[0][0]}",
    )
    check(
        seen[1][0] == MSG_S2C_MOD_LIST,
        f"pos 1 must be S2CModList ({MSG_S2C_MOD_LIST}), got {seen[1][0]}",
    )

    mod_data = parse_mod_data(seen[0][1])
    print(f"  S2CModData: {mod_data}")
    check(mod_data == [], f"a modless server sends an empty S2CModData, got {mod_data}")

    mods, channels, registries, datapacks = parse_mod_list(seen[1][1])
    # The vanilla path asserts the same envelope, so --decline still checks that
    # the server speaks the documented format before we decline it.
    print(f"  S2CModList: mods={mods} registries={registries} datapacks={datapacks}")
    print(f"  channels: {channels}")
    check(mods == [], f"server declares no mods, got {mods}")
    check(registries == [], f"no Forge registries to sync, got {registries}")
    check(datapacks == [], f"no synced datapack registries, got {datapacks}")
    got = dict(channels)
    for required in (CH_HANDSHAKE, CH_PLAY, CH_REGISTER, CH_UNREGISTER, CH_LOGIN_WRAPPER):
        check(required in got, f"server did not advertise {required}")
        check(
            got[required] == NET_VERSION,
            f"{required} advertised as {got[required]!r}, expected {NET_VERSION!r}",
        )
    check(len(channels) == 5, f"expected exactly 5 channels, got {len(channels)}")

    # 5. Answer S2CModList with C2SModListReply, echoing its transaction id.
    #    S2CModData carries noResponse(), so index 0 is deliberately left
    #    unanswered — a client that replies to it is what Forge logs as an
    #    unexpected index.
    #
    #    --decline simulates a *vanilla* client, which knows nothing about
    #    fml:loginwrapper and answers any login query with successful=false.
    #    The server must treat that as "not modded" and finish the login rather
    #    than dropping the connection, otherwise turning native forge on would
    #    break every vanilla join.
    if DECLINE:
        conn.send(packet(0x02, varint(1) + b"\x00"))
        print("  declined the fml query (vanilla client behaviour)")
    else:
        reply = mod_list_reply_body(
            ["forge"],
            [
                (CH_HANDSHAKE, NET_VERSION),
                (CH_PLAY, NET_VERSION),
                (CH_REGISTER, NET_VERSION),
                (CH_UNREGISTER, NET_VERSION),
                (CH_LOGIN_WRAPPER, NET_VERSION),
            ],
        )
        conn.send(
            packet(
                0x02,
                varint(1) + b"\x01" + wrap(CH_HANDSHAKE, MSG_C2S_MOD_LIST_REPLY, reply),
            )
        )

    # 6. Login Success, then a couple of play packets to prove the join landed.
    pid, body = conn.read_packet()
    if pid == 0x00:
        print(f"  server disconnected after handshake: {body.decode(errors='replace')}")
        print("FORGE HANDSHAKE: FAIL")
        return 1
    check(pid == 0x02, f"expected LoginSuccess 0x02, got 0x{pid:02x}")
    i = 16
    username, i = read_string_from(body, i)
    print(f"  LoginSuccess for {username!r}")

    pid, _ = conn.read_packet()
    check(pid == 0x28, f"expected Login(play) 0x28, got 0x{pid:02x}")
    print("  Login(play) received")

    print("FORGE HANDSHAKE: PASS")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ProtocolError as e:
        print(f"FORGE HANDSHAKE: FAIL ({e})")
        sys.exit(1)
    except (EOFError, socket.timeout, ConnectionError) as e:
        print(f"FORGE HANDSHAKE: FAIL ({type(e).__name__}: {e})")
        sys.exit(1)
