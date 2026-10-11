//! Wire types (doc §2) and packet framing + zlib compression (doc §3).
//!
//! The full primitive set is kept for extensibility even where a helper has no
//! caller yet.
#![allow(dead_code)]

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{self, BufReader, BufWriter, Cursor, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Maximum packet length: 2^21 - 1 (doc §3).
pub const MAX_PACKET_LEN: usize = 2_097_151;

/// Default protocol String cap in characters (doc §2).
pub const MAX_STRING_LEN: usize = 32767;

// ---------------------------------------------------------------------------
// Primitive readers / writers
// ---------------------------------------------------------------------------

pub fn read_u8<R: Read>(r: &mut R) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

pub fn read_bool<R: Read>(r: &mut R) -> io::Result<bool> {
    Ok(read_u8(r)? != 0)
}

pub fn read_i8<R: Read>(r: &mut R) -> io::Result<i8> {
    Ok(read_u8(r)? as i8)
}

pub fn read_u16<R: Read>(r: &mut R) -> io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

pub fn read_i16<R: Read>(r: &mut R) -> io::Result<i16> {
    Ok(read_u16(r)? as i16)
}

pub fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

pub fn read_i64<R: Read>(r: &mut R) -> io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_be_bytes(b))
}

pub fn read_f32<R: Read>(r: &mut R) -> io::Result<f32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(f32::from_be_bytes(b))
}

pub fn read_f64<R: Read>(r: &mut R) -> io::Result<f64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(f64::from_be_bytes(b))
}

pub fn read_bytes<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

pub fn write_u8<W: Write>(w: &mut W, v: u8) -> io::Result<()> {
    w.write_all(&[v])
}

pub fn write_bool<W: Write>(w: &mut W, v: bool) -> io::Result<()> {
    write_u8(w, v as u8)
}

pub fn write_i8<W: Write>(w: &mut W, v: i8) -> io::Result<()> {
    write_u8(w, v as u8)
}

pub fn write_u16<W: Write>(w: &mut W, v: u16) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn write_i16<W: Write>(w: &mut W, v: i16) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn write_i32<W: Write>(w: &mut W, v: i32) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn write_i64<W: Write>(w: &mut W, v: i64) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn write_f32<W: Write>(w: &mut W, v: f32) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

pub fn write_f64<W: Write>(w: &mut W, v: f64) -> io::Result<()> {
    w.write_all(&v.to_be_bytes())
}

// ---------------------------------------------------------------------------
// VarInt / VarLong (doc §2): LEB128-style, little-endian 7-bit groups.
// ---------------------------------------------------------------------------

pub fn read_var_int<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut result = 0u32;
    for i in 0..5 {
        let byte = read_u8(r)?;
        result |= ((byte & 0x7F) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(result as i32);
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "varint too long"))
}

pub fn write_var_int<W: Write>(w: &mut W, value: i32) -> io::Result<()> {
    let mut v = value as u32;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        w.write_all(&[byte])?;
        if v == 0 {
            return Ok(());
        }
    }
}

/// Number of bytes `write_var_int` produces for `value`.
pub fn var_int_len(value: i32) -> usize {
    let mut v = value as u32;
    let mut n = 1;
    loop {
        v >>= 7;
        if v == 0 {
            return n;
        }
        n += 1;
    }
}

/// Parse a VarInt from a byte slice without consuming partial data.
/// `Ok(None)` = more bytes needed; `Ok(Some((value, len)))` = parsed `len` bytes.
pub fn try_var_int(buf: &[u8]) -> io::Result<Option<(i32, usize)>> {
    let mut result = 0u32;
    for (i, &byte) in buf.iter().enumerate().take(5) {
        result |= ((byte & 0x7F) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(Some((result as i32, i + 1)));
        }
    }
    if buf.len() < 5 {
        Ok(None)
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "varint too long"))
    }
}

pub fn read_var_long<R: Read>(r: &mut R) -> io::Result<i64> {
    let mut result = 0u64;
    for i in 0..10 {
        let byte = read_u8(r)?;
        result |= ((byte & 0x7F) as u64) << (7 * i);
        if byte & 0x80 == 0 {
            return Ok(result as i64);
        }
    }
    Err(io::Error::new(io::ErrorKind::InvalidData, "varlong too long"))
}

