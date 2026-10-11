//! Managed Forge mode: run a real Forge server as a supervised child process
//! and transparently proxy players to it once it is ready.
//!
//! The Rust process binds the PUBLIC port from second zero: it answers
//! status pings itself while Forge boots, then pipes every connection
//! byte-for-byte to the backend. Any mods work because Forge runs them.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::state::ServerState;

const CONTROL_RUN: u8 = 0;
const CONTROL_STOP_ALL: u8 = 1;

/// Settings for managed mode, read from `server.properties` (`startech-*` keys).
#[derive(Debug, Clone)]
pub struct ManagedConfig {
    pub enabled: bool,
    pub dir: PathBuf,
    pub java: String,
    pub jvm_args: Vec<String>,
    pub internal_port: u16,
    pub internal_ip: String,
    pub auto_restart: bool,
    pub max_restarts: u32,
    pub stop_timeout: Duration,
    pub startup_note: String,
}

impl ManagedConfig {
    pub fn from_config(cfg: &Config, runtime_dir: &Path) -> ManagedConfig {
        ManagedConfig {
            enabled: cfg.forge_enabled,
            dir: if Path::new(&cfg.forge_dir).is_absolute() {
                PathBuf::from(&cfg.forge_dir)
            } else {
                runtime_dir.join(&cfg.forge_dir)
            },
            java: cfg.forge_java.clone(),
            jvm_args: cfg
                .forge_jvm_args
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            internal_port: cfg.forge_internal_port,
            internal_ip: cfg.forge_internal_ip.clone(),
            auto_restart: cfg.forge_auto_restart,
            max_restarts: cfg.forge_max_restarts,
            stop_timeout: Duration::from_secs(cfg.forge_stop_timeout_secs),
            startup_note: cfg.forge_startup_note.clone(),
        }
    }

    /// Backend socket address players get proxied to.
    pub fn backend_addr(&self) -> SocketAddr {
        format!("{}:{}", self.internal_ip, self.internal_port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], self.internal_port)))
    }
}

/// Handle to a running supervisor (shared with the console thread).
#[derive(Clone)]
pub struct SupervisorHandle {
    control: Arc<AtomicU8>,
    stdin_slot: Arc<Mutex<Option<ChildStdin>>>,
    restarts: Arc<std::sync::atomic::AtomicU32>,
}

impl SupervisorHandle {
    /// Forward a console line to the backend's stdin. False if no backend.
    pub fn forward(&self, line: &str) -> bool {
        if let Ok(mut slot) = self.stdin_slot.lock() {
            if let Some(stdin) = slot.as_mut() {
                return stdin
                    .write_all(format!("{line}\n").as_bytes())
                    .and_then(|_| stdin.flush())
                    .is_ok();
            }
        }
        false
    }

    /// Ask the backend to stop (the supervisor decides restart vs shutdown).
    pub fn stop_backend(&self) {
        self.forward("stop");
    }

    pub fn request_stop_all(&self) {
        self.control.store(CONTROL_STOP_ALL, Ordering::Relaxed);
        self.forward("stop");
    }

    pub fn restarts(&self) -> u32 {
        self.restarts.load(Ordering::Relaxed)
    }
}

/// Validate that `dir` looks like an installed Forge server. Returns the
/// unix_args path to launch with.
fn find_unix_args(dir: &Path) -> io::Result<PathBuf> {
    for loader in ["forge", "neoforge"] {
        let base = dir.join("libraries").join("net").join("minecraftforge").join(loader);
        if let Ok(rd) = fs::read_dir(&base) {
            let mut vers: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("unix_args.txt").exists())
                .collect();
            vers.sort();
            if let Some(v) = vers.pop() {
                return Ok(v.join("unix_args.txt"));
            }
        }
    }
    // NeoForge 1.20.2+ lives under net/neoforged/ instead.
    let base = dir
        .join("libraries")
        .join("net")
        .join("neoforged");
    for loader in ["forge", "neoforge"] {
        if let Ok(rd) = fs::read_dir(base.join(loader)) {
            let mut vers: Vec<PathBuf> = rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.join("unix_args.txt").exists())
                .collect();
            vers.sort();
            if let Some(v) = vers.pop() {
                return Ok(v.join("unix_args.txt"));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "no unix_args.txt found under {}/libraries (is a mod loader installed?)",
            dir.display()
        ),
    ))
}

