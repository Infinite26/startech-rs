//! Packet structs + encode/decode for protocol 763 (doc §4, §5).
//!
//! Each packet carries an `ID` const matching doc §4 and an `encode`/`decode`
//! that works on the packet *body* only — framing and compression live in
//! `types::PacketConn`.

use super::types::{self, WireUuid};
use crate::nbt;
use crate::registry;
use std::io::{self, Cursor, Write};

fn cursor(body: &[u8]) -> Cursor<&[u8]> {
    Cursor::new(body)
}

// ── Handshaking (doc §5.1) ─────────────────────────────────────────────────

pub struct Handshake {
    pub protocol_version: i32,
    #[allow(dead_code)]
    pub server_address: String,
    #[allow(dead_code)]
    pub server_port: u16,
    pub next_state: i32,
}

impl Handshake {
    pub const ID: i32 = 0x00;

    pub fn decode(body: &[u8]) -> io::Result<Handshake> {
        let mut r = cursor(body);
        Ok(Handshake {
            protocol_version: types::read_var_int(&mut r)?,
            server_address: types::read_string_max(&mut r, 255)?,
            server_port: types::read_u16(&mut r)?,
            next_state: types::read_var_int(&mut r)?,
        })
    }
}

// ── Status (doc §5.2) ──────────────────────────────────────────────────────

pub struct StatusRequest;

impl StatusRequest {
    pub const ID: i32 = 0x00;
}

pub struct StatusResponse {
    pub json: String,
}

impl StatusResponse {
    pub const ID: i32 = 0x00;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_string(&mut w, &self.json)?;
        Ok(w)
    }
}

pub struct PingRequest {
    pub payload: i64,
}

impl PingRequest {
    pub const ID: i32 = 0x01;

    pub fn decode(body: &[u8]) -> io::Result<PingRequest> {
        Ok(PingRequest {
            payload: types::read_i64(&mut cursor(body))?,
        })
    }
}

pub struct PingResponse {
    pub payload: i64,
}

impl PingResponse {
    pub const ID: i32 = 0x01;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_i64(&mut w, self.payload)?;
        Ok(w)
    }
}

// ── Login (doc §5.3) ───────────────────────────────────────────────────────

pub struct LoginStart {
    pub name: String,
    #[allow(dead_code)]
    pub uuid: Option<WireUuid>,
}

impl LoginStart {
    pub const ID: i32 = 0x00;

    pub fn decode(body: &[u8]) -> io::Result<LoginStart> {
        let mut r = cursor(body);
        let name = types::read_string_max(&mut r, 16)?;
        let uuid = if types::read_bool(&mut r)? {
            Some(types::read_uuid(&mut r)?)
        } else {
            None
        };
        Ok(LoginStart { name, uuid })
    }
}

pub struct LoginDisconnect {
    pub reason: String,
}

impl LoginDisconnect {
    pub const ID: i32 = 0x00;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_string(&mut w, &self.reason)?;
        Ok(w)
    }
}

pub struct LoginSuccess {
    pub uuid: WireUuid,
    pub username: String,
}

impl LoginSuccess {
    pub const ID: i32 = 0x02;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_uuid(&mut w, &self.uuid)?;
        types::write_string(&mut w, &self.username)?;
        types::write_var_int(&mut w, 0)?; // properties: none in offline mode
        Ok(w)
    }
}

pub struct SetCompression {
    pub threshold: i32,
}

impl SetCompression {
    pub const ID: i32 = 0x03;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_var_int(&mut w, self.threshold)?;
        Ok(w)
    }
}

/// Login Plugin Request `0x04` (doc §5.3, forge doc §3) — the vanilla packet
/// Forge rides its whole login-phase handshake on. `message_id` is the
/// per-connection transaction id that the client echoes back; `data` is the
/// remaining bytes of the buffer (not length-prefixed).
pub struct LoginPluginRequest {
    #[allow(dead_code)]
    pub message_id: i32,
    pub channel: String,
    pub data: Vec<u8>,
}

impl LoginPluginRequest {
    pub const ID: i32 = 0x04;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_var_int(&mut w, self.message_id)?;
        types::write_string(&mut w, &self.channel)?;
        w.write_all(&self.data)?;
        Ok(w)
    }
}

