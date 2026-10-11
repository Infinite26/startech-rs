//! Native Forge (FML) login handshake for protocol 763 / MC 1.20.1.
//!
//! Everything in this file is transcribed from Forge 47.4.23 sources rather
//! than guessed; the citations name the Java file the behaviour comes from.
//! See `docs/FORGE-PROTOCOL.md` for the annotated wire format.
//!
//! # Where the handshake lives
//!
//! Forge 1.20.1 has **no** configuration state (that arrived in 1.20.2/protocol
//! 764), so the entire modded handshake happens in the *login* state, on top of
//! the vanilla Login Plugin Request/Response pair. `ServerLoginPacketListenerImpl`
//! is patched to park in an otherwise-unused `NEGOTIATING` state after Login
//! Start and to call `NetworkHooks.tickNegotiation` once per tick; only when
//! that returns true does the vanilla `Login Success` go out
//! (`patches/net/minecraft/server/network/ServerLoginPacketListenerImpl.java.patch:33`).
//!
//! # The envelope
//!
//! Every FML login packet is wrapped in a login query for the channel
//! `fml:loginwrapper` (`LoginWrapper.java:26`), whose payload is:
//!
//! ```text
//! Identifier  target channel ("fml:handshake")
//! VarInt      length of the following bytes
//! bytes       u8 discriminator + message body   (IndexedMessageCodec.java:120,146)
//! ```
//!
//! The transaction id of the login query is the *login index* — Forge uses it to
//! track which replies it is still waiting for, since it is the only
//! request/response pairing available in the login state
//! (`HandshakeHandler.java:58-76`).

use std::io;
use std::time::Duration;

use crate::net::packets::{LoginPluginRequest, LoginPluginResponse};
use crate::net::types::{self, PacketConn};

// ── Constants (NetworkConstants.java, NetworkInitialization.java) ───────────

/// `FMLNETVERSION` — bumped to 3 when `S2CModList` gained the datapack-registry
/// list (NetworkConstants.java:29).
pub const FML_NET_VERSION: i32 = 3;

/// The advertised channel version string, `"FML" + FMLNETVERSION`
/// (NetworkConstants.java:30,50-52).
pub const NET_VERSION: &str = "FML3";

/// Login-phase wrapper channel (LoginWrapper.java:26).
pub const CH_LOGIN_WRAPPER: &str = "fml:loginwrapper";
/// The handshake channel proper (NetworkConstants.java:39).
pub const CH_HANDSHAKE: &str = "fml:handshake";
/// Play-phase FML channel (NetworkConstants.java:40).
pub const CH_PLAY: &str = "fml:play";
/// Vanilla channel names Forge reuses for dynamic registration
/// (NetworkConstants.java:41-42).
pub const CH_REGISTER: &str = "minecraft:register";
pub const CH_UNREGISTER: &str = "minecraft:unregister";

// Message discriminators, i.e. the `int id` passed to `messageBuilder` in
// NetworkInitialization.getHandshakeChannel(). They are written as a leading
// byte by `IndexedMessageCodec.tryEncode` and read back by `consume`.
//
// A modless server only ever emits 5 and 1: there are no Forge registries to
// sync (3), no config files to push (4) and no channel mismatch to report (6),
// and the ack (99) only ever travels client→server. They stay here because they
// are the channel's full message table, and the test
// `discriminators_match_the_java_registration` pins all of them.
#[allow(dead_code)]
pub const MSG_S2C_REGISTRY: u8 = 3;
#[allow(dead_code)]
pub const MSG_S2C_CONFIG_DATA: u8 = 4;
#[allow(dead_code)]
pub const MSG_S2C_CHANNEL_MISMATCH: u8 = 6;
#[allow(dead_code)]
pub const MSG_C2S_ACKNOWLEDGE: u8 = 99;
pub const MSG_S2C_MOD_LIST: u8 = 1;
pub const MSG_C2S_MOD_LIST_REPLY: u8 = 2;
pub const MSG_S2C_MOD_DATA: u8 = 5;

/// Max characters for the `Utf(0x100)` fields in the mod list messages.
const MOD_ID_MAX: usize = 0x100;

/// The login indices this server hands out. Forge uses its position in the
/// gathered-payload list, which is the same thing: the index *is* the vanilla
/// query transaction id, and it is how a reply is matched to its request
/// (`HandshakeHandler.handleIndexedMessage`, :228-235).
const LOGIN_INDEX_MOD_DATA: i32 = 0;
const LOGIN_INDEX_MOD_LIST: i32 = 1;

