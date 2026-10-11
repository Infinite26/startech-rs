use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Mutex;

/// Shared server state, visible to console and connection threads.
pub struct ServerState {
    pub running: AtomicBool,
    pub online: AtomicUsize,
    pub players: Mutex<Vec<String>>,
    /// Managed-forge backend status.
    pub backend_ready: AtomicBool,
    pub backend_note: Mutex<String>,
    pub proxied: AtomicUsize,
}

impl ServerState {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(true),
            online: AtomicUsize::new(0),
            players: Mutex::new(Vec::new()),
            backend_ready: AtomicBool::new(false),
            backend_note: Mutex::new("n/a".to_string()),
            proxied: AtomicUsize::new(0),
        }
    }

    pub fn join(&self, name: &str) {
        self.online.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut p) = self.players.lock() {
            p.push(name.to_string());
        }
    }

    pub fn leave(&self, name: &str) {
        self.online.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut p) = self.players.lock() {
            p.retain(|n| n != name);
        }
    }

    pub fn player_list(&self) -> Vec<String> {
        self.players.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn set_backend_note(&self, note: impl Into<String>) {
        if let Ok(mut n) = self.backend_note.lock() {
            *n = note.into();
        }
    }

    pub fn backend_note(&self) -> String {
        self.backend_note.lock().map(|n| n.clone()).unwrap_or_default()
    }
}