/// Point the backend at the internal port/ip. Backs up server.properties once.
fn patch_server_properties(dir: &Path, port: u16, ip: &str) -> io::Result<()> {
    let path = dir.join("server.properties");
    let bak = dir.join("server.properties.startech-bak");
    let text = fs::read_to_string(&path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    if !bak.exists() {
        fs::write(&bak, &text)?;
        eprintln!("[startech-rs] backed up {} -> {}", path.display(), bak.display());
    }
    let mut has_port = false;
    let mut has_ip = false;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with("server-port=") {
            out.push(format!("server-port={port}"));
            has_port = true;
        } else if line.starts_with("server-ip=") {
            out.push(format!("server-ip={ip}"));
            has_ip = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !has_port {
        out.push(format!("server-port={port}"));
    }
    if !has_ip {
        out.push(format!("server-ip={ip}"));
    }
    out.push(String::new());
    fs::write(&path, out.join("\n"))?;
    Ok(())
}

/// Is `line` a DedicatedServer chat/log line carrying `marker`? If so, the player name.
fn player_event(line: &str, marker: &str) -> Option<String> {
    let idx = line.find(marker)?;
    let before = &line[..idx];
    // Format: "[...] [Server thread/INFO] [minecraft/DedicatedServer/]: <name> joined..."
    let name = before.rsplit("]: ").next()?.trim();
    if name.is_empty() || name.contains(' ') || name.len() > 32 {
        return None;
    }
    Some(name.to_string())
}

fn pump_stream<R: io::Read + Send + 'static>(
    reader: R,
    state: Arc<ServerState>,
    stderr: bool,
) {
    thread::spawn(move || {
        let tag = if stderr { "forge-err" } else { "forge" };
        let mut lines = BufReader::new(reader).lines();
        while let Some(Ok(line)) = lines.next() {
            eprintln!("[{tag}] {line}");
            if line.contains("Done (") && line.contains("For help, type") {
                state.backend_ready.store(true, Ordering::Relaxed);
                state.set_backend_note("online");
                eprintln!("[startech-rs] backend is ready");
            }
            if let Some(name) = player_event(&line, " joined the game") {
                eprintln!("[startech-rs] {name} joined via backend");
                state.join(&name);
            } else if let Some(name) = player_event(&line, " left the game") {
                eprintln!("[startech-rs] {name} left backend");
                state.leave(&name);
            }
            if line.contains("You need to agree to the EULA") {
                state.set_backend_note("eula-missing");
            }
        }
    });
}

/// Spawn one backend run. Returns the child on success.
fn spawn_backend(cfg: &ManagedConfig, unix_args: &Path) -> io::Result<Child> {
    let at_user = format!("@{}", cfg.dir.join("user_jvm_args.txt").display());
    let at_unix = format!("@{}", unix_args.display());
    eprintln!(
        "[startech-rs] launching backend: {} {} {}{} nogui (cwd={})",
        cfg.java,
        at_user,
        if cfg.jvm_args.is_empty() {
            String::new()
        } else {
            format!("{} ", cfg.jvm_args.join(" "))
        },
        at_unix,
        cfg.dir.display()
    );
    Command::new(&cfg.java)
        .arg(at_user)
        // Extra JVM flags must sit BEFORE @unix_args.txt: that argfile carries
        // the main class and --launchTarget, so the JVM only treats leading
        // options as its own. Appending them after it hands them to the
        // application, which rejects them ("X is not a recognized option").
        .args(&cfg.jvm_args)
        .arg(at_unix)
        .arg("nogui")
        .current_dir(&cfg.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// Main supervisor loop. Runs on its own thread until StopAll or fatal.
fn supervise(cfg: ManagedConfig, state: Arc<ServerState>, handle: SupervisorHandle) {
    if !cfg.dir.join("user_jvm_args.txt").exists() {
        state.set_backend_note("missing user_jvm_args.txt");
        eprintln!(
            "[startech-rs] managed mode: {}/user_jvm_args.txt not found — backend disabled",
            cfg.dir.display()
        );
        return;
    }
    let unix_args = match find_unix_args(&cfg.dir) {
        Ok(p) => p,
        Err(e) => {
            state.set_backend_note("no mod loader installed");
            eprintln!("[startech-rs] managed mode: {e} — backend disabled");
            return;
        }
    };
    if let Err(e) = patch_server_properties(&cfg.dir, cfg.internal_port, &cfg.internal_ip) {
        state.set_backend_note("cannot patch server.properties");
        eprintln!("[startech-rs] managed mode: {e} — backend disabled");
        return;
    }

    let mut restarts: u32 = 0;
    let mut remind_at = Instant::now() + Duration::from_secs(60);
    loop {
        if handle.control.load(Ordering::Relaxed) == CONTROL_STOP_ALL {
            state.set_backend_note("stopped");
            break;
        }
        state.backend_ready.store(false, Ordering::Relaxed);
        state.set_backend_note("starting");
        let started_at = Instant::now();
        let mut child = match spawn_backend(&cfg, &unix_args) {
            Ok(c) => c,
            Err(e) => {
                state.set_backend_note(format!("spawn failed: {e}"));
                eprintln!("[startech-rs] backend spawn failed: {e}");
                thread::sleep(Duration::from_secs(10));
                continue;
            }
        };
        if let Some(stdout) = child.stdout.take() {
            pump_stream(stdout, Arc::clone(&state), false);
        }
        if let Some(stderr) = child.stderr.take() {
            pump_stream(stderr, Arc::clone(&state), true);
        }
        if let Some(stdin) = child.stdin.take() {
            if let Ok(mut slot) = handle.stdin_slot.lock() {
                *slot = Some(stdin);
            }
        }

        // Wait for exit, honoring StopAll (send "stop", then kill on timeout).
        let mut stop_sent = false;
        let mut kill_at: Option<Instant> = None;
        let exit_status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("[startech-rs] backend wait error: {e}");
                    break None;
                }
            }
            if handle.control.load(Ordering::Relaxed) == CONTROL_STOP_ALL && !stop_sent {
                eprintln!("[startech-rs] asking backend to stop...");
                handle.stop_backend();
                stop_sent = true;
                kill_at = Some(Instant::now() + cfg.stop_timeout);
            }
            if let Some(deadline) = kill_at {
                if Instant::now() >= deadline {
                    eprintln!("[startech-rs] backend did not stop in time; killing");
                    let _ = child.kill();
                    break child.wait().ok();
                }
            }
            if Instant::now() >= remind_at
                && !state.backend_ready.load(Ordering::Relaxed)
            {
                eprintln!(
                    "[startech-rs] still waiting for backend ({}s)...",
                    started_at.elapsed().as_secs()
                );
                remind_at = Instant::now() + Duration::from_secs(60);
            }
            thread::sleep(Duration::from_millis(500));
        };
        if let Ok(mut slot) = handle.stdin_slot.lock() {
            *slot = None;
        }
        state.backend_ready.store(false, Ordering::Relaxed);

        if handle.control.load(Ordering::Relaxed) == CONTROL_STOP_ALL {
            state.set_backend_note("stopped");
            eprintln!("[startech-rs] backend stopped (status: {exit_status:?})");
            break;
        }
        let uptime = started_at.elapsed();
        eprintln!("[startech-rs] backend exited unexpectedly after {uptime:?} (status: {exit_status:?})");
        if uptime < Duration::from_secs(30) {
            eprintln!("[startech-rs] backend died fast — check the [forge] log lines above (bad java? missing eula? mod crash?)");
        }
        if cfg.auto_restart && restarts < cfg.max_restarts {
            restarts += 1;
            handle.restarts.store(restarts, Ordering::Relaxed);
            state.set_backend_note(format!("restarting ({restarts}/{})", cfg.max_restarts));
            thread::sleep(Duration::from_secs(5 * restarts as u64));
            continue;
        }
        state.set_backend_note("crashed");
        eprintln!("[startech-rs] backend restart budget exhausted — staying up for status pings only");
        break;
    }
}

