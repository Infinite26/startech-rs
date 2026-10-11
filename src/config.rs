use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub motd: String,
    pub max_players: i32,
    pub view_distance: u8,
    pub simulation_distance: u8,
    pub level_name: String,
    pub gamemode: String,
    pub difficulty: String,
    pub hardcore: bool,
    pub online_mode: bool,
    // Managed-forge mode (Rust supervises a real Forge server; any mods work).
    pub forge_enabled: bool,
    // Native-forge mode (Rust speaks the FML handshake itself; no JVM, no mods).
    pub forge_native: bool,
    /// Extra JVM flags appended after the backend's argfiles, so heap can be
    /// tuned without editing the backend's own user_jvm_args.txt. The JVM honours
    /// the last occurrence of a flag, so these win.
    pub forge_jvm_args: String,
    pub forge_dir: String,
    pub forge_java: String,
    pub forge_internal_port: u16,
    pub forge_internal_ip: String,
    pub forge_auto_restart: bool,
    pub forge_max_restarts: u32,
    pub forge_stop_timeout_secs: u64,
    pub forge_startup_note: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: 25566,
            motd: "startech-rs - a 1.20.1 Rust server".into(),
            max_players: 20,
            view_distance: 8,
            simulation_distance: 8,
            level_name: "world".into(),
            gamemode: "survival".into(),
            difficulty: "normal".into(),
            hardcore: false,
            online_mode: false,
            forge_enabled: false,
            forge_native: false,
            forge_jvm_args: String::new(),
            forge_dir: ".".into(),
            forge_java: "java".into(),
            forge_internal_port: 0,
            forge_internal_ip: "127.0.0.1".into(),
            forge_auto_restart: true,
            forge_max_restarts: 3,
            forge_stop_timeout_secs: 30,
            forge_startup_note: "Server is starting — try again in a minute".into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> io::Result<Config> {
        let mut cfg = Config::default();
        if !path.exists() {
            return Ok(cfg);
        }
        let text = fs::read_to_string(path)?;
        let props: HashMap<String, String> = text
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') {
                    return None;
                }
                let (k, v) = l.split_once('=')?;
                Some((k.trim().to_string(), v.trim().to_string()))
            })
            .collect();

        if let Some(v) = props.get("server-port") {
            if let Ok(p) = v.parse() {
                cfg.port = p;
            }
        }
        if let Some(v) = props.get("motd") {
            cfg.motd = v.clone();
        }
        if let Some(v) = props.get("max-players") {
            if let Ok(n) = v.parse() {
                cfg.max_players = n;
            }
        }
        if let Some(v) = props.get("view-distance") {
            if let Ok(n) = v.parse::<u8>() {
                cfg.view_distance = n.clamp(2, 32);
            }
        }
        if let Some(v) = props.get("simulation-distance") {
            if let Ok(n) = v.parse::<u8>() {
                cfg.simulation_distance = n.clamp(2, 32);
            }
        }
        if let Some(v) = props.get("level-name") {
            cfg.level_name = v.clone();
        }
        if let Some(v) = props.get("gamemode") {
            cfg.gamemode = v.clone();
        }
        if let Some(v) = props.get("difficulty") {
            cfg.difficulty = v.clone();
        }
        if let Some(v) = props.get("hardcore") {
            cfg.hardcore = v == "true";
        }
        if let Some(v) = props.get("online-mode") {
            cfg.online_mode = v == "true";
        }
        if let Some(v) = props.get("startech-forge-enabled") {
            cfg.forge_enabled = v == "true";
        }
        if let Some(v) = props.get("startech-forge-native") {
            cfg.forge_native = v == "true";
        }
        if let Some(v) = props.get("startech-forge-jvm-args") {
            cfg.forge_jvm_args = v.clone();
        }
        if let Some(v) = props.get("startech-forge-dir") {
            cfg.forge_dir = v.clone();
        }
        if let Some(v) = props.get("startech-forge-java") {
            cfg.forge_java = v.clone();
        }
        if let Some(v) = props.get("startech-forge-internal-port") {
            if let Ok(p) = v.parse() {
                cfg.forge_internal_port = p;
            }
        }
        if let Some(v) = props.get("startech-forge-internal-ip") {
            cfg.forge_internal_ip = v.clone();
        }
        if let Some(v) = props.get("startech-forge-auto-restart") {
            cfg.forge_auto_restart = v != "false";
        }
        if let Some(v) = props.get("startech-forge-max-restarts") {
            if let Ok(n) = v.parse() {
                cfg.forge_max_restarts = n;
            }
        }
        if let Some(v) = props.get("startech-forge-stop-timeout-secs") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.forge_stop_timeout_secs = n.max(5);
            }
        }
        if let Some(v) = props.get("startech-forge-startup-note") {
            cfg.forge_startup_note = v.clone();
        }
        Ok(cfg)
    }

    pub fn gamemode_id(&self) -> u8 {
        match self.gamemode.as_str() {
            "creative" => 1,
            "adventure" => 2,
            "spectator" => 3,
            _ => 0,
        }
    }

    pub fn difficulty_id(&self) -> u8 {
        match self.difficulty.as_str() {
            "peaceful" => 0,
            "easy" => 1,
            "hard" => 3,
            _ => 2,
        }
    }
}
