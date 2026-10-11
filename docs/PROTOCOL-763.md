# Minecraft Java Edition wire protocol — protocol 763 (1.20.1)

Exact reference for implementing a **minimal offline-mode server** for Minecraft Java
Edition **1.20.1**, protocol number **763**. 1.20 and 1.20.1 share protocol 763
(identical wire protocol; 1.20.1 was a server-crash fix only). 1.20.2 is protocol 764
and is **not** compatible (it introduces the Configuration state and rewrites NBT).

Everything below is for **763 specifically**. Where a newer wiki page describes a
different format (it will be called out), trust this document.

## Sources

| Source | What it was used for |
|---|---|
| PrismarineJS/minecraft-data `data/pc/1.20/protocol.json` (shared by 1.20/1.20.1) | Packet ID maps + field layouts (authoritative ID source #1) |
| wiki.vg `Protocol` page, Wayback snapshot 2023-07-18 (1.20.1 era) | Field layouts, names, framing, registry codec prose |
| wiki.vg `Chunk Format`, Wayback snapshot 2023-06-29 | Chunk section / paletted container / heightmap rules for the 1.20.1 era |
| wiki.vg `Server List Ping`, Wayback snapshot 2023-07-18 | Status JSON shape |
| MCProtocolLib tag `1.20-1` (`MinecraftCodec`, `protocolVersion(763)`) | Independent ID + layout cross-check |
| valence (`valence-rs/valence`, `PROTOCOL_VERSION = 763`, `MINECRAFT_VERSION = "1.20.1"`) | Third cross-check: `valence_generated/extracted/packets.json` (extracted from the vanilla jar), `registry_codec.dat` (vanilla-extracted registry codec), paletted-container encoder |
| oxcraft (`Implodent/oxcraft`, Rust server for 763) | Concrete join-sequence reference |

IDs were cross-checked between minecraft-data, MCProtocolLib and valence's
vanilla-extracted `packets.json`. **All three agree on every packet ID in this
document.** Discrepancies found in other places are listed in
[§11 Discrepancies](#11-discrepancies--known-gaps).

---

## 1. Connection states and transitions

```
                  ┌──────────────┐
   TCP connect →  │ Handshaking  │  (initial state of every connection)
                  └──────┬───────┘
        Handshake 0x00   │
        nextState=1 ─────┼──────────→ Status ──(ping done / disconnect)──→ closed
        nextState=2 ─────┼──────────→ Login
                         │              │
                         │              │ Login Success 0x02 sent & received
                         │              ▼
                         │            Play  (until TCP close)
```

* `Handshaking` is entered on TCP connect. Exactly one packet is valid here:
  serverbound `Handshake` (0x00). Its `Next State` field selects `Status` (1) or
  `Login` (2). The state switch happens **after** that packet is processed.
* `Status` — server list ping only. Either side may close after `Pong Response`.
  The status connection must not transition anywhere else.
* `Login` — authentication/compression negotiation. Ends when `Login Success`
  (0x02, clientbound) has been sent/received; **both** sides then switch to `Play`.
  Per wiki.vg: the Notchian client sends **no** play-state packets until it
  receives `Login (play)` (0x28) from the server, so a slow server is fine.
* `Play` — game state. There is no way back to Login/Status on the same
  connection. Disconnect with `Disconnect (play)` (0x1A) or just close TCP.
* Protocol 763 has **no** Configuration state (that is 1.20.2+/764) and **no**
  `Login Acknowledged` packet. Do not implement either.

Offline mode (`online-mode=false`): skip `Encryption Request`/`Encryption
Response` entirely. Everything else is unchanged.

---

## 2. Data types

All multi-byte values are **big-endian**, except VarInt/VarLong which are
little-endian 7-bit groups.

| Type | Wire size | Encoding |
|---|---|---|
| `Boolean` | 1 | `0x00` = false, `0x01` = true (any non-zero = true) |
| `Byte` / `Unsigned Byte` | 1 | i8 / u8 |
| `Short` / `Unsigned Short` | 2 | i16 / u16 big-endian |
| `Int` | 4 | i32 big-endian |
| `Long` | 8 | i64 big-endian |
| `Float` | 4 | IEEE-754 binary32 big-endian |
| `Double` | 8 | IEEE-754 binary64 big-endian |
| `String (n)` | 1..(4n+3) | VarInt byte-length (not char count) + UTF-8. Max **n chars** and **4n bytes**; max n = 32767 |
| `Chat` | ≤ 262144·4+3 | `String (262144)` — a JSON chat component |
| `Identifier` | ≤ 32767 | `String (32767)`; `namespace:value`, default namespace `minecraft` |
| `VarInt` | 1..5 | LEB128-style, signed 32-bit, **no** zigzag; negative = 5 bytes |
| `VarLong` | 1..10 | same, 64-bit |
| `UUID` | 16 | two u64 big-endian: **most significant 64 bits first**, then least significant |
| `Position` | 8 | packed i64: `(x & 0x3FFFFFF) << 38 \| (z & 0x3FFFFFF) << 12 \| (y & 0xFFF)` — x: 26 bits, z: 26 bits, y: 12 bits, all signed two's complement (x/z ∈ ±33 554 432, y ∈ ±2048) |
| `Angle` | 1 | u8, rotation in 1/256 of a full turn (used by e.g. Set Head Rotation; *not* used by Set Default Spawn Position, which uses `Float`) |
| `NBT` | varies | **Standard NBT** for 763: root is `TAG_Compound` (0x0A) + root name (u16 length + UTF-8, normally empty `""`) + payload. Do **not** use the 1.20.2+ "anonymous NBT" (no root name). Big-endian. |
| `BitSet` | varies | VarInt number of longs + that many i64 (`BitSet.toLongArray()` layout: bit n is `longs[n/64] & (1 << (n%64))`) |
| `Optional X` | 0 or X | presence decided by an explicit `Boolean` in the packet (763 never infers it silently except where noted) |
| `Array of X` | varies | VarInt count + count × X, unless the packet says otherwise |
| `Byte Array` | varies | length given by a preceding VarInt ("Size"/"Length") or by remaining packet length ("rest") |

VarInt reference values: `0` → `00`; `1` → `01`; `127` → `7f`; `128` → `80 01`;
`255` → `ff 01`; `25565` → `dd c7 01`; `-1` → `ff ff ff ff 0f`.

---

## 3. Packet framing

A packet on the wire is always:

```
[VarInt length] [payload of `length` bytes]
```

`length` counts the packet ID **plus** data. Max packet size is
2²¹−1 = **2 097 151** bytes (max 3-byte VarInt).

### 3.1 Without compression (before Set Compression, or threshold < 0)

```
Length   : VarInt        = len(Packet ID) + len(Data)
Packet ID: VarInt
Data     : Byte Array
```

### 3.2 With compression (after Set Compression with threshold ≥ 0)

From the next packet on, the frame becomes:

```
Packet Length  : VarInt   = len(Data Length) + len(zlib(Packet ID + Data))
Data Length    : VarInt   = uncompressed size of (Packet ID + Data), or 0
Packet ID+Data : Byte Array, zlib (RFC 1950 / DEFLATE stream) compressed IFF Data Length != 0
```

Rules (exact):

* `Data Length == 0` → the ID+data block that follows is **not** compressed
  (raw bytes), and the compressed and uncompressed sizes are equal.
  This is how small packets are sent: "uncompressed with an extra 0".
* `Data Length > 0` → the block is a zlib stream that must decompress to
  **exactly** `Data Length` bytes; `Data Length` is the size of
  `VarInt packet id + data`, not of the data alone.
* Threshold `T` semantics (the field of `Set Compression`):
  * if `len(VarInt id + data) < T` → send with `Data Length = 0` (uncompressed);
  * else → compress and set `Data Length = len(id + data)`.
  * `T = 0` → **every** packet is compressed (nothing is `< 0`).
  * `T = 256` → packets whose uncompressed size is 256 bytes or more are
    compressed; smaller ones are sent raw. (256 is what this project uses.)
  * Negative `T` → compression is *disabled*: keep the plain framing of §3.1
    (vanilla never sends Set Compression at all when compression is off; either
    is acceptable, but the Notchian client treats a negative threshold as "no
    compression").
* `Set Compression` itself is sent in the **uncompressed** format (it is the last
  uncompressed packet). Compression applies to both directions from then on.
* Both compressed and uncompressed lengths must be ≤ 2 097 151.

---

## 4. Packet ID tables (protocol 763)

Names in bold are the wiki.vg (canonical) names; the last column is the
minecraft-data alias (useful when reading prismarine data). IDs are identical in
MCProtocolLib (`ClientboundPackets1_20_1`/`ServerboundPackets1_20_1` style
registries) and valence's vanilla-extracted `packets.json`.

### 4.1 Handshaking

| ID | Bound | Packet | minecraft-data |
|---|---|---|---|
| `0x00` (0) | S→C | **Handshake** | `set_protocol` |
| `0xFE` (254) | S→C | **Legacy Server List Ping** (1.6 ping; optional) | `legacy_server_list_ping` |

There are no clientbound packets in Handshaking.

### 4.2 Status

| ID | Bound | Packet | minecraft-data |
|---|---|---|---|
| `0x00` (0) | C→S | **Status Request** | `ping_start` |
| `0x01` (1) | C→S | **Ping Request** | `ping` |
| `0x00` (0) | S→C | **Status Response** | `server_info` |
| `0x01` (1) | S→C | **Ping Response** | `ping` |

### 4.3 Login

| ID | Bound | Packet | minecraft-data |
|---|---|---|---|
| `0x00` (0) | C→S | **Login Start** | `login_start` |
| `0x01` (1) | C→S | **Encryption Response** (skip in offline mode) | `encryption_begin` |
| `0x02` (2) | C→S | **Login Plugin Response** | `login_plugin_response` |
| `0x00` (0) | S→C | **Disconnect (login)** | `disconnect` |
| `0x01` (1) | S→C | **Encryption Request** (skip in offline mode) | `encryption_begin` |
| `0x02` (2) | S→C | **Login Success** | `success` |
| `0x03` (3) | S→C | **Set Compression** | `compress` |
| `0x04` (4) | S→C | **Login Plugin Request** | `login_plugin_request` |

### 4.4 Play — clientbound (S→C), 111 packets

| ID | Dec | Packet | minecraft-data |
|---|---|---|---|
| `0x00` | 0 | Bundle Delimiter | `bundle_delimiter` |
| `0x01` | 1 | Spawn Entity | `spawn_entity` |
| `0x02` | 2 | Spawn Experience Orb | `spawn_entity_experience_orb` |
| `0x03` | 3 | Spawn Player | `named_entity_spawn` |
| `0x04` | 4 | Entity Animation | `animation` |
| `0x05` | 5 | Award Statistics | `statistics` |
| `0x06` | 6 | Acknowledge Block Change | `acknowledge_player_digging` |
| `0x07` | 7 | Set Block Destroy Stage | `block_break_animation` |
| `0x08` | 8 | Block Entity Data | `tile_entity_data` |
| `0x09` | 9 | Block Action | `block_action` |
| `0x0A` | 10 | Block Update | `block_change` |
| `0x0B` | 11 | Boss Bar | `boss_bar` |
| `0x0C` | 12 | Change Difficulty | `difficulty` |
| `0x0D` | 13 | Chunk Biomes | `chunk_biomes` |
| `0x0E` | 14 | Clear Titles | `clear_titles` |
| `0x0F` | 15 | Command Suggestions Response | `tab_complete` |
| `0x10` | 16 | Commands | `declare_commands` |
| `0x11` | 17 | Close Container | `close_window` |
| `0x12` | 18 | Set Container Content | `window_items` |
| `0x13` | 19 | Set Container Property | `craft_progress_bar` |
| `0x14` | 20 | Set Container Slot | `set_slot` |
| `0x15` | 21 | Set Cooldown | `set_cooldown` |
| `0x16` | 22 | Chat Suggestions | `chat_suggestions` |
| `0x17` | 23 | **Plugin Message** | `custom_payload` |
| `0x18` | 24 | Damage Event | `damage_event` |
| `0x19` | 25 | Delete Message | `hide_message` |
| `0x1A` | 26 | **Disconnect (play)** | `kick_disconnect` |
| `0x1B` | 27 | Disguised Chat Message | `profileless_chat` |
| `0x1C` | 28 | Entity Event | `entity_status` |
| `0x1D` | 29 | Explosion | `explosion` |
| `0x1E` | 30 | Unload Chunk | `unload_chunk` |
| `0x1F` | 31 | **Game Event** | `game_state_change` |
| `0x20` | 32 | Open Horse Screen | `open_horse_window` |
| `0x21` | 33 | Hurt Animation | `hurt_animation` |
| `0x22` | 34 | Initialize World Border | `initialize_world_border` |
| `0x23` | 35 | **Keep Alive** | `keep_alive` |
| `0x24` | 36 | **Chunk Data and Update Light** ("Level Chunk With Light") | `map_chunk` |
| `0x25` | 37 | World Event | `world_event` |
| `0x26` | 38 | Particle | `world_particles` |
| `0x27` | 39 | Update Light | `update_light` |
| `0x28` | 40 | **Login (play)** ("Join Game") | `login` |
| `0x29` | 41 | Map Data | `map` |
| `0x2A` | 42 | Merchant Offers | `trade_list` |
| `0x2B` | 43 | Update Entity Position | `rel_entity_move` |
| `0x2C` | 44 | Update Entity Position and Rotation | `entity_move_look` |
| `0x2D` | 45 | Update Entity Rotation | `entity_look` |
| `0x2E` | 46 | Move Vehicle | `vehicle_move` |
| `0x2F` | 47 | Open Book | `open_book` |
| `0x30` | 48 | Open Screen | `open_window` |
| `0x31` | 49 | Open Sign Editor | `open_sign_entity` |
| `0x32` | 50 | Ping (play) | `ping` |
| `0x33` | 51 | Place Ghost Recipe | `craft_recipe_response` |
| `0x34` | 52 | Player Abilities | `abilities` |
| `0x35` | 53 | Player Chat Message | `player_chat` |
| `0x36` | 54 | End Combat | `end_combat_event` |
| `0x37` | 55 | Enter Combat | `enter_combat_event` |
| `0x38` | 56 | Combat Death | `death_combat_event` |
| `0x39` | 57 | Player Info Remove | `player_remove` |
| `0x3A` | 58 | Player Info Update | `player_info` |
| `0x3B` | 59 | Look At | `face_player` |
| `0x3C` | 60 | **Synchronize Player Position** | `position` |
| `0x3D` | 61 | Update Recipe Book | `unlock_recipes` |
| `0x3E` | 62 | Remove Entities | `entity_destroy` |
| `0x3F` | 63 | Remove Entity Effect | `remove_entity_effect` |
| `0x40` | 64 | Resource Pack | `resource_pack_send` |
| `0x41` | 65 | Respawn | `respawn` |
| `0x42` | 66 | Set Head Rotation | `entity_head_rotation` |
| `0x43` | 67 | Update Section Blocks | `multi_block_change` |
| `0x44` | 68 | Select Advancements Tab | `select_advancement_tab` |
| `0x45` | 69 | Server Data | `server_data` |
| `0x46` | 70 | Set Action Bar Text | `action_bar` |
| `0x47` | 71 | Set Border Center | `world_border_center` |
| `0x48` | 72 | Set Border Lerp Size | `world_border_lerp_size` |
| `0x49` | 73 | Set Border Size | `world_border_size` |
| `0x4A` | 74 | Set Border Warning Delay | `world_border_warning_delay` |
| `0x4B` | 75 | Set Border Warning Distance | `world_border_warning_reach` |
| `0x4C` | 76 | Set Camera | `camera` |
| `0x4D` | 77 | Set Held Item | `held_item_slot` |
| `0x4E` | 78 | **Set Center Chunk** | `update_view_position` |
| `0x4F` | 79 | **Set Render Distance** | `update_view_distance` |
| `0x50` | 80 | **Set Default Spawn Position** | `spawn_position` |
| `0x51` | 81 | Display Objective | `scoreboard_display_objective` |
| `0x52` | 82 | Set Entity Metadata | `entity_metadata` |
| `0x53` | 83 | Link Entities | `attach_entity` |
| `0x54` | 84 | Set Entity Velocity | `entity_velocity` |
| `0x55` | 85 | Set Equipment | `entity_equipment` |
| `0x56` | 86 | **Set Experience** | `experience` |
| `0x57` | 87 | **Set Health** | `update_health` |
| `0x58` | 88 | Update Objectives | `scoreboard_objective` |
| `0x59` | 89 | Set Passengers | `set_passengers` |
| `0x5A` | 90 | Update Teams | `teams` |
| `0x5B` | 91 | Update Score | `scoreboard_score` |
| `0x5C` | 92 | Set Simulation Distance | `simulation_distance` |
| `0x5D` | 93 | Set Subtitle Text | `set_title_subtitle` |
| `0x5E` | 94 | **Update Time** | `update_time` |
| `0x5F` | 95 | Set Title Text | `set_title_text` |
| `0x60` | 96 | Set Title Animation Times | `set_title_time` |
| `0x61` | 97 | Entity Sound Effect | `entity_sound_effect` |
| `0x62` | 98 | Sound Effect | `sound_effect` |
| `0x63` | 99 | Stop Sound | `stop_sound` |
| `0x64` | 100 | **System Chat Message** | `system_chat` |
| `0x65` | 101 | Set Tab List Header And Footer | `playerlist_header` |
| `0x66` | 102 | Tag Query Response | `nbt_query_response` |
| `0x67` | 103 | Pickup Item | `collect` |
| `0x68` | 104 | Teleport Entity | `entity_teleport` |
| `0x69` | 105 | Update Advancements | `advancements` |
| `0x6A` | 106 | Update Attributes | `entity_update_attributes` |
| `0x6B` | 107 | Feature Flags | `feature_flags` |
| `0x6C` | 108 | Entity Effect | `entity_effect` |
| `0x6D` | 109 | Update Recipes | `declare_recipes` |
| `0x6E` | 110 | Update Tags | `tags` |

### 4.5 Play — serverbound (C→S), 51 packets

| ID | Dec | Packet | minecraft-data |
|---|---|---|---|
| `0x00` | 0 | **Confirm Teleportation** | `teleport_confirm` |
| `0x01` | 1 | Query Block Entity Tag | `query_block_nbt` |
| `0x02` | 2 | Change Difficulty | `set_difficulty` |
| `0x03` | 3 | Message Acknowledgment | `message_acknowledgement` |
| `0x04` | 4 | **Chat Command** | `chat_command` |
| `0x05` | 5 | **Chat Message** | `chat_message` |
| `0x06` | 6 | Player Session | `chat_session_update` |
| `0x07` | 7 | Client Command | `client_command` |
| `0x08` | 8 | Client Information | `settings` |
| `0x09` | 9 | Command Suggestions Request | `tab_complete` |
| `0x0A` | 10 | Click Container Button | `enchant_item` |
| `0x0B` | 11 | Click Container | `window_click` |
| `0x0C` | 12 | Close Container | `close_window` |
| `0x0D` | 13 | **Plugin Message** | `custom_payload` |
| `0x0E` | 14 | Edit Book | `edit_book` |
| `0x0F` | 15 | Query Entity Tag | `query_entity_nbt` |
| `0x10` | 16 | Interact | `use_entity` |
| `0x11` | 17 | Jigsaw Generate | `generate_structure` |
| `0x12` | 18 | **Keep Alive** | `keep_alive` |
| `0x13` | 19 | Lock Difficulty | `lock_difficulty` |
| `0x14` | 20 | **Set Player Position** | `position` |
| `0x15` | 21 | **Set Player Position and Rotation** | `position_look` |
| `0x16` | 22 | Set Player Rotation | `look` |
| `0x17` | 23 | Set Player On Ground | `flying` |
| `0x18` | 24 | Move Vehicle | `vehicle_move` |
| `0x19` | 25 | Paddle Boat | `steer_boat` |
| `0x1A` | 26 | Pick Item | `pick_item` |
| `0x1B` | 27 | Place Recipe | `craft_recipe_request` |
| `0x1C` | 28 | Player Abilities | `abilities` |
| `0x1D` | 29 | Player Action | `block_dig` |
| `0x1E` | 30 | Player Command | `entity_action` |
| `0x1F` | 31 | Player Input | `steer_vehicle` |
| `0x20` | 32 | Pong (play) | `pong` |
| `0x21` | 33 | Change Recipe Book Settings | `recipe_book` |
| `0x22` | 34 | Set Seen Recipe | `displayed_recipe` |
| `0x23` | 35 | Rename Item | `name_item` |
| `0x24` | 36 | Resource Pack | `resource_pack_receive` |
| `0x25` | 37 | Seen Advancements | `advancement_tab` |
| `0x26` | 38 | Select Trade | `select_trade` |
| `0x27` | 39 | Set Beacon Effect | `set_beacon_effect` |
| `0x28` | 40 | Set Held Item | `held_item_slot` |
| `0x29` | 41 | Program Command Block | `update_command_block` |
| `0x2A` | 42 | Program Command Block Minecart | `update_command_block_minecart` |
| `0x2B` | 43 | Set Creative Mode Slot | `set_creative_slot` |
| `0x2C` | 44 | Program Jigsaw Block | `update_jigsaw_block` |
| `0x2D` | 45 | Program Structure Block | `update_structure_block` |
| `0x2E` | 46 | Update Sign | `update_sign` |
| `0x2F` | 47 | Swing Arm | `arm_animation` |
| `0x30` | 48 | Teleport To Entity | `spectate` |
| `0x31` | 49 | Use Item On | `block_place` |
| `0x32` | 50 | Use Item | `use_item` |

---

## 5. Field-by-field layouts

Unless noted, `String` = `String (32767)`. "rest" = remaining bytes of the
packet (no length prefix).

### 5.1 Handshaking

**Handshake** `0x00` (C→S)

| Field | Type | Notes |
|---|---|---|
| Protocol Version | VarInt | **763** for 1.20.1 |
| Server Address | String (255) | hostname used to connect (SRV target if SRV was used) |
| Server Port | Unsigned Short | e.g. 25565 |
| Next State | VarInt | 1 = Status, 2 = Login (3 = transfer is 1.20.5+, not 763) |

### 5.2 Status

**Status Request** `0x00` (C→S) — no fields.

**Status Response** `0x00` (S→C)

| Field | Type |
|---|---|
| JSON Response | String (32767) — JSON text, see §8 |

**Ping Request** `0x01` (C→S): `Payload: Long`.
**Ping Response** `0x01` (S→C): `Payload: Long` (echo the same value).

### 5.3 Login

**Login Start** `0x00` (C→S)

| Field | Type | Notes |
|---|---|---|
| Name | String (16) | player username |
| Has Player UUID | Boolean | whether the next field is present |
| Player UUID | Optional UUID | only if `Has Player UUID` |

The Notchian client in offline mode still sends its offline UUID here; a server
may ignore it. The *standard* offline UUID (used by vanilla/Bukkit for
`online-mode=false`) is
`UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))` — an
MD5-based UUIDv3 over those exact bytes (no namespace prefix).

**Login Success** `0x02` (S→C) — 1.20.1 layout (UUID first; this is
post-1.19.3, do **not** use the old 1.19.2 layout)

| Field | Type | Notes |
|---|---|---|
| UUID | UUID | player UUID |
| Username | String (16) | |
| Number Of Properties | VarInt | usually 0 in offline mode |
| Property × N | Name: String (32767)<br>Value: String (32767)<br>Is Signed: Boolean<br>Signature: Optional String (32767) | skin/textures etc. (online mode) |

This packet switches the connection to **Play** on both sides.

**Set Compression** `0x03` (S→C)

| Field | Type | Notes |
|---|---|---|
| Threshold | VarInt | see §3.2; 256 recommended; negative disables |

**Disconnect (login)** `0x00` (S→C): `Reason: Chat` (JSON chat component string).

**Encryption Request** `0x01` (S→C) / **Encryption Response** `0x01` (C→S):
not used in offline mode. (Request: Server ID `String (20)` (empty),
Public Key `Byte Array` with VarInt length, Verify Token `Byte Array` with
VarInt length. Response: Shared Secret `Byte Array` (128), Verify Token
`Byte Array` (128), both RSA-encrypted.)

**Login Plugin Request** `0x04` (S→C)

| Field | Type |
|---|---|
| Message ID | VarInt (unique per connection) |
| Channel | Identifier |
| Data | rest (max 1048576) |

**Login Plugin Response** `0x02` (C→S)

| Field | Type | Notes |
|---|---|---|
| Message ID | VarInt | echo of the request |
| Successful | Boolean | false if the channel was not understood |
| Data | rest — only present if Successful | |

### 5.4 Play — the packets a minimal server needs

**Login (play) / "Join Game"** `0x28` (S→C) — full 763 layout, in order:

| # | Field | Type | Notes |
|---|---|---|---|
| 1 | Entity ID | Int | the player's EID |
| 2 | Is hardcore | Boolean | |
| 3 | Game mode | Unsigned Byte | 0 survival, 1 creative, 2 adventure, 3 spectator |
| 4 | Previous Game mode | Byte | −1 = none, else 0..3 |
| 5 | Dimension Count | VarInt | size of the next array |
| 6 | Dimension Names | Array of Identifier | e.g. one entry `"minecraft:overworld"` (vanilla sends 3: overworld, the_end, the_nether) |
| 7 | Registry Codec | NBT | see §6 — unnamed root TAG_Compound |
| 8 | Dimension Type | Identifier | must be a key of `minecraft:dimension_type` in the codec, e.g. `"minecraft:overworld"` |
| 9 | Dimension Name | Identifier | the dimension being spawned into, e.g. `"minecraft:overworld"` |
| 10 | Hashed seed | Long | first 8 bytes of SHA-256 of the world seed (0 is fine) |
| 11 | Max Players | VarInt | ignored by the client |
| 12 | View Distance | VarInt | 2..32 |
| 13 | Simulation Distance | VarInt | |
| 14 | Reduced Debug Info | Boolean | false recommended |
| 15 | Enable respawn screen | Boolean | |
| 16 | Is Debug | Boolean | |
| 17 | Is Flat | Boolean | affects horizon/fog |
| 18 | Has death location | Boolean | if true: next two fields present |
| 19 | Death dimension name | Optional Identifier | |
| 20 | Death location | Optional Position | |
| 21 | Portal cooldown | VarInt | |

**Plugin Message** `0x17` (both directions)

| Field | Type | Notes |
|---|---|---|
| Channel | Identifier | e.g. `"minecraft:brand"` |
| Data | rest | brand: `String` (UTF-8 brand name) for `minecraft:brand` |

**Disconnect (play)** `0x1A` (S→C): `Reason: Chat`.

**Keep Alive** `0x23` (S→C) and `0x12` (C→S): `Keep Alive ID: Long` (both
directions use the same layout; the client echoes the ID). Send every 1–15 s;
the client times out after 20 s without one.

**Synchronize Player Position** `0x3C` (S→C)

| Field | Type | Notes |
|---|---|---|
| X | Double | absolute if flag not set |
| Y | Double | feet position |
| Z | Double | |
| Yaw | Float | degrees |
| Pitch | Float | degrees |
| Flags | Byte | bit field: 0x01 X, 0x02 Y, 0x04 Z, 0x08 Y_ROT, 0x10 X_ROT — if the bit is **set** the value is *relative*; use `0x00` for an absolute teleport |
| Teleport ID | VarInt | client must answer with Confirm Teleportation |

This packet also closes the client's "Downloading terrain" screen.

**Confirm Teleportation** `0x00` (C→S): `Teleport ID: VarInt`.

**Set Player Position** `0x14` (C→S)

| Field | Type |
|---|---|
| X | Double |
| Feet Y | Double (head Y − 1.62) |
| Z | Double |
| On Ground | Boolean |

**Set Player Position and Rotation** `0x15` (C→S)

| Field | Type |
|---|---|
| X | Double |
| Feet Y | Double |
| Z | Double |
| Yaw | Float |
| Pitch | Float |
| On Ground | Boolean |

**Set Center Chunk** `0x4E` (S→C): `Chunk X: VarInt`, `Chunk Z: VarInt`
(chunk coords of the player, `block >> 4`, arithmetic shift). Sent when the
player crosses a chunk border (and on integer Y change).

**Set Render Distance** `0x4F` (S→C): `View Distance: VarInt` (2..32).

**Set Default Spawn Position** `0x50` (S→C)

| Field | Type |
|---|---|
| Location | Position |
| Angle | Float (degrees; compass respawn angle) |

**Level Chunk With Light** (wiki.vg name "Chunk Data and Update Light") `0x24`
(S→C) — full layout in §7.

**System Chat Message** `0x64` (S→C)

| Field | Type | Notes |
|---|---|---|
| Content | Chat | JSON chat component (`String (262144)`) |
| Overlay | Boolean | true = action bar, false = chat box |

**Set Health** `0x57` (S→C)

| Field | Type | Notes |
|---|---|---|
| Health | Float | ≤0 dead, 20 = full |
| Food | VarInt | 0..20 |
| Food Saturation | Float | 0.0..5.0 |

**Set Experience** `0x56` (S→C) — wire order used by the Notchian client and by
minecraft-data / valence / MCProtocolLib:

| Field | Type | Notes |
|---|---|---|
| Experience bar | Float | 0.0..1.0 |
| Level | VarInt | |
| Total Experience | VarInt | |

(wiki.vg's 1.20.1 table lists "Total Experience" before "Level"; that is a
documentation error — see §11.)

**Update Time** `0x5E` (S→C)

| Field | Type | Notes |
|---|---|---|
| World Age | Long | ticks, monotonic |
| Time of day | Long | ticks mod 24000; negative freezes the sun at abs(value) |

**Game Event** `0x1F` (S→C)

| Field | Type | Notes |
|---|---|---|
| Event | Unsigned Byte | see table |
| Value | Float | meaning depends on Event |

Events valid in **763** (0..11 only — event 13 "start waiting for level chunks"
is 1.20.3+ and does not exist here):

| Event | Meaning | Value |
|---|---|---|
| 0 | No respawn block available | — |
| 1 | Begin raining | — |
| 2 | End raining | — |
| 3 | Change game mode | 0..3 |
| 4 | Win game | 0 respawn, 1 credits then respawn |
| 5 | Demo event | 0/101..104 |
| 6 | Arrow hit player | — |
| 7 | Rain level change | 0.0..1.0 |
| 8 | Thunder level change | 0.0..1.0 |
| 9 | Pufferfish sting sound | — |
| 10 | Elder guardian appearance | — |
| 11 | Enable respawn screen | 0 enable, 1 immediate respawn |

**Chat Command** `0x04` (C→S)

| Field | Type | Notes |
|---|---|---|
| Command | String (256) | without the leading `/` |
| Timestamp | Long | epoch ms |
| Salt | Long | |
| Argument count | VarInt | |
| Argument signature × N | Name: String<br>Signature: Byte Array (256) | empty array in offline mode |
| Message Count | VarInt | minecraft-data calls this `offset` |
| Acknowledged | BitSet(20) | **3 bytes fixed** (not a VarInt-prefixed BitSet) |

**Chat Message** `0x05` (C→S)

| Field | Type | Notes |
|---|---|---|
| Message | String (256) | |
| Timestamp | Long | |
| Salt | Long | |
| Has Signature | Boolean | **false** in offline mode |
| Signature | Optional Byte Array (256) | |
| Message Count | VarInt | `offset` in minecraft-data |
| Acknowledged | BitSet(20) | 3 bytes fixed |

For a minimal server, accept these and broadcast via `System Chat Message`
(0x64) to everyone — that avoids the signed `Player Chat Message` (0x35)
machinery entirely.

---

## 6. The Registry Codec (field 7 of Login (play))

Wire format: **standard NBT**, root is an unnamed (`""`) `TAG_Compound`
(`0x0A 0x00 0x00` + payload). Not the 1.20.2+ nameless NBT.

Structure (all keys are strings; `value` is a `TAG_List` of `TAG_Compound`):

```
TAG_Compound "" {
  "minecraft:<registry>" : TAG_Compound {
      "type"  : TAG_String = "minecraft:<registry>"     // the registry key again
      "value" : TAG_List<TAG_Compound> [
          { "name"    : TAG_String = "minecraft:entry_name"
            "id"      : TAG_Int    = <index in this list, 0-based, must match>
            "element" : TAG_Compound = <entry data> }
          ...
      ]
  }
  ... more registries ...
}
```

### 6.1 Registries present in 1.20.1

The vanilla 1.20/1.20.1 client expects **six** registry keys (this is the set
vanilla sends; verified against valence's vanilla-extracted
`registry_codec.dat`, minecraft-data's `loginPacket.json` capture, and go-mc's
`NetworkCodec` for 1.20.x). Counts are the vanilla sizes:

| Registry key | Vanilla entries | Needed for |
|---|---|---|
| `minecraft:dimension_type` | 4 (`overworld`, `overworld_caves`, `the_end`, `the_nether`) | field 8 (Dimension Type) lookup, world height/sections |
| `minecraft:worldgen/biome` | 64 | chunk biome palette global IDs |
| `minecraft:chat_type` | 7 | rendering chat (`chat`, `emote_command`, `msg_command_*`, `say_command`, `team_msg_command_*`) |
| `minecraft:damage_type` | 44 | damage/death messages |
| `minecraft:trim_pattern` | 16 | armor trim rendering |
| `minecraft:trim_material` | 10 | armor trim rendering |

The wiki.vg 1.20.1 page's prose still says "three registries
(dimension_type, worldgen/biome, chat_type)" — that text was written for
1.19.2 and is **stale** for 763; vanilla sends six. A practical minimal set is
all six keys present, with `dimension_type` containing the dimension you spawn
into, `worldgen/biome` containing every biome your chunk data references,
`chat_type` containing `minecraft:chat`, and the other three either
vanilla-complete or small stub sets (they are only consumed when the client
sees a damage message / a trimmed item). Missing a registry the client
actually dereferences = immediate disconnect or crash. Safest for
compatibility: copy valence's `registry_codec.dat` (6 registries, vanilla
contents).

### 6.2 `minecraft:dimension_type` element (exact 1.20.1 schema)

`TAG_Compound` with these fields (1.20.1 has **no** `cloud_height` — that is
1.21+; field order in the compound is irrelevant):

| Field | NBT type | Vanilla overworld |
|---|---|---|
| `piglin_safe` | Byte | 0 |
| `natural` | Byte | 1 |
| `ambient_light` | Float | 0.0 |
| `fixed_time` | Long (**optional**, omit for overworld) | (the_end: 6000) |
| `infiniburn` | String | `#minecraft:infiniburn_overworld` |
| `respawn_anchor_works` | Byte | 0 |
| `has_skylight` | Byte | 1 |
| `bed_works` | Byte | 1 |
| `effects` | String | `minecraft:overworld` / `minecraft:the_nether` / `minecraft:the_end` |
| `has_raids` | Byte | 1 |
| `logical_height` | Int | 384 |
| `coordinate_scale` | Double | 1.0 |
| `monster_spawn_light_level` | Int **or** Compound | Compound: `{type:"minecraft:uniform", value:{min_inclusive:0, max_inclusive:7}}` |
| `monster_spawn_block_light_limit` | Int | 0 |
| `min_y` | Int | −64 |
| `height` | Int | 384 (must be a multiple of 16; `min_y` too) |
| `ultrawarm` | Byte | 0 |
| `has_ceiling` | Byte | 0 |

`height` decides the chunk geometry: **number of chunk sections per column =
height / 16** (24 for overworld) and the heightmap bit width
(§7.2). `min_y` decides the section index offset and heightmap values.

### 6.3 `minecraft:worldgen/biome` element (1.20.1 schema)

Required fields (vanilla 1.20.1 biome JSON; the old `depth`/`scale`/`category`
fields are gone by 1.20.1):

| Field | NBT type | Example (plains) |
|---|---|---|
| `has_precipitation` | Byte | 1 |
| `temperature` | Float | 0.8 |
| `downfall` | Float | 0.4 |
| `effects` | Compound | see below |

`effects` compound:

| Field | NBT type | Notes |
|---|---|---|
| `sky_color` | Int | e.g. 7907327 |
| `fog_color` | Int | 12638463 |
| `water_color` | Int | 4159204 |
| `water_fog_color` | Int | 329011 |
| `foliage_color` | Int (optional) | |
| `grass_color` | Int (optional) | |
| `grass_color_modifier` | String (optional) | `swamp`, `dark_forest` |
| `mood_sound` | Compound (optional) | `{tick_delay:6000, offset:2.0, block_search_extent:8, sound:"minecraft:ambient.cave"}` |
| `additions_sound` | Compound (optional) | `{tick_chance: 0.05 (Double), sound: "..."}` |
| `music` | Compound (optional) | `{sound:"...", min_delay:12000 (Int), max_delay:24000 (Int), replace_current_music:0 (Byte)}` |
| `ambient_sound` | String (optional) | |
| `particle` | Compound (optional) | `{probability: Float, options: Compound{type: String}}` |

### 6.4 Other registry elements (compact)

`minecraft:chat_type` element: `{ chat: {translation_key: String, style: Compound?(chat style), parameters: List<String>}, narration: {translation_key: String, parameters: List<String>} }` — vanilla `minecraft:chat`:
`chat.type.text` / `chat.type.text.narrate`, parameters `["sender","content"]`.

`minecraft:damage_type` element: `{message_id: String, scaling: String
("never"|"when_caused_by_living_non_player"|"always"), exhaustion: Float,
effects: String? ("hurt"|"thirst"|"none"|"poison"), death_message_type: String?
("default"|"intentional_gameplay"|"fall")}`.

`minecraft:trim_pattern` element: `{asset_id: String, template_item: String,
description: Compound (chat component, e.g. {"translate":"trim_pattern.minecraft.coast"})}`.
`minecraft:trim_material` element: `{asset_name: String, ingredient: String,
item_model_index: Float, description: Compound (chat, may carry "color")}`.

### 6.5 Concrete minimal codec example

Minimal-but-real codec (3 registries; add the other three as
`{type: <key>, value: []}` empty lists only if you accept the risk, or ship the
vanilla sets). Shown as SNBT for readability — on the wire it is binary NBT
with root name `""`:

```snbt
{
  "minecraft:dimension_type": {
    type: "minecraft:dimension_type",
    value: [
      {
        name: "minecraft:overworld",
        id: 0,
        element: {
          piglin_safe: 0b, natural: 1b, ambient_light: 0.0f,
          infiniburn: "#minecraft:infiniburn_overworld",
          respawn_anchor_works: 0b, has_skylight: 1b, bed_works: 1b,
          effects: "minecraft:overworld", has_raids: 1b,
          logical_height: 384, coordinate_scale: 1.0d,
          monster_spawn_light_level: {
            type: "minecraft:uniform",
            value: { min_inclusive: 0, max_inclusive: 7 }
          },
          monster_spawn_block_light_limit: 0,
          min_y: -64, height: 384, ultrawarm: 0b, has_ceiling: 0b
        }
      }
    ]
  },
  "minecraft:worldgen/biome": {
    type: "minecraft:worldgen/biome",
    value: [
      {
        name: "minecraft:plains", id: 0,
        element: {
          has_precipitation: 1b, temperature: 0.8f, downfall: 0.4f,
          effects: {
            sky_color: 7907327, fog_color: 12638463,
            water_color: 4159204, water_fog_color: 329011,
            mood_sound: { tick_delay: 6000, offset: 2.0d,
                          block_search_extent: 8, sound: "minecraft:ambient.cave" }
          }
        }
      }
    ]
  },
  "minecraft:chat_type": {
    type: "minecraft:chat_type",
    value: [
      {
        name: "minecraft:chat", id: 0,
        element: {
          chat:      { translation_key: "chat.type.text",
                       parameters: ["sender", "content"] },
          narration: { translation_key: "chat.type.text.narrate",
                       parameters: ["sender", "content"] }
        }
      }
    ]
  }
}
```

Then `Dimension Type = "minecraft:overworld"`, `Dimension Name =
"minecraft:overworld"`, and chunk biomes use global biome ID `0` (= plains).
`id` must equal the entry's index in the list. The client validates:
dimension type exists and `height`/`min_y` are sane (multiple of 16,
`logical_height ≤ height`), biome IDs referenced by chunk data are < number of
biome entries, and (per implementation experience, not verified against Mojang
source) chat types exist when chat is displayed. **Gap:** the precise set of
registries/fields the vanilla 1.20.1 client hard-fails on vs. merely uses
lazily could not be confirmed from a primary source; shipping all six
registries with vanilla contents is the safe choice.

---

## 7. Level Chunk With Light — `0x24` (Chunk Data and Update Light)

### 7.1 Top-level layout (exact 763 order)

| # | Field | Type |
|---|---|---|
| 1 | Chunk X | Int (block X >> 4) |
| 2 | Chunk Z | Int |
| 3 | Heightmaps | NBT (unnamed TAG_Compound) |
| 4 | Size | VarInt — byte length of Data |
| 5 | Data | Byte Array (`Size` bytes) — sections, §7.3 |
| 6 | Number of block entities | VarInt |
| 7 | Block entities | array (§7.5) |
| 8 | Sky Light Mask | BitSet |
| 9 | Block Light Mask | BitSet |
| 10 | Empty Sky Light Mask | BitSet |
| 11 | Empty Block Light Mask | BitSet |
| 12 | Sky Light array count | VarInt |
| 13 | Sky Light arrays | count × (VarInt length (=2048) + 2048 bytes) |
| 14 | Block Light array count | VarInt |
| 15 | Block Light arrays | count × (VarInt length (=2048) + 2048 bytes) |

> **There is no "Trust Edges" boolean in 1.20.1.** That field belonged to
> Update Light in ≤1.17 and was removed in 1.18; the field list above goes
> straight from the block-entity array to the light masks. (Current wiki pages
> describe 1.21.x and must not be used for the section format.)

The Data section is **not** length-prefixed internally; its size in *sections*
is `dimension_type.height / 16` (24 for overworld), sent bottom-to-top.

### 7.2 Heightmaps

An unnamed `TAG_Compound` of `TAG_Long_Array`s (standard NBT). The client needs
`MOTION_BLOCKING`; `WORLD_SURFACE` is sent by vanilla but **not required** for
the chunk to be accepted. Missing heightmaps are tolerated (client
initializes them to minimums).

For a 1.20.1 overworld (`min_y = -64`, `height = 384`):

* bits per entry = `ceil(log2(height + 1))` = **9**;
* 256 entries (16×16 columns), packed like the paletted data array
  (§7.4): 7 entries per long (63 used bits, 1 padding bit at the top),
  **37 longs** total (36 full + 1 with 4 entries, 3 unused);
* values are `y - min_y` (0 = empty column, 384 = world top), ordered with x
  fastest, then z.

### 7.3 Data: chunk sections

For each of the `height/16` sections, bottom to top, with **no** count prefix:

| Field | Type | Notes |
|---|---|---|
| Block count | Short | number of non-air blocks (any block other than air/cave_air/void_air). If this hits 0 the client stops rendering the section — send accurate counts |
| Block states | Paletted Container | 4096 entries (16³), x fastest, then z, then y |
| Biomes | Paletted Container | 64 entries (4³), x fastest, then z, then y |

(1.20.1 has **no** "fluid count" field — that is a later addition.)

### 7.4 Paletted Container (both block states and biomes)

| Field | Type | Notes |
|---|---|---|
| Bits Per Entry | Unsigned Byte | see rules below |
| Palette | depends on BPE | see below |
| Data Array Length | VarInt | **number of longs** — present in 763 (removed only in 1.21.5+) |
| Data Array | Array of Long (i64 BE) | packed indices/global IDs; empty when BPE = 0 |

Palette forms:

* **Single-valued** — `BPE = 0`: one `VarInt` (global block-state ID or biome
  ID), then `Data Array Length = 0`.
* **Indirect** — `Palette Length: VarInt` + that many `VarInt` global IDs, then
  the data array of local indices.
* **Direct** — no palette fields at all; the data array holds global IDs.

BPE rules for 1.20.1 (vanilla; verified against valence's encoder and the
2023 wiki.vg page):

| Container | Single | Indirect | Direct |
|---|---|---|---|
| Block states | 1 unique → BPE 0 | 2..256 unique → BPE **4** (2–16 unique), 5 (17–32), 6 (33–64), 7 (65–128), 8 (129–256) | >256 unique → BPE **15** |
| Biomes | 1 unique → BPE 0 | 2..8 unique → BPE **1** (2), 2 (3–4), 3 (5–8) | >8 unique → BPE **6** |

In other words: the indirect palette tops out at **8 bits / 256 entries for
block states** and **3 bits / 8 entries for biomes**; beyond that you must use
the direct (global) form. Note the round-up: block-state BPE is never below 4
in indirect form.

Direct bit widths (hard facts for 1.20.1):

* Block states: 1.20/1.20.1 has **24 135 block states** (IDs 0..24134;
  minecraft-data `blocks.json` and valence's vanilla-extracted `blocks.json`
  agree) → `ceil(log2(24135))` = **15 bits**.
* Biomes: vanilla has **64 biome entries** (IDs 0..63) → `ceil(log2(64))` =
  **6 bits**. If your registry codec has a different number of biomes, the
  width is `ceil(log2(entry_count))` (the client computes it from *your*
  registry).

Data array packing (**1.16+ format**, unchanged in 1.20.1):

* `entries_per_long = floor(64 / bits_per_entry)` (15 bpp → 4 per long;
  9 bpp → 7 per long; 4 bpp → 16 per long);
* `long_count = ceil(entry_count / entries_per_long)` — equals the
  `Data Array Length` you must write;
* entries are packed starting at bit 0 (least significant) of each long,
  never spanning longs; the leftover high bits of each long are padding;
* longs are sent big-endian;
* index math: `long_index = i / entries_per_long`,
  `bit_index = (i % entries_per_long) * bits_per_entry`,
  `value = (longs[long_index] >> bit_index) & ((1 << bits_per_entry) - 1)`.

Worked example (all-air section): `Block count = 0x0000`,
block states `BPE=0, palette=VarInt 0 (air), longs=0`, biomes
`BPE=0, palette=VarInt 0, longs=0` → 2 + 3 + 3 = 8 bytes per section.

### 7.5 Block entities

`Number of block entities: VarInt`, then per entity:

| Field | Type | Notes |
|---|---|---|
| Packed XZ | Byte | `((blockX & 15) << 4) \| (blockZ & 15)` |
| Y | Short | absolute Y |
| Type | VarInt | block-entity-type registry ID (vanilla 1.20.1 has 41 types, IDs 0..40; safest is to send 0 block entities) |
| Data | NBT | without X/Y/Z fields; unnamed root compound |

### 7.6 Light data

The four masks are `BitSet`s = `VarInt long-count` + longs. They cover
**`sections + 2`** sections (26 bits for overworld): bit 0 is the section just
below `min_y`, bits 1..24 the world sections, bit 25 the section just above
`min_y + height`. Bit *i* set in "X Mask" means the i-th light array below
belongs to that section.

* **Sky Light Mask / Block Light Mask** — sections for which real light data
  follows.
* **Empty Sky Light Mask / Empty Block Light Mask** — sections known to be all
  zeros. A bit must not be set in both a data mask and its empty mask; leaving
  it unset in both means "no update".

Then, for each set bit of the sky mask (lowest first): `VarInt 2048` + 2048
bytes; same for block light. One array = one 16×16×16 section, one nibble per
block: byte index `= ((y << 8) | (z << 4) | x) / 2`; odd indices use the high
nibble (`& 0xF0`), even indices the low nibble (`& 0x0F`).

Minimal options:

* **Pitch black / "no lighting work":** all four mask long-counts = 0, both
  array counts = 0. Valid; the client treats the chunk as unlit.
* **Full skylight:** for each of the 26 section slots set the bit in
  `Sky Light Mask`, and send 26 arrays of 2048 × `0xFF` (light 15 everywhere);
  block light empty masks all set (or all data zero arrays). For a
  superflat-like world only the slots that actually contain sky need data.

---

## 8. Status response JSON (1.20.1)

`Status Response` carries a `String (32767)` of JSON (no length beyond the
string prefix):

```json
{
  "version": {
    "name": "1.20.1",
    "protocol": 763
  },
  "players": {
    "max": 20,
    "online": 1,
    "sample": [ { "name": "Notch", "id": "4566e69f-c907-48ee-8d71-d7ba5aa00d20" } ]
  },
  "description": { "text": "Hello world" },
  "favicon": "data:image/png;base64,<base64 of a 64x64 PNG>",
  "enforcesSecureChat": false,
  "previewsChat": false
}
```

* `version.name` must be a string; use `"1.20.1"` and `version.protocol: 763`.
* `players.sample[].id` must be a **valid UUID** or the client aborts.
* `description` is a chat component (legacy `§`-in-text is accepted in `text`);
  full JSON components also fine.
* `favicon` optional; must be `data:image/png;base64,` + base64 (no newlines)
  of an exactly 64×64 PNG or it is ignored.
* `enforcesSecureChat` (Boolean) and `previewsChat` (Boolean) exist in 1.20.1;
  set `enforcesSecureChat: false` for offline mode.
* `players.sample` may be omitted when online = 0.

---

## 9. Minimal offline join sequence (threshold 256)

Exact packet order for a vanilla 1.20.1 client, offline mode, no encryption.
`C` = client, `S` = server.

```
 0. TCP connect (C → S)

 ── Handshaking ─────────────────────────────────────────────
 1. C→S  Handshake        0x00  {protocolVersion=763, addr, port, nextState=2}
         (both sides switch to Login)

 ── Login ───────────────────────────────────────────────────
 2. C→S  Login Start      0x00  {name, hasUUID=true, uuid}
 3. S→C  Set Compression  0x03  {threshold=256}          ← last uncompressed frame
         (both sides now use the compressed framing of §3.2)
 4. S→C  Login Success    0x02  {uuid, name, properties=0}
         (both sides switch to Play; client sends NOTHING until 0x28 arrives)

 ── Play (S→C first) ────────────────────────────────────────
 5. S→C  Login (play)     0x28  {entityId, gamemode, dimNames=["minecraft:overworld"],
                                 registryCodec (§6), dimensionType="minecraft:overworld",
                                 dimensionName="minecraft:overworld", hashedSeed=0,
                                 maxPlayers, viewDistance, simulationDistance,
                                 reducedDebugInfo=false, enableRespawnScreen=true,
                                 isDebug=false, isFlat=<world>,
                                 hasDeathLocation=false, portalCooldown=0}
 6. S→C  Feature Flags    0x6B  {["minecraft:vanilla"]}          (optional; vanilla sends it)
 7. S→C  Plugin Message   0x17  {"minecraft:brand", <String brand>}   (optional)
 8. S→C  Change Difficulty 0x0C {difficulty, locked}              (optional)
 9. S→C  Player Abilities  0x34 {flags, 0.05f, 0.1f}              (optional)
10. S→C  Set Health        0x57 {20.0f, 20, 5.0f}                 (recommended)
11. S→C  Set Experience    0x56 {0.0f, level=0, total=0}          (optional)
12. S→C  Update Time       0x5E {worldAge, timeOfDay}             (optional)
13. S→C  Set Default Spawn 0x50 {Position spawn, angle=0.0f}      (optional but usual)
14. S→C  Set Center Chunk  0x4E {chunkX, chunkZ}                  (player's chunk)
15. S→C  Set Render Dist.  0x4F {viewDistance}                    (optional)
16. S→C  Chunk Data+Light  0x24  × N  (send a grid of chunks around the player,
                                       e.g. 3×3; chunks without neighbors are not
                                       rendered by the vanilla client)
17. S→C  Sync Player Pos   0x3C {x, y, z, yaw, pitch, flags=0x00, teleportId=1}
                                 ← closes "Downloading terrain"; use absolute values

 ── Play (C→S) ──────────────────────────────────────────────
18. C→S  Confirm Teleport 0x00  {teleportId=1}
19. C→S  Plugin Message   0x0D  {"minecraft:brand", ...}          (may arrive any time)
20. C→S  Client Information 0x08 {...}                            (may arrive any time)
21. C→S  Set Player Pos / Pos+Rot  0x14 / 0x15  (movement, ~every tick)

 ── steady state ────────────────────────────────────────────
22. S→C  Keep Alive 0x23 {id} every 1–15 s  ↔  C→S Keep Alive 0x12 {same id}
        (client times out after 20 s without a keep-alive)
```

Checklist / pitfalls:

* `Login (play)` **must** be the first Play packet. The client won't speak Play
  before it.
* The player must end up standing in a chunk that has been sent (16); send a
  few surrounding chunks or the vanilla client may render nothing.
* `Synchronize Player Position` with `flags = 0x00` and a fresh `teleportId`
  is what actually spawns the player; wait for `Confirm Teleportation` with the
  same ID before trusting client position.
* Game Event 13 ("start waiting for level chunks") **does not exist** in 763 —
  it is 1.20.3+. In 1.20.1 the position packet + chunks are sufficient.
* `Player Info Update` (0x3A, add-player action) for the joining player is what
  fills the tab list and gives the client its own skin; not strictly required
  to stand in a chunk, but vanilla sends it and you should too.
* Keep-alive IDs are opaque (vanilla uses millis); just echo.
* After login, everything must use the compressed framing if you sent
  Set Compression.

---

## 10. Rust implementation notes (types)

Suggested mapping for the types used above:

| Protocol | Rust |
|---|---|
| VarInt | `i32` with LEB128-style codec (max 5 bytes) |
| VarLong | `i64`, max 10 bytes |
| String (n) | `String` + `VarInt` byte-len prefix |
| UUID | `[u8; 16]` or two `u64` (MSB first) |
| Position | `i64` packed per §2; decode with arithmetic shifts |
| Angle | `u8` |
| NBT | standard big-endian NBT, root name `""` (write `0x0A 0x00 0x00` …) |
| BitSet | `Vec<u64>` + `VarInt` len |
| arrays | `Vec<T>` + `VarInt` count |
| Optional X | `bool` + `Option<T>` |

---

## 11. Discrepancies & known gaps

Discrepancies found between sources:

1. **oxcraft `SetCompression` ID = `0x04` — wrong.** minecraft-data,
   MCProtocolLib (`ClientboundLoginCompressionPacket` at `0x03`), valence's
   vanilla-extracted `packets.json` (`LoginCompressionS2CPacket` id 3) and
   wiki.vg all say **`0x03`**. `0x04` is Login Plugin Request. Use `0x03`.
2. **wiki.vg's Set Experience field order** (Experience bar, *Total
   Experience*, *Level*) disagrees with minecraft-data, valence
   (`bar, level, total_xp`) and MCProtocolLib (`experience, level,
   totalExperience`), which all read/write **bar, level, total**. This
   document uses the implementation order (bar, level, total); wiki.vg's table
   is treated as the error.
3. **wiki.vg's registry-codec prose for 1.20.1 says "three registries"** (text
   frozen since 1.19.2). Vanilla 1.20.1 sends **six** (confirmed by valence's
   vanilla-extracted `registry_codec.dat`, minecraft-data's captured
   `loginPacket.json`, and go-mc's 1.20.x `NetworkCodec`).
4. **"Trust Edges" is not a 763 field.** It exists only in ≤1.17 Update Light;
   Level Chunk With Light in 1.20.1 has no such boolean (minecraft-data,
   MCProtocolLib and valence all omit it).
5. **Current (2026) minecraft.wiki "Chunk format" page is for modern versions**
   (no `Data Array Length`, typed heightmap structure, per-section fluid
   count). For 763 use §7: the long-count VarInt **is** sent, heightmaps are
   NBT long arrays, and sections have no fluid count.
6. Naming differs everywhere: e.g. wiki.vg "Chunk Data and Update Light" =
   minecraft-data `map_chunk` = MCPL `ClientboundLevelChunkWithLightPacket`;
   "Set Center Chunk" = `update_view_position`; "Disguised Chat Message" =
   `profileless_chat`; Chat packets' `Message Count` = minecraft-data `offset`.
7. **Biome direct-palette width is registry-size dependent.** Vanilla 64 biomes
   → 6 bits. The modern wiki's "7" corresponds to later versions/registries;
   for 763 compute `ceil(log2(entry_count))` from the registry you send.
8. Offline UUID: the standard is
   `UUID.nameUUIDFromBytes("OfflinePlayer:"+name)` (MD5, no namespace). oxcraft
   computes `Uuid::new_v3(NAMESPACE_DNS, …)`, which is **not** the same value.
   Clients accept whatever UUID Login Success carries, but use the standard
   formula if you want vanilla-compatible player data.

Known gaps (could not be determined from primary sources):

* The exact registry/field set the vanilla 1.20.1 client hard-requires in the
  registry codec (vs. lazily dereferences). Six vanilla registries is the safe
  answer; a truly minimal codec (1 dimension + few biomes + 1 chat type) is
  plausible (oxcraft ships 4 partial registries in its own demo) but was not
  verified against the vanilla client.
* The canonical name→ID mapping for the 41 block-entity types was not pulled
  into this document (send zero block entities in a minimal server and the
  question is moot; the list is in valence's `blocks.json`
  (`block_entity_types`) and minecraft-data).
* Whether `previewsChat`/`enforcesSecureChat` are strictly required keys in the
  status JSON (both are tolerated when present; omission behaviour untested).
* Legacy 1.6 Server List Ping (`0xFE`) details were not re-verified; skip it
  for a minimal server.
