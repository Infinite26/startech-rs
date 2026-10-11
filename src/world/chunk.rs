//! Chunk decode → wire `ChunkPayload` (doc §7 "Level Chunk With Light").
//!
//! `ChunkPayload.data` is exactly the packet's Data field: 24 sections bottom
//! to top, each `Block count (i16)` + block-states paletted container + biome
//! paletted container, with **no** trailing block-entity array — in 763 block
//! entities are packet fields 6–7 *after* Data (§7.1), so the net layer writes
//! the entity count (0) itself.

use super::blockstates;
use super::stats;
use super::{ChunkPayload, WorldStats};
use crate::biomes;
use crate::nbt::{self, Value};
use std::path::Path;

/// Overworld: 24 sections, y = -4..=19 (world y -64..=319).
const SECTIONS: i32 = 24;
const SECTION_Y_MIN: i32 = -4;
const MIN_Y: i32 = -64;
const BLOCKS_PER_SECTION: usize = 16 * 16 * 16;
const BIOMES_PER_SECTION: usize = 4 * 4 * 4;
const HEIGHTMAP_ENTRIES: usize = 16 * 16;
/// Heightmap values are 0..=384 (`y - min_y`) → 9 bits (doc §7.2).
const HEIGHTMAP_BITS: u32 = 9;

/// Load chunk (x, z) from the Anvil region files. `None` = chunk absent.
pub fn load_chunk(world_dir: &Path, x: i32, z: i32) -> Option<ChunkPayload> {
    let root = super::anvil::read_chunk_nbt(world_dir, x, z)?;
    let payload = build_payload(&root);
    stats::chunk_parsed();
    Some(payload)
}

/// All-air chunk: 24 empty sections, zeroed heightmaps, biome 0 (plains).
pub fn void_chunk() -> ChunkPayload {
    let mut data = Vec::with_capacity(SECTIONS as usize * 8);
    for _ in 0..SECTIONS {
        data.extend_from_slice(&0i16.to_be_bytes()); // block count
        write_single(&mut data, 0); // block states: air
        write_single(&mut data, 0); // biomes: plains (id 0 in our registry)
    }
    let zeros = vec![0i64; packed_len(HEIGHTMAP_ENTRIES, HEIGHTMAP_BITS)];
    ChunkPayload {
        heightmaps_nbt: heightmaps_nbt(&zeros),
        data,
    }
}

/// Aggregated load statistics (thread-safe; parsed lazily per chunk).
pub fn stats() -> WorldStats {
    stats::snapshot()
}

// ---------------------------------------------------------------------------
// Payload construction
// ---------------------------------------------------------------------------

fn build_payload(root: &Value) -> ChunkPayload {
    // 1.18+ chunk NBT is flat; pre-1.18 wraps everything in `Level`.
    let level = root.get("Level").unwrap_or(root);
    let sections = match level.get("sections") {
        Some(Value::List(items)) => Some(items),
        _ => None,
    };

    let mut data = Vec::with_capacity(SECTIONS as usize * 512);
    // Heightmap column values (x fastest, then z), 0 = empty column.
    let mut heights = [0u16; HEIGHTMAP_ENTRIES];

    for sy in SECTION_Y_MIN..SECTION_Y_MIN + SECTIONS {
        let sec = sections.and_then(|items| {
            items
                .iter()
                .find(|s| s.get("Y").and_then(Value::as_i64) == Some(sy as i64))
        });
        encode_section(&mut data, sy, sec, &mut heights);
    }

    // Heightmaps: non-straddling 9-bit packing into 37 longs (doc §7.2).
    let packed = pack(&heights.map(|h| h as u32), HEIGHTMAP_BITS);
    ChunkPayload {
        heightmaps_nbt: heightmaps_nbt(&packed),
        data,
    }
}