/// Login Plugin Response `0x02` (doc §5.3, forge doc §3) — the client's answer
/// to a `LoginPluginRequest`. A vanilla client answers `successful = false` for
/// a channel it does not know, which is how we detect a non-Forge peer.
pub struct LoginPluginResponse {
    pub message_id: i32,
    pub successful: bool,
    pub data: Vec<u8>,
}

impl LoginPluginResponse {
    pub const ID: i32 = 0x02;

    pub fn decode(body: &[u8]) -> io::Result<LoginPluginResponse> {
        let mut r = cursor(body);
        let message_id = types::read_var_int(&mut r)?;
        let successful = types::read_bool(&mut r)?;
        let data = r.get_ref()[r.position() as usize..].to_vec();
        Ok(LoginPluginResponse {
            message_id,
            successful,
            data,
        })
    }
}

// ── Play: clientbound (doc §4.4, §5.4) ─────────────────────────────────────

/// Login (play) / "Join Game" `0x28` (doc §5.4). Death location fields are
/// omitted (`has death location = false`).
pub struct LoginPlay {
    pub entity_id: i32,
    pub hardcore: bool,
    pub gamemode: u8,
    pub previous_gamemode: i8,
    pub dimension_names: Vec<String>,
    pub dimension_type: String,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub is_debug: bool,
    pub is_flat: bool,
    pub portal_cooldown: i32,
}

impl LoginPlay {
    pub const ID: i32 = 0x28;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_i32(&mut w, self.entity_id)?;
        types::write_bool(&mut w, self.hardcore)?;
        types::write_u8(&mut w, self.gamemode)?;
        types::write_i8(&mut w, self.previous_gamemode)?;
        types::write_var_int(&mut w, self.dimension_names.len() as i32)?;
        for name in &self.dimension_names {
            types::write_string(&mut w, name)?;
        }
        nbt::write(&mut w, &registry::registry_codec())?;
        types::write_string(&mut w, &self.dimension_type)?;
        types::write_string(&mut w, &self.dimension_name)?;
        types::write_i64(&mut w, self.hashed_seed)?;
        types::write_var_int(&mut w, self.max_players)?;
        types::write_var_int(&mut w, self.view_distance)?;
        types::write_var_int(&mut w, self.simulation_distance)?;
        types::write_bool(&mut w, self.reduced_debug_info)?;
        types::write_bool(&mut w, self.enable_respawn_screen)?;
        types::write_bool(&mut w, self.is_debug)?;
        types::write_bool(&mut w, self.is_flat)?;
        types::write_bool(&mut w, false)?; // has death location
        types::write_var_int(&mut w, self.portal_cooldown)?;
        Ok(w)
    }
}

/// Plugin Message `0x17` (doc §5.4).
pub struct PluginMessage {
    pub channel: String,
    pub data: Vec<u8>,
}

impl PluginMessage {
    pub const ID: i32 = 0x17;

    /// `minecraft:brand` plugin message: data is a protocol String.
    pub fn brand(name: &str) -> PluginMessage {
        let mut data = Vec::new();
        types::write_string(&mut data, name).expect("vec writes are infallible");
        PluginMessage {
            channel: "minecraft:brand".to_string(),
            data,
        }
    }

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_string(&mut w, &self.channel)?;
        w.write_all(&self.data)?;
        Ok(w)
    }
}

/// Set Default Spawn Position `0x50` (doc §5.4).
pub struct SetDefaultSpawnPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub angle: f32,
}

impl SetDefaultSpawnPosition {
    pub const ID: i32 = 0x50;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_position(&mut w, self.x, self.y, self.z)?;
        types::write_f32(&mut w, self.angle)?;
        Ok(w)
    }
}

/// Set Center Chunk `0x4E` (minecraft-data alias `update_view_position`, doc §5.4).
pub struct SetCenterChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl SetCenterChunk {
    pub const ID: i32 = 0x4E;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_var_int(&mut w, self.chunk_x)?;
        types::write_var_int(&mut w, self.chunk_z)?;
        Ok(w)
    }
}

/// Set Health `0x57` (doc §5.4).
pub struct SetHealth {
    pub health: f32,
    pub food: i32,
    pub saturation: f32,
}

impl SetHealth {
    pub const ID: i32 = 0x57;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_f32(&mut w, self.health)?;
        types::write_var_int(&mut w, self.food)?;
        types::write_f32(&mut w, self.saturation)?;
        Ok(w)
    }
}

/// Update Time `0x5E` (doc §5.4).
pub struct UpdateTime {
    pub world_age: i64,
    pub time_of_day: i64,
}

