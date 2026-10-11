//! Cross-thread load statistics for the world layer (`WorldStats` snapshot).
//!
//! `load_chunk` is called from many connection threads, so counters are
//! atomics and the dropped-name histogram lives behind a mutex.

use super::WorldStats;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static REGIONS_READ: AtomicU64 = AtomicU64::new(0);
static CHUNKS_PARSED: AtomicU64 = AtomicU64::new(0);
static CHUNKS_MISSING: AtomicU64 = AtomicU64::new(0);
static UNKNOWN_BLOCKSTATES: AtomicU64 = AtomicU64::new(0);
static DROPPED: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

pub(crate) fn region_read() {
    REGIONS_READ.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn chunk_parsed() {
    CHUNKS_PARSED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn chunk_missing() {
    CHUNKS_MISSING.fetch_add(1, Ordering::Relaxed);
}

/// Count one dropped (non-vanilla / unknown) palette entry.
pub(crate) fn unknown_blockstate(name: &str) {
    UNKNOWN_BLOCKSTATES.fetch_add(1, Ordering::Relaxed);
    let mut guard = DROPPED.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    *map.entry(name.to_string()).or_insert(0) += 1;
}

/// Snapshot of all counters; `dropped_blocks` is the top 20 names by count.
pub(crate) fn snapshot() -> WorldStats {
    let dropped = {
        let guard = DROPPED.lock().unwrap();
        match guard.as_ref() {
            Some(map) => {
                let mut v: Vec<(String, u64)> = map.iter().map(|(k, &c)| (k.clone(), c)).collect();
                // count desc, then name asc for deterministic ordering
                v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                v.truncate(20);
                v
            }
            None => Vec::new(),
        }
    };
    WorldStats {
        regions_read: REGIONS_READ.load(Ordering::Relaxed),
        chunks_parsed: CHUNKS_PARSED.load(Ordering::Relaxed),
        chunks_missing: CHUNKS_MISSING.load(Ordering::Relaxed),
        unknown_blockstates: UNKNOWN_BLOCKSTATES.load(Ordering::Relaxed),
        dropped_blocks: dropped,
    }
}
