# Forge (FML) login protocol — protocol 763 / MC 1.20.1

How `startech-rs` speaks Forge natively, with no JVM and no mods. Everything
here was transcribed from **Forge 47.4.23 sources**, not from memory; the
citations name the Java file and line the behaviour comes from. The
implementation is `src/forge.rs`.

Enable with `startech-forge-native=true` in `server.properties`. This is a
*different* thing from `startech-forge-enabled`, which supervises a real Forge
JVM and proxies players to it (see `src/manage.rs`).

## Sources

| Source | What it settled |
|---|---|
| `net/minecraftforge/network/NetworkConstants.java` | `FMLNETVERSION = 3`, `NETVERSION = "FML3"`, channel names |
| `net/minecraftforge/network/NetworkInitialization.java` | the message/discriminator table and which messages are login packets |
| `net/minecraftforge/network/LoginWrapper.java` | the `fml:loginwrapper` envelope |
| `net/minecraftforge/network/simple/IndexedMessageCodec.java` | the leading discriminator byte |
| `net/minecraftforge/network/HandshakeMessages.java` | every payload layout |
| `net/minecraftforge/network/HandshakeHandler.java` | the server-side state machine |
| `net/minecraftforge/network/NetworkRegistry.java` | channel set, gathering order, validation rules |
| `net/minecraftforge/network/ServerStatusPing.java` | the `forgeData` status blob |
| `patches/net/minecraft/server/network/ServerLoginPacketListenerImpl.java.patch` | *when* the handshake runs relative to `Login Success` |
| `patches/net/minecraft/network/protocol/status/ServerStatus.java.patch` | `forgeData` in the ping JSON |
| `patches/net/minecraft/network/protocol/login/ServerboundCustomQueryPacket.java.patch` | login query = login index |

Retrieved with:

```bash
curl -o f-src.jar https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.4.23/forge-1.20.1-47.4.23-sources.jar
unzip -q f-src.jar -d src
```

## 1. There is no configuration state in 763

The Configuration state arrived in 1.20.2 / protocol 764. On 763 the whole
modded handshake happens in the **login** state, riding the vanilla login
plugin request/response pair (`LoginPluginRequest` `0x04` serverbound,
`LoginPluginResponse` `0x02` serverbound — see `PROTOCOL-763.md` §5.3).

Forge patches `ServerLoginPacketListenerImpl` so that after `Login Start` the
connection parks in `NEGOTIATING` — a state vanilla never uses — and each tick
calls `NetworkHooks.tickNegotiation` → `HandshakeHandler.tickLogin`. Only when
that returns true is vanilla `Login Success` sent
(`ServerLoginPacketListenerImpl.java.patch:8-17,33`). `NetworkHooks.tickNegotiation`
is literally `return HandshakeHandler.tickLogin(networkManager)`.

Consequences for the packet order, all of which `src/net/connection.rs`
follows:

1. `Set Compression` `0x03` — vanilla, sent from the `Login Start` handler, so it
   is still the last uncompressed frame.
2. compression is enabled for both directions;
3. **the FML login queries** — already compressed;
4. `Login Success` `0x02`.

## 2. The envelope

Every FML login packet is a login query on the channel `fml:loginwrapper`
(`LoginWrapper.java:26`). Its payload is
`LoginWrapper.wrapPacket` + `IndexedMessageCodec.tryEncode`:

```text
LoginPluginRequest 0x04 (server → client)
  VarInt        transaction id  ← the "login index"
  Identifier    "fml:loginwrapper"
  Identifier    target channel, always "fml:handshake"
  VarInt        length of the next block
  bytes         u8 discriminator, then the message body
```

The reply mirrors it:

```text
LoginPluginResponse 0x02 (client → server)
  VarInt        transaction id, echoed
  Boolean       successful
  bytes         Identifier target, VarInt length, u8 discriminator, body
```

Two details that are easy to get wrong and are pinned by tests:

* **The discriminator is on the wire.** `tryEncode` writes
  `target.writeByte(codec.index & 0xff)` before the body (`IndexedMessageCodec.java:118-124`)
  and `consume` reads it back the same way (`:146`). The message body itself
  carries no index — `C2SAcknowledge.encode` writes literally nothing
  (`HandshakeMessages.java:245-247`).