impl UpdateTime {
    pub const ID: i32 = 0x5E;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_i64(&mut w, self.world_age)?;
        types::write_i64(&mut w, self.time_of_day)?;
        Ok(w)
    }
}

/// Set Experience `0x56` — wire order is bar, level, total (doc §5.4, §11 item 2).
pub struct SetExperience {
    pub bar: f32,
    pub level: i32,
    pub total: i32,
}

impl SetExperience {
    pub const ID: i32 = 0x56;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_f32(&mut w, self.bar)?;
        types::write_var_int(&mut w, self.level)?;
        types::write_var_int(&mut w, self.total)?;
        Ok(w)
    }
}

/// Synchronize Player Position `0x3C` (doc §5.4).
pub struct SyncPlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub flags: u8,
    pub teleport_id: i32,
}

impl SyncPlayerPosition {
    pub const ID: i32 = 0x3C;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_f64(&mut w, self.x)?;
        types::write_f64(&mut w, self.y)?;
        types::write_f64(&mut w, self.z)?;
        types::write_f32(&mut w, self.yaw)?;
        types::write_f32(&mut w, self.pitch)?;
        types::write_u8(&mut w, self.flags)?;
        types::write_var_int(&mut w, self.teleport_id)?;
        Ok(w)
    }
}

/// Keep Alive — clientbound `0x23`, serverbound `0x12` (doc §5.4).
pub struct KeepAlive {
    pub id: i64,
}

impl KeepAlive {
    pub const CLIENTBOUND_ID: i32 = 0x23;
    pub const SERVERBOUND_ID: i32 = 0x12;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_i64(&mut w, self.id)?;
        Ok(w)
    }

    pub fn decode(body: &[u8]) -> io::Result<KeepAlive> {
        Ok(KeepAlive {
            id: types::read_i64(&mut cursor(body))?,
        })
    }
}

/// Chunk sections in a 1.20.1 overworld column: height 384 / 16 (doc §7.3).
pub const SECTION_COUNT: usize = 24;

/// Light masks cover `sections + 2` slots: one below `min_y`, the world
/// sections, one above the world (doc §7.6).
const LIGHT_SLOTS: usize = SECTION_COUNT + 2;

/// Level Chunk With Light `0x24` (doc §7).
pub struct LevelChunkWithLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// Complete network-NBT document (empty root name) for the Heightmaps field.
    pub heightmaps_nbt: Vec<u8>,
    /// Exactly the packet's Data field: the 24 sections (doc §7.3), no block entities.
    pub data: Vec<u8>,
}

impl LevelChunkWithLight {
    pub const ID: i32 = 0x24;

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut w = Vec::new();
        types::write_i32(&mut w, self.chunk_x)?;
        types::write_i32(&mut w, self.chunk_z)?;
        w.write_all(&self.heightmaps_nbt)?;
        types::write_var_int(&mut w, self.data.len() as i32)?;
        w.write_all(&self.data)?;
        types::write_var_int(&mut w, 0)?; // block entity count (doc §7.5)

        // Trivial light (doc §7.6): full skylight in every slot, empty block light.
        let full: i64 = (1i64 << LIGHT_SLOTS) - 1;
        types::write_bit_set(&mut w, &[full])?; // sky light mask
        types::write_bit_set(&mut w, &[0])?; // block light mask
        types::write_bit_set(&mut w, &[0])?; // empty sky light mask
        types::write_bit_set(&mut w, &[full])?; // empty block light mask
        types::write_var_int(&mut w, LIGHT_SLOTS as i32)?; // sky light array count
        for _ in 0..LIGHT_SLOTS {
            types::write_var_int(&mut w, 2048)?;
            w.write_all(&[0xFF; 2048])?; // light 15 everywhere
        }
        types::write_var_int(&mut w, 0)?; // block light array count
        Ok(w)
    }
}

// ── Play: serverbound (doc §4.5, §5.4) ─────────────────────────────────────

/// Confirm Teleportation `0x00` (doc §5.4).
pub struct ConfirmTeleport {
    pub teleport_id: i32,
}

impl ConfirmTeleport {
    pub const ID: i32 = 0x00;

    pub fn decode(body: &[u8]) -> io::Result<ConfirmTeleport> {
        Ok(ConfirmTeleport {
            teleport_id: types::read_var_int(&mut cursor(body))?,
        })
    }
}