/// Who is on the other end of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Peer {
    /// Answered our login query with `successful = false`, or never spoke FML.
    Vanilla,
    /// Completed the FML handshake and sent a `C2SModListReply`.
    Forge,
}

/// One `(modId, displayName, version)` triple — the value half of `S2CModData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    pub id: String,
    pub display_name: String,
    pub version: String,
}

/// What the client told us about itself in its `C2SModListReply`
/// (HandshakeMessages.C2SModListReply).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientModList {
    pub mods: Vec<String>,
    pub channels: Vec<(String, String)>,
    /// Registry name -> client-side content hash. Forge itself sends this empty
    /// ("TODO: Fill with known hashes", HandshakeMessages.java:183) and we only
    /// need it for diagnostics.
    pub registries: Vec<(String, String)>,
}

/// The five channels a Forge 47.x server has registered, in `NetworkRegistry`
/// insertion order. `buildChannelVersions()` (NetworkRegistry.java) is just
/// `instances` mapped to their protocol version, and those five instances are
/// the only ones Forge itself creates:
///
/// * `fml:handshake` / `fml:play` — NetworkConstants.java:43-44
/// * `minecraft:register` / `minecraft:unregister` — NetworkConstants.java:45
/// * `fml:loginwrapper` — LoginWrapper.java:30
///
/// Advertising exactly this set matters: `validateChannels` iterates *our*
/// instances and disconnects the peer on any channel we hold that it does not
/// answer for (`HandshakeHandler.java:240-249`), while the client applies the
/// mirror-image test to whatever we advertise.
pub fn channels() -> Vec<(String, &'static str)> {
    vec![
        (CH_HANDSHAKE.to_string(), NET_VERSION),
        (CH_PLAY.to_string(), NET_VERSION),
        (CH_REGISTER.to_string(), NET_VERSION),
        (CH_UNREGISTER.to_string(), NET_VERSION),
        (CH_LOGIN_WRAPPER.to_string(), NET_VERSION),
    ]
}

// ── Encoding helpers ────────────────────────────────────────────────────────

