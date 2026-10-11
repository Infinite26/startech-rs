//! Per-client connection state machine (handshake → status | login → play).

use super::packets::*;
use super::types::{self, PacketConn};
use crate::config::Config;
use crate::forge;
use crate::state::ServerState;
use crate::world::World;
use std::io::{self, ErrorKind};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 1.20.1 protocol number (doc §1).
pub const PROTOCOL_VERSION: i32 = 763;
/// Version name reported in the status response (doc §8).
pub const GAME_VERSION: &str = "1.20.1";

const COMPRESSION_THRESHOLD: i32 = 256;
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(30);
/// Socket read timeout so the loops can service keep-alives and `state.running`.
const READ_TICK: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a Forge client gets to answer the FML login queries. Forge itself
/// has no login-phase timeout beyond the vanilla 30 s read timeout, so this is
/// deliberately not tighter than that (forge doc §4).
const FORGE_NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(20);

/// Serve a connection while the managed backend is still starting:
/// status pings are answered normally, login attempts get a polite
/// disconnect with `notice` instead of a silent close.
pub fn serve_startup(
    stream: TcpStream,
    config: &Config,
    state: &Arc<ServerState>,
    notice: &str,
) -> io::Result<()> {
    let mut conn = PacketConn::new(stream)?;
    conn.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let (id, body) = match conn.read_packet() {
        Ok(pkt) => pkt,
        Err(_) => return Ok(()),
    };
    if id != Handshake::ID {
        return Ok(());
    }
    let hs = match Handshake::decode(&body) {
        Ok(hs) => hs,
        Err(_) => return Ok(()),
    };

    match hs.next_state {
        1 => status_loop(&mut conn, config, state),
        2 => {
            let reason = format!("{{\"text\":\"{}\"}}", json_escape(notice));
            conn.send_packet(
                LoginDisconnect::ID,
                &LoginDisconnect { reason }.encode()?,
            )?;
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn handle_client(
    stream: TcpStream,
    world: Arc<World>,
    config: Config,
    state: Arc<ServerState>,
) -> io::Result<()> {
    let mut conn = PacketConn::new(stream)?;    conn.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;

    // ── Handshaking (doc §5.1): exactly one packet is valid here ──
    let (id, body) = match conn.read_packet() {
        Ok(pkt) => pkt,
        Err(_) => return Ok(()),
    };
    if id != Handshake::ID {
        return Ok(());
    }
    let hs = match Handshake::decode(&body) {
        Ok(hs) => hs,
        Err(_) => return Ok(()),
    };

    match hs.next_state {
        1 => status_loop(&mut conn, &config, &state),
        2 if hs.protocol_version == PROTOCOL_VERSION => {
            login_loop(conn, world, config, state)
        }
        2 => {
            // Login-state Disconnect so a wrong-version client sees a reason
            // instead of a silent close (doc §4.3).
            let reason = format!(
                "{{\"text\":\"Outdated server! I'm still running {}\"}}",
                GAME_VERSION
            );
            conn.send_packet(
                LoginDisconnect::ID,
                &LoginDisconnect { reason }.encode()?,
            )?;
            Ok(())
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Status (doc §5.2, §8)
// ---------------------------------------------------------------------------

fn status_loop(conn: &mut PacketConn, config: &Config, state: &Arc<ServerState>) -> io::Result<()> {
    conn.set_read_timeout(Some(READ_TICK))?;
    let mut idle_since = Instant::now();
    loop {
        if !state.running.load(Ordering::Relaxed) {
            return Ok(());
        }
        if idle_since.elapsed() > STATUS_IDLE_TIMEOUT {
            return Ok(());
        }
        let (id, body) = match conn.read_packet() {
            Ok(pkt) => pkt,
            Err(e) if is_timeout(&e) => continue,
            Err(_) => return Ok(()),
        };
        idle_since = Instant::now();
        match id {
            StatusRequest::ID => {
                let json = status_json(config, state);
                conn.send_packet(StatusResponse::ID, &StatusResponse { json }.encode()?)?;
            }
            PingRequest::ID => {
                let ping = PingRequest::decode(&body)?;
                conn.send_packet(
                    PingResponse::ID,
                    &PingResponse {
                        payload: ping.payload,
                    }
                    .encode()?,
                )?;
            }
            _ => {}
        }
    }
}

fn status_json(config: &Config, state: &Arc<ServerState>) -> String {
    let online = state.online.load(Ordering::Relaxed);
    // Forge's patched ServerStatus adds an optional top-level "forgeData"
    // object; when present a Forge client shows the server as an FML3 server
    // with no mods (forge doc §6).
    let forge_data = if config.forge_native {
        format!(r#","forgeData":{}"#, forge::status_forge_data())
    } else {
        String::new()
    };
    format!(
        concat!(
            r#"{{"version":{{"name":"{}","protocol":{}}},"#,
            r#""players":{{"max":{},"online":{}}},"#,
            r#""description":{{"text":"{}"}},"#,
            r#""enforcesSecureChat":false,"previewsChat":false"#,
            r#"{}"#,
            r#"}}"#,
        ),
        GAME_VERSION,
        PROTOCOL_VERSION,
        config.max_players,
        online,
        json_escape(&config.motd),
        forge_data,
    )
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Login → Play (doc §5.3, §9)
// ---------------------------------------------------------------------------

fn login_loop(
    mut conn: PacketConn,
    world: Arc<World>,
    config: Config,
    state: Arc<ServerState>,
) -> io::Result<()> {
    // Login Start (doc §5.3). The optional client UUID is ignored; the offline
    // UUID is derived from the name.
    let (id, body) = match conn.read_packet() {
        Ok(pkt) => pkt,
        Err(_) => return Ok(()),
    };
    if id != LoginStart::ID {
        return Ok(());
    }
    let login = match LoginStart::decode(&body) {
        Ok(l) => l,
        Err(_) => return Ok(()),
    };
    let name = login.name;

    // Set Compression (threshold 256) — sent in the plain framing, the last
    // uncompressed packet (doc §3.2, §9).
    conn.send_packet(
        SetCompression::ID,
        &SetCompression {
            threshold: COMPRESSION_THRESHOLD,
        }
        .encode()?,
    )?;
    conn.set_compression(COMPRESSION_THRESHOLD);

    // Forge FML handshake (forge doc §3). Forge's patched
    // ServerLoginPacketListenerImpl holds the connection in `NEGOTIATING` here
    // and withholds Login Success until `tickNegotiation` returns true, so the
    // queries go out *after* Set Compression — i.e. already compressed — and
    // *before* Login Success.
    let peer = if config.forge_native {
        match forge::negotiate(&mut conn, FORGE_NEGOTIATION_TIMEOUT) {
            Ok((peer, client)) => {
                if peer == forge::Peer::Forge {
                    let mods = client
                        .as_ref()
                        .map(|c| c.mods.len())
                        .unwrap_or_default();
                    let missing = client.as_ref().map(forge::missing_channels).unwrap_or_default();
                    eprintln!(
                        "[startech-rs] {name}: FML handshake complete (client mods: {mods}, channels missing: {missing:?})"
                    );
                } else {
                    eprintln!("[startech-rs] {name}: peer declined the FML handshake, treating as vanilla");
                }
                peer
            }
            Err(e) => {
                // A peer that hangs up instead of answering has almost always read
                // our (empty) mod list and refused it: Forge clients require their
                // own mods to be present on the server, so a heavily modded client
                // cannot join a modless one no matter how correct the handshake is.
                // Say that outright, because a bare "connection closed" sends people
                // hunting for a protocol bug that is not there.
                let hint = if matches!(
                    e.kind(),
                    ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                ) {
                    " -- peer hung up without answering the mod list; a modded Forge \
                     client requires its mods on the server, so it cannot join this \
                     modless one (use a vanilla 1.20.1 client, or startech-forge-enabled \
                     to run a real Forge JVM)"
                } else {
                    ""
                };
                eprintln!("[startech-rs] {name}: FML handshake failed: {e}{hint}");
                return Ok(());
            }
        }
    } else {
        forge::Peer::Vanilla
    };

    // Login Success (doc §5.3). Protocol 763 has NO Login Acknowledged packet
    // (doc §1): both sides switch to Play here, and the client sends nothing
    // until Login (play) 0x28 arrives.
    let uuid = types::offline_uuid(&name);
    conn.send_packet(
        LoginSuccess::ID,
        &LoginSuccess {
            uuid,
            username: name.clone(),
        }
        .encode()?,
    )?;

    state.join(&name);
    let result = play_loop(&mut conn, &world, &config, &state, peer);
    state.leave(&name);
    if let Err(e) = result {
        eprintln!("[startech-rs] {name}: connection ended: {e}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Play (doc §9 join sequence + steady state)
// ---------------------------------------------------------------------------

fn play_loop(
    conn: &mut PacketConn,
    world: &Arc<World>,
    config: &Config,
    state: &Arc<ServerState>,
    peer: forge::Peer,
) -> io::Result<()> {
    conn.set_read_timeout(Some(READ_TICK))?;

    let level = &world.level;
    let spawn_chunk_x = level.spawn_x >> 4;
    let spawn_chunk_z = level.spawn_z >> 4;

    // 1. Login (play) — must be the first Play packet (doc §9).
    conn.send_packet(
        LoginPlay::ID,
        &LoginPlay {
            entity_id: 0,
            hardcore: config.hardcore || level.hardcore,
            gamemode: config.gamemode_id(),
            previous_gamemode: -1,
            dimension_names: vec!["minecraft:overworld".to_string()],
            dimension_type: "minecraft:overworld".to_string(),
            dimension_name: "minecraft:overworld".to_string(),
            hashed_seed: level.seed,
            max_players: config.max_players,
            view_distance: config.view_distance as i32,
            simulation_distance: config.simulation_distance as i32,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            is_debug: false,
            is_flat: false,
            portal_cooldown: 0,
        }
        .encode()?,
    )?;

    // 2. Plugin Message: brand (doc §9). A Forge client reads this to decide
    // whether it is talking to a modded server, so a Forge peer gets the brand
    // real Forge sends; everything else keeps our own (forge doc §5).
    let brand = if peer == forge::Peer::Forge {
        "forge"
    } else {
        "startech-rs"
    };
    conn.send_packet(PluginMessage::ID, &PluginMessage::brand(brand).encode()?)?;

    // 3. Set Default Spawn Position at the world spawn (doc §9).
    conn.send_packet(
        SetDefaultSpawnPosition::ID,
        &SetDefaultSpawnPosition {
            x: level.spawn_x,
            y: level.spawn_y,
            z: level.spawn_z,
            angle: 0.0,
        }
        .encode()?,
    )?;

    // 4. Set Center Chunk (minecraft-data `update_view_position`) (doc §9).
    conn.send_packet(
        SetCenterChunk::ID,
        &SetCenterChunk {
            chunk_x: spawn_chunk_x,
            chunk_z: spawn_chunk_z,
        }
        .encode()?,
    )?;

    // 5. Set Health (doc §9).
    conn.send_packet(
        SetHealth::ID,
        &SetHealth {
            health: 20.0,
            food: 20,
            saturation: 5.0,
        }
        .encode()?,
    )?;

    // 6. Update Time (doc §9).
    conn.send_packet(
        UpdateTime::ID,
        &UpdateTime {
            world_age: level.time,
            time_of_day: level.time.rem_euclid(24_000),
        }
        .encode()?,
    )?;

    // 7. Set Experience (doc §9).
    conn.send_packet(
        SetExperience::ID,
        &SetExperience {
            bar: 0.0,
            level: 0,
            total: 0,
        }
        .encode()?,
    )?;

    // 8. Chunk Data + Light around the spawn chunk (doc §9 item 16).
    send_chunks(conn, world, config, spawn_chunk_x, spawn_chunk_z)?;

    // 9. Synchronize Player Position (doc §9 item 17) — absolute teleport that
    // also closes the client's "Downloading terrain" screen.
    let teleport_id = 1;
    conn.send_packet(
        SyncPlayerPosition::ID,
        &SyncPlayerPosition {
            x: level.spawn_x as f64,
            y: level.spawn_y as f64, // feet Y
            z: level.spawn_z as f64,
            yaw: 0.0,
            pitch: 0.0,
            flags: 0x00, // absolute
            teleport_id,
        }
        .encode()?,
    )?;

    // ── Steady state: keep-alives + serverbound handling ──
    let mut next_keep_alive = Instant::now() + KEEP_ALIVE_INTERVAL;
    let mut awaiting_keep_alive: Option<Instant> = None;
    let mut pending_teleport = Some(teleport_id);

    loop {
        if !state.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Disconnect a client that has not answered a keep-alive for 30 s.
        if let Some(sent_at) = awaiting_keep_alive {
            if sent_at.elapsed() >= KEEP_ALIVE_TIMEOUT {
                eprintln!("[startech-rs] keep-alive timeout, disconnecting client");
                return Ok(());
            }
        }

        if Instant::now() >= next_keep_alive {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            conn.send_packet(KeepAlive::CLIENTBOUND_ID, &KeepAlive { id }.encode()?)?;
            if awaiting_keep_alive.is_none() {
                awaiting_keep_alive = Some(Instant::now());
            }
            next_keep_alive = Instant::now() + KEEP_ALIVE_INTERVAL;
        }

        let (id, body) = match conn.read_packet() {
            Ok(pkt) => pkt,
            Err(e) if is_timeout(&e) => continue,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => {
                eprintln!("[startech-rs] read error, closing connection: {e:?}");
                return Ok(());
            }
        };

        match id {
            KeepAlive::SERVERBOUND_ID => {
                let _ = KeepAlive::decode(&body);
                awaiting_keep_alive = None;
            }
            ConfirmTeleport::ID => {
                if let Ok(confirm) = ConfirmTeleport::decode(&body) {
                    if pending_teleport == Some(confirm.teleport_id) {
                        pending_teleport = None;
                    }
                }
            }
            // Movement (0x14..0x17), Plugin Message / brand (0x0D), Client
            // Information (0x08) and any other serverbound packet: every frame
            // is length-prefixed, so unknown packets can always be dropped
            // safely. No movement broadcast is needed for a single-player test.
            _ => {}
        }
    }
}

fn send_chunks(
    conn: &mut PacketConn,
    world: &Arc<World>,
    config: &Config,
    center_x: i32,
    center_z: i32,
) -> io::Result<()> {
    let radius = i32::from(config.view_distance).min(6);
    let mut error_logged = false;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (x, z) = (center_x + dx, center_z + dz);
            let payload = match world.chunk(x, z) {
                Some(p) => p,
                None => world.void_chunk(),
            };
            if payload.heightmaps_nbt.is_empty() || payload.data.is_empty() {
                if !error_logged {
                    eprintln!("[startech-rs] chunk ({x}, {z}): bad payload, skipping");
                    error_logged = true;
                }
                continue;
            }
            let packet = LevelChunkWithLight {
                chunk_x: x,
                chunk_z: z,
                heightmaps_nbt: payload.heightmaps_nbt,
                data: payload.data,
            };
            conn.send_packet(LevelChunkWithLight::ID, &packet.encode()?)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error classification helpers
// ---------------------------------------------------------------------------

fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}