* **The transaction id is the correlation mechanism.** It is the only
  request/response pairing the login state offers, so Forge sends its payloads
  with their position in the list as the id and tracks which replies it is still
  missing (`HandshakeHandler.java:58-76,341-384`).

## 3. Message table

Discriminators are the `int id` arguments to `messageBuilder` in
`NetworkInitialization.getHandshakeChannel()`:

| id | message | dir | login? | needs reply |
|----|---------|-----|--------|-------------|
| 1 | `S2CModList` | S→C | yes | yes — `C2SModListReply` |
| 2 | `C2SModListReply` | C→S | yes | — |
| 3 | `S2CRegistry` | S→C | yes | yes — ack |
| 4 | `S2CConfigData` | S→C | yes | yes — ack |
| 5 | `S2CModData` | S→C | yes | **no** (`noResponse()`) |
| 6 | `S2CChannelMismatchData` | S→C | yes | — |
| 99 | `C2SAcknowledge` | C→S | yes | — |

### Payloads

`S2CModData` (id 5) — `writeMap` over `modId → (displayName, version)`:

```text
VarInt   count
  count × { Utf(0x100) modId, Utf(0x100) displayName, Utf(0x100) version }
```

`S2CModList` (id 1):

```text
VarInt   mod count          → count × Utf(0x100) mod id
VarInt   channel count      → count × { Identifier, Utf(0x100) version }
VarInt   registry count     → count × Identifier      (Forge registries to sync)
VarInt   datapack reg count → count × Identifier      (synced datapack registries)
```

`C2SModListReply` (id 2) — mod ids, channel versions, then registry hashes:

```text
VarInt   mod count      → count × Utf(0x100)
VarInt   channel count  → count × { Identifier, Utf(0x100) }
VarInt   registry count → count × { Identifier, Utf(0x100) }
```

Forge itself sends that last map empty — *"Fill with known hashes, which requires
keeping a file cache"* (`HandshakeMessages.java:183`) — so we treat it as
optional and read nothing from it.

## 4. The sequence this server runs

`gatherLoginPayloads` walks `NetworkRegistry.instances` in insertion order and
each channel contributes its login packets in message-registration order
(`NetworkRegistry.java`, `NetworkInitialization.java`). With no mods there are
no Forge registries to sync and no config files to push, so the list collapses
to two payloads:

| pos | payload | id | reply expected? |
|-----|---------|----|-----------------|
| 0 | `S2CModData`, empty map | 5 | no (`noResponse()`) |
| 1 | `S2CModList`, 0 mods, 5 channels | 1 | yes |

`S2CModData` really does come *first*: its builder is registered before
`S2CModList`'s (`NetworkInitialization.java:32` then `:41`), which is why the
Java comment on `S2CModList` reads *"Always first handshake message after the
data sent by S2CModData"* (`HandshakeMessages.java:51`).

The client replies to pos 1 with `C2SModListReply` and deliberately leaves pos 0
unanswered; Forge logs a reply to an unneeded index but does not disconnect
(`HandshakeHandler.java:228-235`). `src/forge.rs` matches that.

### Channels

`buildChannelVersions()` is just the registered instances mapped to their
protocol version, and Forge registers exactly five:

| channel | where |
|---|---|
| `fml:handshake` | `NetworkConstants.java:39,43` |
| `fml:play` | `NetworkConstants.java:40,44` |
| `minecraft:register` | `NetworkConstants.java:41,45` |
| `minecraft:unregister` | `NetworkConstants.java:42,45` |
| `fml:loginwrapper` | `LoginWrapper.java:30` |

Getting this set wrong is fatal in both directions. `validateChannels` iterates
**our own** instances and disconnects the peer if any of them is missing from
the client's list (`HandshakeHandler.java:240-249`); the client applies the
mirror-image test to whatever we advertise. So `src/forge.rs::missing_channels`
reproduces the server-side rule and the client simulator asserts all five are
present and say `FML3`.

### Vanilla clients