fn encode_section(out: &mut Vec<u8>, sy: i32, sec: Option<&Value>, heights: &mut [u16; 256]) {
    // ---- block states ----
    let mut ids = [0u32; BLOCKS_PER_SECTION]; // default air
    if let Some(bs) = sec.and_then(|s| s.get("block_states")) {
        let palette = map_block_palette(bs.get("palette"));
        let idxs = decode_disk_indices(bs, BLOCKS_PER_SECTION, 4);
        for (dst, pi) in ids.iter_mut().zip(idxs) {
            *dst = palette.get(pi as usize).copied().unwrap_or(0);
        }
    }

    // Block count (non-air, doc §7.3) + heightmap update, in wire order
    // (x fastest, then z, then y — same order as the on-disk data array).
    let mut count = 0i16;
    for (i, id) in ids.iter().enumerate() {
        if !blockstates::is_air_like(*id) {
            count += 1;
            let x = i & 15;
            let z = (i >> 4) & 15;
            let y = sy * 16 + (i >> 8) as i32;
            // value = (highest non-air y + 1) - min_y; all-air column stays 0.
            let h = (y + 1 - MIN_Y) as u16;
            let slot = x + 16 * z;
            if heights[slot] < h {
                heights[slot] = h;
            }
        }
    }

    out.extend_from_slice(&count.to_be_bytes());
    write_block_states(out, &ids);

    // ---- biomes ----
    let mut bio = [0u32; BIOMES_PER_SECTION]; // default plains
    if let Some(b) = sec.and_then(|s| s.get("biomes")) {
        let palette = map_biome_palette(b.get("palette"));
        let idxs = decode_disk_indices(b, BIOMES_PER_SECTION, 1);
        for (dst, pi) in bio.iter_mut().zip(idxs) {
            *dst = palette.get(pi as usize).copied().unwrap_or(0);
        }
    }
    write_biomes(out, &bio);
}

