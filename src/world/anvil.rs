//! Anvil region file reader (`region/r.X.Z.mca`).
//!
//! Region format (doc): 8 KiB header = 1024 location entries (3-byte BE
//! sector offset + 1-byte sector count) + 1024 timestamps. Chunk N at chunk
//! coords (cx, cz) is at index `(cx & 31) + (cz & 31) * 32`. The chunk payload
//! is `[u32 BE length][u8 compression][compressed NBT]` where `length` counts
//! the compression byte plus the compressed bytes.

use super::stats;
use crate::nbt::{self, Value};
use flate2::read::{GzDecoder, ZlibDecoder};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Read one chunk's NBT from the overworld (`world_dir/region`).
///
/// Returns `None` when the chunk is absent, not fully generated, corrupt, or
/// the region file cannot be read (all counted as "missing" in stats).
pub fn read_chunk_nbt(world_dir: &Path, x: i32, z: i32) -> Option<Value> {
    read_chunk_nbt_in(&world_dir.join("region"), x, z)
}

/// Read one chunk's NBT from a specific dimension: `dim` is `""` (overworld),
/// `"DIM-1"` (the nether) or `"DIM1"` (the end).
pub fn read_chunk_nbt_dim(world_dir: &Path, dim: &str, x: i32, z: i32) -> Option<Value> {
    let region_dir = if dim.is_empty() {
        world_dir.join("region")
    } else {
        world_dir.join(dim).join("region")
    };
    read_chunk_nbt_in(&region_dir, x, z)
}

/// Read one chunk's NBT from an explicit directory of `.mca` files.
pub fn read_chunk_nbt_in(region_dir: &Path, x: i32, z: i32) -> Option<Value> {
    let path = region_dir.join(format!("r.{}.{}.mca", x >> 5, z >> 5));
    match read_chunk_raw(&path, x, z) {
        None => {
            stats::chunk_missing();
            None
        }
        Some(root) => {
            // 1.18+ chunk NBT is flat; pre-1.18 wraps everything in `Level`.
            let level = root.get("Level").unwrap_or(&root);
            // Only serve fully generated chunks (doc: skip other statuses as missing).
            let status = level.get("Status").and_then(Value::as_str).unwrap_or("");
            if status != "minecraft:full" && status != "full" {
                stats::chunk_missing();
                return None;
            }
            Some(root)
        }
    }
}

/// Open a region file and pull out one chunk's decompressed NBT.
fn read_chunk_raw(path: &Path, x: i32, z: i32) -> Option<Value> {
    let mut f = File::open(path).ok()?;
    stats::region_read();

    // Header: 8 KiB (1024 locations + 1024 timestamps).
    let mut header = [0u8; 8192];
    f.read_exact(&mut header).ok()?;

    // `& 31` on i32 gives the non-negative in-region coordinate for negatives.
    let idx = ((x & 31) as usize) + ((z & 31) as usize) * 32;
    let loc = &header[idx * 4..idx * 4 + 4];
    let sector_offset = ((loc[0] as u32) << 16) | ((loc[1] as u32) << 8) | loc[2] as u32;
    if sector_offset == 0 {
        return None;
    }

    // Chunk payload header at the sector.
    f.seek(SeekFrom::Start(sector_offset as u64 * 4096)).ok()?;
    let mut head = [0u8; 5];
    f.read_exact(&mut head).ok()?;
    let len = u32::from_be_bytes(head[0..4].try_into().unwrap()) as usize;
    let compression = head[4];
    // `len` covers the compression byte + compressed payload; sane bounds only.
    if len < 1 || len > 32 * 1024 * 1024 {
        return None;
    }
    let mut comp = vec![0u8; len - 1];
    f.read_exact(&mut comp).ok()?;

    let raw = match compression {
        1 => decode(&mut GzDecoder::new(&comp[..])),
        2 => decode(&mut ZlibDecoder::new(&comp[..])),
        3 => Some(comp),
        _ => None,
    }?;
    nbt::read(&mut &raw[..]).ok()
}

fn decode<R: Read>(r: &mut R) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    r.read_to_end(&mut out).ok()?;
    Some(out)
}
