//! world: level.dat parsing, Anvil region reading, and 1.20.1 chunk wire payloads.

pub mod anvil;
pub mod blockstates;
pub mod chunk;
pub mod level;
mod stats;

use std::io;
use std::path::{Path, PathBuf};

/// Level metadata parsed from `world/level.dat`.
#[derive(Debug, Clone)]
pub struct LevelInfo {
    pub spawn_x: i32,
    pub spawn_y: i32,
    pub spawn_z: i32,
    pub seed: i64,
    pub gamemode: u8,
    pub difficulty: u8,
    pub hardcore: bool,
    pub raining: bool,
    pub thundering: bool,
    pub time: i64,
}

/// Everything the net layer needs for one Level Chunk With Light packet,
/// except the light arrays (the net layer writes trivial full-sky light).
pub struct ChunkPayload {
    /// Heightmaps NBT compound (empty root name), containing at least MOTION_BLOCKING.
    pub heightmaps_nbt: Vec<u8>,
    /// Chunk "data" segment: block count + sections + block entities (see doc §8).
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct WorldStats {
    pub regions_read: u64,
    pub chunks_parsed: u64,
    pub chunks_missing: u64,
    pub unknown_blockstates: u64,
    /// Top dropped non-vanilla block names (name, occurrences) for the load report.
    pub dropped_blocks: Vec<(String, u64)>,
}

/// Loaded world (read-only at runtime; shared across connection threads).
pub struct World {
    dir: PathBuf,
    pub level: LevelInfo,
}

impl World {
    /// Load level metadata from `world_dir` (expects `level.dat`; region dirs optional).
    pub fn load(world_dir: &Path) -> io::Result<World> {
        let level = level::LevelInfo::load(world_dir)?;
        Ok(World {
            dir: world_dir.to_path_buf(),
            level: LevelInfo {
                spawn_x: level.spawn_x,
                spawn_y: level.spawn_y,
                spawn_z: level.spawn_z,
                seed: level.seed,
                gamemode: level.gamemode,
                difficulty: level.difficulty,
                hardcore: level.hardcore,
                raining: level.raining,
                thundering: level.thundering,
                time: level.time,
            },
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Load chunk (x, z) from the Anvil region files. `None` = chunk absent.
    pub fn chunk(&self, x: i32, z: i32) -> Option<ChunkPayload> {
        chunk::load_chunk(&self.dir, x, z)
    }

    /// All-void chunk payload used when a chunk is absent.
    pub fn void_chunk(&self) -> ChunkPayload {
        chunk::void_chunk()
    }

    /// Aggregated load statistics (parses happen lazily; this reports what was seen).
    pub fn stats(&self) -> WorldStats {
        chunk::stats()
    }
}