/// `FriendlyByteBuf.writeUtf(s, 0x100)` — a length-prefixed string whose limit
/// is counted in characters, not bytes.
fn write_mod_str(w: &mut Vec<u8>, s: &str) -> io::Result<()> {
    if s.chars().count() > MOD_ID_MAX {
        return Err(invalid("fml string exceeds 0x100 characters"));
    }
    types::write_string(w, s)
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn read_count(r: &mut &[u8]) -> io::Result<usize> {
    let n = types::read_var_int(r)?;
    // Every one of these is a "read N elements" loop; a hostile length would
    // otherwise allocate before failing.
    if !(0..=0x10000).contains(&n) {
        return Err(invalid("fml collection length out of range"));
    }
    Ok(n as usize)
}

/// `S2CModData` (HandshakeMessages.java:159-165): `writeMap` over
/// `modId -> (displayName, version)`.
pub fn encode_mod_data(mods: &[ModInfo]) -> io::Result<Vec<u8>> {
    let mut w = Vec::new();
    types::write_var_int(&mut w, mods.len() as i32)?;
    for m in mods {
        write_mod_str(&mut w, &m.id)?;
        write_mod_str(&mut w, &m.display_name)?;
        write_mod_str(&mut w, &m.version)?;
    }
    Ok(w)
}

/// `S2CModList` (HandshakeMessages.java:97-113): mod ids, channel versions,
/// the Forge registries that need syncing, then the synced datapack registries.
pub fn encode_mod_list(
    mods: &[String],
    channels: &[(String, String)],
    registries: &[String],
    data_pack_registries: &[String],
) -> io::Result<Vec<u8>> {
    let mut w = Vec::new();
    types::write_var_int(&mut w, mods.len() as i32)?;
    for m in mods {
        write_mod_str(&mut w, m)?;
    }
    types::write_var_int(&mut w, channels.len() as i32)?;
    for (name, version) in channels {
        types::write_string(&mut w, name)?;
        write_mod_str(&mut w, version)?;
    }
    types::write_var_int(&mut w, registries.len() as i32)?;
    for r in registries {
        types::write_string(&mut w, r)?;
    }
    types::write_var_int(&mut w, data_pack_registries.len() as i32)?;
    for r in data_pack_registries {
        types::write_string(&mut w, r)?;
    }
    Ok(w)
}

/// `C2SModListReply` (HandshakeMessages.java:213-229) as sent by the client.
pub fn decode_mod_list_reply(data: &[u8]) -> io::Result<ClientModList> {
    let mut r = data;
    let mut out = ClientModList::default();

    let n = read_count(&mut r)?;
    for _ in 0..n {
        out.mods.push(types::read_string_max(&mut r, MOD_ID_MAX)?);
    }
    let n = read_count(&mut r)?;
    for _ in 0..n {
        let name = types::read_string_max(&mut r, 32767)?;
        let version = types::read_string_max(&mut r, MOD_ID_MAX)?;
        out.channels.push((name, version));
    }
    let n = read_count(&mut r)?;
    for _ in 0..n {
        let name = types::read_string_max(&mut r, 32767)?;
        let hash = types::read_string_max(&mut r, MOD_ID_MAX)?;
        out.registries.push((name, hash));
    }
    Ok(out)
}

/// Wrap a handshake message for the login query envelope: the target channel
/// identifier, then a length-delimited block whose first byte is the message
/// discriminator (LoginWrapper.wrapPacket + IndexedMessageCodec.tryEncode).
pub fn wrap(target: &str, message_id: u8, body: &[u8]) -> io::Result<Vec<u8>> {
    let mut w = Vec::new();
    types::write_string(&mut w, target)?;
    types::write_var_int(&mut w, (body.len() + 1) as i32)?;
    w.push(message_id);
    w.extend_from_slice(body);
    Ok(w)
}

/// Inverse of [`wrap`], returning `(target channel, discriminator, body)`.
pub fn unwrap(data: &[u8]) -> io::Result<(String, u8, &[u8])> {
    let mut r = data;
    let target = types::read_string_max(&mut r, 32767)?;
    let len = types::read_var_int(&mut r)?;
    if len <= 0 || len as usize > r.len() {
        return Err(invalid("fml login wrapper length out of range"));
    }
    let block = &r[..len as usize];
    if block.is_empty() {
        return Err(invalid("fml login wrapper block is empty"));
    }
    Ok((target, block[0], &block[1..]))
}

/// Send one `fml:loginwrapper` login query. `login_index` becomes the vanilla
/// transaction id (`HandshakeHandler.tickServer` passes `packetPosition`).
fn send_login_query(
    conn: &mut PacketConn,
    login_index: i32,
    target: &str,
    message_id: u8,
    body: &[u8],
) -> io::Result<()> {
    let data = wrap(target, message_id, body)?;
    conn.send_packet(
        LoginPluginRequest::ID,
        &LoginPluginRequest {
            message_id: login_index,
            channel: CH_LOGIN_WRAPPER.to_string(),
            data,
        }
        .encode()?,
    )
}

// ── The negotiation driver ──────────────────────────────────────────────────

/// Run the server half of the FML login handshake and report what the peer was.
///
/// Forge sends its gathered login payloads one per tick and tracks the ones
/// that still owe a reply (`HandshakeHandler.tickServer`: `packetPosition`
/// walks the list, `sentMessages` holds the indices that need an answer, and
/// negotiation is done when both are exhausted). We have no mods and therefore
/// no Forge registries or config files to sync, which collapses that list to
/// exactly two payloads in registration order:
///
/// | pos | message             | reply? | source |
/// |-----|---------------------|--------|--------|
/// | 0   | `S2CModData` (5)    | no     | `noResponse()`, HandshakeHandler.java:222-225 |
/// | 1   | `S2CModList` (1)    | yes    | client answers with `C2SModListReply` (2) |
///
/// `S2CModData` really does precede `S2CModList` — the message builders are
/// registered in that order (NetworkInitialization.java:32 then :41) and
/// `gatherLoginPayloads` walks `instances` in registration order — which is why
/// the Java comment on `S2CModList` reads "always first handshake message *after*
/// the data sent by S2CModData" (HandshakeMessages.java:51).
///
/// A vanilla client answers the query with `successful = false`; we take that as
/// "not modded" and let the join continue, mirroring Forge's `ConnectionType.VANILLA`.
pub fn negotiate(conn: &mut PacketConn, timeout: Duration) -> io::Result<(Peer, Option<ClientModList>)> {
    let channels = channels();
    let channel_pairs: Vec<(String, String)> = channels
        .iter()
        .map(|(n, v)| (n.clone(), v.to_string()))
        .collect();

    // pos 0 — S2CModData. Empty map, but Forge always sends it, and the client's
    // `handleModData` builds the ConnectionData the play phase reads.
    let mod_data = encode_mod_data(&[])?;
    send_login_query(
        conn,
        LOGIN_INDEX_MOD_DATA,
        CH_HANDSHAKE,
        MSG_S2C_MOD_DATA,
        &mod_data,
    )?;

    // pos 1 — S2CModList. Zero mods, no Forge registries to sync.
    let mod_list = encode_mod_list(&[], &channel_pairs, &[], &[])?;
    send_login_query(
        conn,
        LOGIN_INDEX_MOD_LIST,
        CH_HANDSHAKE,
        MSG_S2C_MOD_LIST,
        &mod_list,
    )?;

    conn.set_read_timeout(Some(timeout))?;
    loop {
        let (id, body) = conn.read_packet()?;
        if id != LoginPluginResponse::ID {
            // Nothing else is legal in this window; a client that wanders off
            // protocol is not worth guessing at.
            return Err(invalid("unexpected packet during fml negotiation"));
        }
        let response = LoginPluginResponse::decode(&body)?;
        if !response.successful {
            // Vanilla client: it does not know `fml:loginwrapper`.
            return Ok((Peer::Vanilla, None));
        }
        let (target, discriminator, payload) = unwrap(&response.data)?;
        if target != CH_HANDSHAKE {
            continue;
        }
        match discriminator {
            MSG_C2S_MOD_LIST_REPLY if response.message_id == LOGIN_INDEX_MOD_LIST => {
                let list = decode_mod_list_reply(payload)?;
                return Ok((Peer::Forge, Some(list)));
            }
            // `S2CModData` was sent with `noResponse()`, so Forge never answers
            // it and a reply carrying `LOGIN_INDEX_MOD_DATA` is unexpected.
            // Forge logs that and carries on (HandshakeHandler.java:231-234);
            // so do we, because the payload index — not the transaction id —
            // is what actually selects the message.
            _ => continue,
        }
    }
}

/// Whether the client's advertised channel list satisfies ours, mirroring
/// `NetworkRegistry.validateServerChannels` (NetworkRegistry.java): every
/// channel *we* registered must appear in the peer's list. Returns the channels
/// the peer is missing.
pub fn missing_channels(client: &ClientModList) -> Vec<String> {
    channels()
        .into_iter()
        .filter(|(name, _)| !client.channels.iter().any(|(n, _)| n == name))
        .map(|(name, _)| name)
        .collect()
}

/// The `forgeData` object Forge adds to the server-list ping
/// (`patches/net/minecraft/network/protocol/status/ServerStatus.java.patch:11`).
///
/// `ServerStatusPing.CODEC` reads `fmlNetworkVersion`, an *optional* compact
/// binary blob `d`, and keeps `channels`/`mods` as empty legacy placeholders
/// (ServerStatusPing.java:60-70,84-94). We have no mods, so the legacy arrays
/// stay empty and `d` is omitted — a Forge client still parses the ping and
/// shows the server as an FML3 server.
pub fn status_forge_data() -> String {
    format!(
        r#"{{"fmlNetworkVersion":{},"channels":[],"mods":[],"truncated":false}}"#,
        FML_NET_VERSION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_data_roundtrip_shape() {
        // Two mods => count, then id/display/version per entry.
        let mods = vec![
            ModInfo {
                id: "minecraft".into(),
                display_name: "Minecraft".into(),
                version: "1.20.1".into(),
            },
            ModInfo {
                id: "forge".into(),
                display_name: "Forge".into(),
                version: "47.4.23".into(),
            },
        ];
        let bytes = encode_mod_data(&mods).unwrap();
        assert_eq!(bytes[0], 2, "map size prefix");
        let mut r = &bytes[..];
        types::read_var_int(&mut r).unwrap();
        assert_eq!(types::read_string_max(&mut r, MOD_ID_MAX).unwrap(), "minecraft");
        assert_eq!(
            types::read_string_max(&mut r, MOD_ID_MAX).unwrap(),
            "Minecraft"
        );
        assert_eq!(types::read_string_max(&mut r, MOD_ID_MAX).unwrap(), "1.20.1");
    }

    #[test]
    fn mod_list_is_empty_with_five_channels() {
        let pairs: Vec<(String, String)> = channels()
            .iter()
            .map(|(n, v)| (n.clone(), v.to_string()))
            .collect();
        let bytes = encode_mod_list(&[], &pairs, &[], &[]).unwrap();
        let mut r = &bytes[..];
        assert_eq!(types::read_var_int(&mut r).unwrap(), 0, "no mods");
        assert_eq!(types::read_var_int(&mut r).unwrap(), 5, "five channels");
        for expected in [
            CH_HANDSHAKE,
            CH_PLAY,
            CH_REGISTER,
            CH_UNREGISTER,
            CH_LOGIN_WRAPPER,
        ] {
            assert_eq!(
                types::read_string_max(&mut r, 32767).unwrap(),
                expected
            );
            assert_eq!(types::read_string_max(&mut r, MOD_ID_MAX).unwrap(), "FML3");
        }
        assert_eq!(types::read_var_int(&mut r).unwrap(), 0, "no registries");
        assert_eq!(
            types::read_var_int(&mut r).unwrap(),
            0,
            "no datapack registries"
        );
    }

    #[test]
    fn wrap_unwrap_roundtrip() {
        let body = encode_mod_list(&[], &channels().into_iter().map(|(a, b)| (a, b.to_string())).collect::<Vec<_>>(), &[], &[]).unwrap();
        let packet = wrap(CH_HANDSHAKE, MSG_S2C_MOD_LIST, &body).unwrap();
        let (target, discriminator, payload) = unwrap(&packet).unwrap();
        assert_eq!(target, CH_HANDSHAKE);
        assert_eq!(discriminator, MSG_S2C_MOD_LIST);
        assert_eq!(payload, &body[..]);
    }

    #[test]
    fn decode_mod_list_reply_from_forge_shaped_bytes() {
        // Byte-for-byte the shape HandshakeMessages.C2SModListReply::decode reads,
        // with the full five-channel set a modless Forge 47.x client advertises.
        let mut b = Vec::new();
        types::write_var_int(&mut b, 1).unwrap(); // one mod: forge itself
        types::write_string(&mut b, "forge").unwrap();
        types::write_var_int(&mut b, 5).unwrap(); // five channels
        for name in channels() {
            types::write_string(&mut b, &name.0).unwrap();
            types::write_string(&mut b, name.1).unwrap();
        }
        types::write_var_int(&mut b, 0).unwrap(); // no registry hashes

        let list = decode_mod_list_reply(&b).unwrap();
        assert_eq!(list.mods, vec!["forge".to_string()]);
        assert_eq!(list.channels.len(), 5);
        assert!(
            missing_channels(&list).is_empty(),
            "a complete forge reply must validate"
        );
    }

    #[test]
    fn discriminators_match_the_java_registration() {
        // Guards the table against a typo: these are the `int id`s passed to
        // `messageBuilder` in NetworkInitialization.getHandshakeChannel, and
        // they go on the wire verbatim as the payload's first byte.
        assert_eq!(MSG_S2C_MOD_LIST, 1);
        assert_eq!(MSG_C2S_MOD_LIST_REPLY, 2);
        assert_eq!(MSG_S2C_REGISTRY, 3);
        assert_eq!(MSG_S2C_CONFIG_DATA, 4);
        assert_eq!(MSG_S2C_MOD_DATA, 5);
        assert_eq!(MSG_S2C_CHANNEL_MISMATCH, 6);
        assert_eq!(MSG_C2S_ACKNOWLEDGE, 99);
        // Only S2CModData carries `noResponse()`, which is why it is the one
        // login payload that does not hold the handshake open.
        assert_eq!(MSG_S2C_MOD_DATA, 5);
    }

    #[test]
    fn missing_channels_detects_a_bare_reply() {
        let list = ClientModList::default();
        let missing = missing_channels(&list);
        assert_eq!(missing.len(), 5);
        assert!(missing.contains(&CH_LOGIN_WRAPPER.to_string()));
    }

    #[test]
    fn status_data_declares_fml3() {
        let json = status_forge_data();
        assert!(json.contains("\"fmlNetworkVersion\":3"));
        assert!(json.contains("\"channels\":[]"));
    }
}
