//! `world/level.dat` parsing (gzip NBT) → `LevelInfo`.

use crate::nbt::{self, Value};
use flate2::read::GzDecoder;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

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

impl LevelInfo {
    /// Load `world_dir/level.dat` (gzip NBT; raw NBT also accepted).
    pub fn load(world_dir: &Path) -> io::Result<LevelInfo> {
        let raw = fs::read(world_dir.join("level.dat"))?;
        let root = read_nbt_auto(&raw)?;
        // Most fields live under `Data`; tolerate a flattened root as well.
        let data = root.get("Data").unwrap_or(&root);

        // Seed: 1.16+ `WorldGenSettings.seed`, legacy `RandomSeed`.
        let seed = data
            .get("WorldGenSettings")
            .and_then(|w| w.get("seed"))
            .and_then(Value::as_i64)
            .or_else(|| data.get("RandomSeed").and_then(Value::as_i64))
            .unwrap_or(0);

        Ok(LevelInfo {
            spawn_x: data.get("SpawnX").and_then(Value::as_i32).unwrap_or(0),
            spawn_y: data.get("SpawnY").and_then(Value::as_i32).unwrap_or(64),
            spawn_z: data.get("SpawnZ").and_then(Value::as_i32).unwrap_or(0),
            seed,
            gamemode: data.get("GameType").and_then(Value::as_i32).unwrap_or(0) as u8,
            difficulty: data.get("Difficulty").and_then(Value::as_i64).unwrap_or(2) as u8,
            hardcore: data
                .get("hardcore")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
                .unwrap_or(false),
            raining: data
                .get("raining")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
                .unwrap_or(false),
            thundering: data
                .get("thundering")
                .and_then(Value::as_i64)
                .map(|v| v != 0)
                .unwrap_or(false),
            time: data.get("DayTime").and_then(Value::as_i64).unwrap_or(0),
        })
    }
}

/// Parse NBT that may (standard) or may not (rare) be gzip-wrapped.
fn read_nbt_auto(raw: &[u8]) -> io::Result<Value> {
    if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut buf = Vec::new();
        GzDecoder::new(raw).read_to_end(&mut buf)?;
        nbt::read(&mut &buf[..])
    } else {
        nbt::read(&mut &raw[..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_real_level_dat() {
        // Real 1.20.1 Forge save copied read-only from the test box.
        let dir = std::path::Path::new("/tmp/opencode/world-test");
        if !dir.join("level.dat").exists() {
            eprintln!("skipping: /tmp/opencode/world-test not present");
            return;
        }
        let li = LevelInfo::load(dir).unwrap();
        assert_eq!((li.spawn_x, li.spawn_y, li.spawn_z), (6, 67, 5));
        assert_eq!(li.seed, 8396522220033743855);
        assert_eq!(li.gamemode, 0);
        assert_eq!(li.difficulty, 0);
        assert!(!li.hardcore && !li.raining && !li.thundering);
        assert_eq!(li.time, 870000);
    }
}