/// Start the supervisor thread. Returns None when managed mode is disabled.
pub fn start_supervisor(
    cfg: ManagedConfig,
    state: Arc<ServerState>,
) -> Option<SupervisorHandle> {
    if !cfg.enabled {
        return None;
    }
    if cfg.internal_port == 0 {
        state.set_backend_note("startech-forge-internal-port not set");
        eprintln!("[startech-rs] managed mode enabled but startech-forge-internal-port is not set — backend disabled");
        return None;
    }
    let handle = SupervisorHandle {
        control: Arc::new(AtomicU8::new(CONTROL_RUN)),
        stdin_slot: Arc::new(Mutex::new(None)),
        restarts: Arc::new(std::sync::atomic::AtomicU32::new(0)),
    };
    let worker = handle.clone();
    let state_clone = Arc::clone(&state);
    thread::spawn(move || supervise(cfg, state_clone, worker));
    Some(handle)
}

/// Transparently proxy one client connection to the backend (both directions).
pub fn proxy_connection(
    client: TcpStream,
    backend_addr: SocketAddr,
    state: &Arc<ServerState>,
) -> io::Result<()> {
    let backend = TcpStream::connect(backend_addr)?;
    state.proxied.fetch_add(1, Ordering::Relaxed);
    let c2 = client.try_clone()?;
    let b2 = backend.try_clone()?;
    let t1 = thread::spawn(move || pipe(c2, backend));
    let t2 = thread::spawn(move || pipe(b2, client));
    let _ = t1.join();
    let _ = t2.join();
    state.proxied.fetch_sub(1, Ordering::Relaxed);
    Ok(())
}

fn pipe(mut reader: TcpStream, mut writer: TcpStream) {
    let _ = io::copy(&mut reader, &mut writer);
    let _ = reader.shutdown(Shutdown::Both);
    let _ = writer.shutdown(Shutdown::Both);
}
