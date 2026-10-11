# startech-rs — a Minecraft server written from scratch in Rust

A ground-up implementation of a Minecraft **Java Edition 1.20.1 (protocol 763)**
server — no JVM, no upstream server. It speaks the wire protocol natively and,
uniquely, supports **Forge (FML) clients natively** as well.

## Why it's interesting

Most hobby servers wrap an existing Java server. This one implements the protocol
itself from primary sources, and the Forge handshake is transcribed from Forge
47.4.23 source — see the docs:

- [`docs/PROTOCOL-763.md`](docs/PROTOCOL-763.md) — exact 763 wire format (packet
  IDs, field layouts, chunk format), sourced from `minecraft-data` + wiki.vg.
- [`docs/FORGE-PROTOCOL.md`](docs/FORGE-PROTOCOL.md) — the FML3 login handshake,
  discriminator codec, `forgeData` status blob, with file:line citations into Forge.

## Features

- Offline-mode login, status ping, configuration, play state
- World / chunk generation and NBT, registries, biomes
- Native Forge login (`startech-forge-native=true`) **or** supervision of a real
  Forge JVM (`startech-forge-enabled`)

## Requirements

- Rust (edition 2021). No Java needed for the native path.

## Build & run

```bash
cargo build --release
cp server.properties.example server.properties   # then edit
./target/release/startech-rs server.properties
```

`tools/` contains helper clients used during development:
`fake_client.py`, `forge_client.py`, `status_ping.py`, `gen_blockstates.py`,
`chunk_doctor.py`.

`target/` is git-ignored.