pub fn write_var_long<W: Write>(w: &mut W, value: i64) -> io::Result<()> {
    let mut v = value as u64;
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        w.write_all(&[byte])?;
        if v == 0 {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// String (doc §2): VarInt byte length + UTF-8, capped at n chars / 4n bytes.
// ---------------------------------------------------------------------------

pub fn read_string_max<R: Read>(r: &mut R, max_chars: usize) -> io::Result<String> {
    let len = read_var_int(r)?;
    if len < 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "negative string length"));
    }
    if (len as usize) > max_chars.saturating_mul(4) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string too long"));
    }
    let bytes = read_bytes(r, len as usize)?;
    let s = String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "string is not UTF-8"))?;
    if s.chars().count() > max_chars {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "string too long"));
    }
    Ok(s)
}

pub fn read_string<R: Read>(r: &mut R) -> io::Result<String> {
    read_string_max(r, MAX_STRING_LEN)
}

pub fn write_string<W: Write>(w: &mut W, s: &str) -> io::Result<()> {
    write_var_int(w, s.len() as i32)?;
    w.write_all(s.as_bytes())
}

// ---------------------------------------------------------------------------
// UUID (doc §2): 16 raw bytes, most significant 64 bits first.
// ---------------------------------------------------------------------------

pub type WireUuid = [u8; 16];

pub fn read_uuid<R: Read>(r: &mut R) -> io::Result<WireUuid> {
    let mut u = [0u8; 16];
    r.read_exact(&mut u)?;
    Ok(u)
}

pub fn write_uuid<W: Write>(w: &mut W, uuid: &WireUuid) -> io::Result<()> {
    w.write_all(uuid)
}