/// On-disk block palette → wire global state ids (modded → air + stats).
fn map_block_palette(pal: Option<&Value>) -> Vec<u32> {
    let items = match pal {
        Some(Value::List(items)) => items,
        _ => return vec![blockstates::air()],
    };
    items
        .iter()
        .map(|entry| {
            let name = entry.get("Name").and_then(Value::as_str).unwrap_or("minecraft:air");
            let props: Vec<(String, String)> = match entry.get("Properties") {
                Some(Value::Compound(c)) => c
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            match blockstates::lookup(name, &props) {
                Some(id) => id,
                None => {
                    stats::unknown_blockstate(name);
                    blockstates::air()
                }
            }
        })
        .collect()
}

/// On-disk biome palette (names) → wire biome ids via `crate::biomes`.
fn map_biome_palette(pal: Option<&Value>) -> Vec<u32> {
    let items = match pal {
        Some(Value::List(items)) => items,
        _ => return vec![0],
    };
    items
        .iter()
        .map(|v| biomes::biome_id(v.as_str().unwrap_or("")) as u32)
        .collect()
}

// ---------------------------------------------------------------------------
// Paletted containers (doc §7.4)
// ---------------------------------------------------------------------------

fn write_single(out: &mut Vec<u8>, value: u32) {
    out.push(0); // bits per entry = 0: single-valued palette
    write_varint(out, value as i32);
    write_varint(out, 0); // empty data array
}

/// Encode 4096 global blockstate ids as a wire block-states container.
///
/// BPE rules (doc §7.4): 1 unique → 0; 2..256 → indirect with BPE 4..8
/// (round-up: never below 4); >256 → direct BPE 15 (24135 states, ceil(log2)=15).
fn write_block_states(out: &mut Vec<u8>, ids: &[u32; BLOCKS_PER_SECTION]) {
    // Collect the distinct ids in first-seen order (stable wire palette).
    let mut palette: Vec<u32> = Vec::new();
    let mut remap = [u16::MAX; 1 << 15]; // state ids fit 15 bits
    for &id in ids {
        let slot = &mut remap[id as usize];
        if *slot == u16::MAX {
            *slot = palette.len() as u16;
            palette.push(id);
        }
    }

    match palette.len() {
        1 => write_single(out, palette[0]),
        2..=256 => {
            let bits: u32 = match palette.len() {
                2..=16 => 4,
                17..=32 => 5,
                33..=64 => 6,
                65..=128 => 7,
                _ => 8,
            };
            out.push(bits as u8);
            write_varint(out, palette.len() as i32);
            for &id in &palette {
                write_varint(out, id as i32);
            }
            let local: Vec<u32> = ids.iter().map(|&id| remap[id as usize] as u32).collect();
            write_packed(out, &local, bits);
        }
        _ => {
            out.push(15); // direct: global ids, 15 bits each
            write_packed(out, ids, 15);
        }
    }
}

/// Encode 64 biome ids as a wire biome container.
///
/// BPE rules (doc §7.4): 1 unique → 0; 2..8 → indirect BPE 1..3; >8 → direct
/// BPE 6 (64 registry entries, ceil(log2(64)) = 6).
fn write_biomes(out: &mut Vec<u8>, ids: &[u32; BIOMES_PER_SECTION]) {
    let mut palette: Vec<u32> = Vec::new();
    let mut remap = [u8::MAX; 64];
    for &id in ids {
        let slot = &mut remap[id as usize];
        if *slot == u8::MAX {
            *slot = palette.len() as u8;
            palette.push(id);
        }
    }

    match palette.len() {
        1 => write_single(out, palette[0]),
        2..=8 => {
            let bits: u32 = match palette.len() {
                2 => 1,
                3..=4 => 2,
                _ => 3,
            };
            out.push(bits as u8);
            write_varint(out, palette.len() as i32);
            for &id in &palette {
                write_varint(out, id as i32);
            }
            let local: Vec<u32> = ids.iter().map(|&id| remap[id as usize] as u32).collect();
            write_packed(out, &local, bits);
        }
        _ => {
            out.push(6); // direct: global biome ids, 6 bits each
            write_packed(out, ids, 6);
        }
    }
}

/// Write `Data Array Length` (VarInt long count) + the packed longs (BE).
fn write_packed(out: &mut Vec<u8>, values: &[u32], bits: u32) {
    let longs = pack(values, bits);
    write_varint(out, longs.len() as i32);
    for l in longs {
        out.extend_from_slice(&l.to_be_bytes());
    }
}

// ---------------------------------------------------------------------------
// Bit packing (1.16+ NON-STRADDLING format, doc §7.4)
// ---------------------------------------------------------------------------

/// Longs needed to hold `count` values at `bits` per value (values never
/// cross a long boundary: `entries_per_long = floor(64 / bits)`).
fn packed_len(count: usize, bits: u32) -> usize {
    let epl = (64 / bits) as usize;
    count.div_ceil(epl)
}

/// Pack values into big-endian longs, non-straddling.
fn pack(values: &[u32], bits: u32) -> Vec<i64> {
    let epl = (64 / bits) as usize;
    let mask = (1u64 << bits) - 1;
    let mut longs = vec![0i64; values.len().div_ceil(epl)];
    for (i, &v) in values.iter().enumerate() {
        let bit_index = (i % epl) * bits as usize;
        longs[i / epl] |= (((v as u64) & mask) << bit_index) as i64;
    }
    longs
}

/// Unpack `count` values from non-straddling packed longs.
fn unpack(data: &[i64], bits: u32, count: usize) -> Vec<u32> {
    let epl = (64 / bits) as usize;
    let mask = (1u64 << bits) - 1;
    (0..count)
        .map(|i| {
            let l = data.get(i / epl).copied().unwrap_or(0) as u64;
            let bit_index = (i % epl) * bits as usize;
            ((l >> bit_index) & mask) as u32
        })
        .collect()
}

/// Decode an on-disk paletted container's `data` into palette indices.
///
/// On-disk BPE = `max(min_bits, ceil(log2(palette_len)))` — block states use
/// min 4, biomes min 1 (verified against real 1.20.1 region data); a single
/// entry palette stores no `data` at all. If the stored long count matches a
/// different width (corrupt or exotic data), fall back to the unique width
/// that does fit.
fn decode_disk_indices(container: &Value, entries: usize, min_bits: u32) -> Vec<u32> {
    let pal_len = match container.get("palette") {
        Some(Value::List(items)) => items.len(),
        _ => 0,
    };
    if pal_len <= 1 {
        return vec![0; entries];
    }
    let data = match container.get("data").and_then(Value::as_long_array) {
        Some(d) if !d.is_empty() => d,
        _ => return vec![0; entries],
    };

    let ceil_log2 = usize::BITS - (pal_len as usize - 1).leading_zeros();
    let want = min_bits.max(ceil_log2);
    if packed_len(entries, want) == data.len() {
        return unpack(data, want, entries);
    }
    // Fallback: the (unique) width whose long count matches the stored data.
    let mut found = 0u32;
    let mut matches = 0;
    for b in 1..=8u32 {
        if packed_len(entries, b) == data.len() {
            found = b;
            matches += 1;
        }
    }
    if matches == 1 {
        unpack(data, found, entries)
    } else {
        vec![0; entries]
    }
}

// ---------------------------------------------------------------------------
// Wire primitives
// ---------------------------------------------------------------------------

fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut u = v as u32;
    loop {
        let b = (u & 0x7f) as u8;
        u >>= 7;
        if u == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Heightmaps as network NBT (empty root name, doc §7.2): at least
/// `MOTION_BLOCKING`; `WORLD_SURFACE` included for vanilla parity.
fn heightmaps_nbt(packed: &[i64]) -> Vec<u8> {
    nbt::to_bytes(&Value::compound(vec![
        ("MOTION_BLOCKING", Value::LongArray(packed.to_vec())),
        ("WORLD_SURFACE", Value::LongArray(packed.to_vec())),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        for bits in [1u32, 2, 3, 4, 5, 6, 7, 8, 9, 15] {
            let n = 64;
            let mask = (1u32 << bits) - 1;
            let vals: Vec<u32> = (0..n).map(|i| (i as u32 * 7 + 3) & mask).collect();
            let packed = pack(&vals, bits);
            assert_eq!(packed.len(), packed_len(n, bits));
            assert_eq!(unpack(&packed, bits, n), vals);
        }
    }

    #[test]
    fn packing_is_non_straddling() {
        // 9 bits: 7 entries per long (63 used bits, 1 padding bit); the 8th
        // entry starts a new long and never straddles the boundary.
        let vals = vec![0x1ffu32; 8];
        let packed = pack(&vals, 9);
        assert_eq!(packed.len(), 2);
        let mut expected0 = 0u64;
        for i in 0..7 {
            expected0 |= 0x1ffu64 << (i * 9);
        }
        assert_eq!(packed[0], expected0 as i64);
        assert_eq!(packed[1], 0x1ff);
    }

    #[test]
    fn void_chunk_shape() {
        let p = void_chunk();
        // 8 bytes per empty section: i16 count + 2x (1 + VarInt + VarInt).
        assert_eq!(p.data.len(), SECTIONS as usize * 8);
        let root = nbt::read(&mut &p.heightmaps_nbt[..]).unwrap();
        for key in ["MOTION_BLOCKING", "WORLD_SURFACE"] {
            let arr = root.get(key).unwrap().as_long_array().unwrap();
            assert_eq!(arr.len(), packed_len(HEIGHTMAP_ENTRIES, HEIGHTMAP_BITS));
            assert!(arr.iter().all(|&l| l == 0));
        }
    }

    #[test]
    fn smoke_real_world() {
        // Real 1.20.1 Forge save copied read-only from the test box.
        let dir = std::path::Path::new("/tmp/opencode/world-test");
        if !dir.join("region/r.0.0.mca").exists() {
            eprintln!("skipping: /tmp/opencode/world-test not present");
            return;
        }
        let mut served = 0;
        for z in 0..32 {
            for x in 0..32 {
                if let Some(p) = load_chunk(dir, x, z) {
                    served += 1;
                    assert!(p.data.len() >= SECTIONS as usize * 8);
                    let root = nbt::read(&mut &p.heightmaps_nbt[..]).unwrap();
                    let mb = root.get("MOTION_BLOCKING").unwrap().as_long_array().unwrap();
                    assert_eq!(mb.len(), packed_len(HEIGHTMAP_ENTRIES, HEIGHTMAP_BITS));
                }
            }
        }
        assert!(served > 0, "expected at least one full chunk in region 0,0");
        let s = stats();
        assert!(s.chunks_parsed >= served as u64);
        println!(
            "smoke: served {served} chunks; parsed={} missing={} unknown_states={} dropped={:?}",
            s.chunks_parsed, s.chunks_missing, s.unknown_blockstates,
            s.dropped_blocks.iter().take(5).collect::<Vec<_>>()
        );
    }
}