A vanilla client does not know `fml:loginwrapper` and answers any login query
with `successful = false`. `src/forge.rs::negotiate` treats that as
`Peer::Vanilla`, stops negotiating and lets the login finish — Forge's own
`ConnectionType.VANILLA` path. Without this, turning the flag on would break
every vanilla join.

## 5. Play phase

Nothing extra is required for a modless server. Forge's two `fml:play` messages
(`SpawnEntity`, `OpenContainer`) are driven by mods. The one visible change: a
Forge peer is sent the brand `forge` instead of `startech-rs`, because a Forge
client uses the brand to decide it is talking to a modded server.

## 6. Server list ping

Forge's patched `ServerStatus` adds an optional top-level `forgeData` object
(`ServerStatus.java.patch:11`) decoded by `ServerStatusPing.CODOC`, which reads
`fmlNetworkVersion`, an *optional* compact binary blob `d`, and keeps
`channels`/`mods` as empty legacy placeholders (`ServerStatusPing.java:60-94`).
With no mods the legacy arrays stay empty and `d` is omitted, and a Forge
client still parses the ping and reports an FML3 server:

```json
"forgeData": { "fmlNetworkVersion": 3, "channels": [], "mods": [], "truncated": false }
```

## 7. Not implemented

| Forge feature | status | why |
|---|---|---|
| mods (`S2CModList.mods` non-empty) | no | a mod is JVM bytecode against obfuscated Minecraft classes; running one needs a JVM plus a remapper, not a protocol implementation |
| Forge registry sync (`S2CRegistry`, id 3) | no | only meaningful when mods add entries; the vanilla registry codec already goes out in `Login (play)` field 7 |
| config sync (`S2CConfigData`, id 4) | no | no mods, no configs |
| `minecraft:register` play channel | no | it advertises dynamic modded channels, of which there are none |
| NeoForge / 1.20.2+ (764) | no | different protocol; the configuration state moves the handshake |

Loading real Forge mods is a different project — that is what the
host/guest bridge in `/home/infinite/Projects/Module_mod` is for.

### Verified against a real Forge client

A heavily modded Forge 1.20.1 client (Star Technology, 155 mods) was pointed at
this server. It **rejected the connection**, and the client log confirms the
handshake itself is correct:

```text
[Render thread/INFO] [ConnectScreen]: Connecting to 127.0.0.1, 25599
[ERROR] [NetworkRegistry/NETREGISTRY]: Channels [...] rejected vanilla connections
[ERROR] [HandshakeHandler/FMLHANDSHAKE]: Terminating connection with server, mismatched mod list
[INFO] [ForgeHooksClient]: Client has mods that are missing on server: [gtceu, create, ae2, ...]
```

There are **zero** protocol parse errors in that log — no `invalid
discriminator`, no `No channel named`, no `Received invalid message`. Forge
decoded the envelope, ran the mod comparison, and refused the *content*: the
server advertises zero mods against the client's 155.

This is Forge working as designed, not a defect. `validateChannels` treats a
channel as required unless the mod explicitly accepts `ABSENT`, so a modded
client cannot join a server that does not run its mods. No change to this
server's bytes can fix that; the client would also then expect the registries,
recipes and blocks those mods register.

**So: a vanilla 1.20.1 client can join a native-forge server; a modded Forge
client cannot.** For the modded pack, use `startech-forge-enabled`, which
supervises a real Forge JVM that actually loads the mods.

When a peer hangs up without answering, the server now says exactly this
rather than the bare `connection closed`, because the bare form looks like a
protocol bug.

## 8. Verifying

```bash
cargo test                                    # codec + protocol-constant tests
cargo build --release

# in a runtime dir with eula.txt and a world, startech-forge-native=true
python3 tools/forge_client.py 127.0.0.1 25599          # expects PASS
python3 tools/forge_client.py 127.0.0.1 25599 --decline  # vanilla fallback
python3 tools/fake_client.py 127.0.0.1 25599            # vanilla join, must still PASS
python3 tools/status_ping.py 127.0.0.1 25599            # forgeData present
```

`tools/forge_client.py` is a from-the-spec Forge client, not a wrapper around
`src/forge.rs`, so agreement between the two is evidence about the wire format
rather than a tautology. It is still not a real Minecraft client: the
authoritative check is a Forge 1.20.1 client joining.