/// Standard offline-mode player UUID (doc §5.3): Java
/// `UUID.nameUUIDFromBytes("OfflinePlayer:"+name)` — MD5, version 3, IETF variant.
pub fn offline_uuid(name: &str) -> WireUuid {
    use md5::Digest;
    let hash = md5::Md5::digest(format!("OfflinePlayer:{name}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash);
    bytes[6] = (bytes[6] & 0x0F) | 0x30; // version nibble 3
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // IETF variant
    bytes
}

// ---------------------------------------------------------------------------
// Position / Angle (doc §2)
// ---------------------------------------------------------------------------

/// Pack a Position: x:26 bits, z:26 bits, y:12 bits (signed two's complement).
pub fn pack_position(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

pub fn unpack_position(v: i64) -> (i32, i32, i32) {
    let x = (v >> 38) as i32;
    let y = ((v << 52) >> 52) as i32;
    let z = ((v << 26) >> 38) as i32;
    (x, y, z)
}

pub fn read_position<R: Read>(r: &mut R) -> io::Result<(i32, i32, i32)> {
    Ok(unpack_position(read_i64(r)?))
}

pub fn write_position<W: Write>(w: &mut W, x: i32, y: i32, z: i32) -> io::Result<()> {
    write_i64(w, pack_position(x, y, z))
}

pub fn read_angle<R: Read>(r: &mut R) -> io::Result<f32> {
    Ok(read_u8(r)? as f32 * 360.0 / 256.0)
}

pub fn write_angle<W: Write>(w: &mut W, degrees: f32) -> io::Result<()> {
    write_u8(w, (degrees * 256.0 / 360.0) as u8)
}

// ---------------------------------------------------------------------------
// BitSet (doc §2): VarInt long count + big-endian i64s.
// ---------------------------------------------------------------------------

pub fn write_bit_set<W: Write>(w: &mut W, longs: &[i64]) -> io::Result<()> {
    write_var_int(w, longs.len() as i32)?;
    for &l in longs {
        write_i64(w, l)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Packet framing + compression (doc §3)
// ---------------------------------------------------------------------------

/// Reads and writes length-prefixed packet frames (doc §3), switching to the
/// compressed framing after `Set Compression`. Partial frames survive read
/// timeouts so the connection loop can interleave keep-alives and shutdown
/// checks without corrupting the stream.
pub struct PacketConn {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    compression: Option<i32>,
    raw: Vec<u8>,
    tmp: Vec<u8>,
}

impl PacketConn {
    pub fn new(stream: TcpStream) -> io::Result<PacketConn> {
        let writer = BufWriter::new(stream.try_clone()?);
        let reader = BufReader::new(stream);
        Ok(PacketConn {
            reader,
            writer,
            compression: None,
            raw: Vec::new(),
            tmp: vec![0u8; 8192],
        })
    }

    /// Socket read timeout for the per-client loops (None = block forever).
    pub fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(d)
    }

    /// Enable/disable compressed framing (doc §3.2). Negative threshold disables.
    pub fn set_compression(&mut self, threshold: i32) {
        self.compression = if threshold < 0 {
            None
        } else {
            Some(threshold)
        };
    }

    fn fill(&mut self) -> io::Result<usize> {
        let n = self.reader.read(&mut self.tmp)?;
        self.raw.extend_from_slice(&self.tmp[..n]);
        Ok(n)
    }

    /// Pull one complete frame (length prefix stripped) out of `raw`, if ready.
    fn take_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        let (len, hdr) = match try_var_int(&self.raw)? {
            Some(x) => x,
            None => return Ok(None),
        };
        let len = len as usize;
        if len == 0 || len > MAX_PACKET_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame length"));
        }
        if self.raw.len() < hdr + len {
            return Ok(None);
        }
        let frame = self.raw[hdr..hdr + len].to_vec();
        self.raw.drain(..hdr + len);
        Ok(Some(frame))
    }

    /// Undo the compression framing (doc §3.2) if compression is enabled.
    fn decode_frame(&self, frame: &[u8]) -> io::Result<Vec<u8>> {
        match self.compression {
            None => Ok(frame.to_vec()),
            Some(_) => {
                let mut cur = Cursor::new(frame);
                let data_len = read_var_int(&mut cur)?;
                let rest = &frame[cur.position() as usize..];
                if data_len == 0 {
                    return Ok(rest.to_vec());
                }
                if data_len < 0 || data_len as usize > MAX_PACKET_LEN {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "bad data length"));
                }
                let mut out = Vec::new();
                ZlibDecoder::new(rest).read_to_end(&mut out)?;
                if out.len() != data_len as usize {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "decompressed size mismatch",
                    ));
                }
                Ok(out)
            }
        }
    }

    /// Read one packet; returns `(packet id, body)` (body excludes the id).
    pub fn read_packet(&mut self) -> io::Result<(i32, Vec<u8>)> {
        loop {
            if let Some(frame) = self.take_frame()? {
                let payload = self.decode_frame(&frame)?;
                let mut cur = Cursor::new(payload.as_slice());
                let id = read_var_int(&mut cur)?;
                let body = payload[cur.position() as usize..].to_vec();
                return Ok((id, body));
            }
            if self.fill()? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed",
                ));
            }
        }
    }

    /// Frame and send one packet (id + body), then flush.
    pub fn send_packet(&mut self, id: i32, body: &[u8]) -> io::Result<()> {
        let mut payload = Vec::with_capacity(body.len() + 5);
        write_var_int(&mut payload, id)?;
        payload.extend_from_slice(body);
        if payload.len() > MAX_PACKET_LEN {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "packet too large"));
        }
        match self.compression {
            None => {
                write_var_int(&mut self.writer, payload.len() as i32)?;
                self.writer.write_all(&payload)?;
            }
            Some(thr) => {
                if payload.len() < thr as usize {
                    // Below threshold: raw payload with Data Length = 0 (doc §3.2).
                    write_var_int(&mut self.writer, (1 + payload.len()) as i32)?;
                    write_var_int(&mut self.writer, 0)?;
                    self.writer.write_all(&payload)?;
                } else {
                    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
                    enc.write_all(&payload)?;
                    let compressed = enc.finish()?;
                    let frame_len = var_int_len(payload.len() as i32) + compressed.len();
                    if frame_len > MAX_PACKET_LEN {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "packet too large",
                        ));
                    }
                    write_var_int(&mut self.writer, frame_len as i32)?;
                    write_var_int(&mut self.writer, payload.len() as i32)?;
                    self.writer.write_all(&compressed)?;
                }
            }
        }
        self.writer.flush()
    }
}
