mod biomes;
mod config;
mod forge;
mod manage;
mod nbt;
mod net;
mod registry;
mod state;
mod world;

use std::io::{self, BufRead};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

fn main() {
    let cfg_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "server.properties".into());
    let config = match config::Config::load(Path::new(&cfg_path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read {cfg_path}: {e}");
            return;
        }
    };

    if !Path::new("eula.txt").exists() || !std::fs::read_to_string("eula.txt").map(|s| s.contains("eula=true")).unwrap_or(false) {
        eprintln!("You need to agree to the EULA in eula.txt (eula=true) to run this server.");
        return;
    }

    let state = Arc::new(state::ServerState::new());
    let addr = format!("0.0.0.0:{}", config.port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[startech-rs] cannot bind {addr}: {e}");
            return;
        }
    };

    // ── Managed Forge mode: supervise a real Forge server, proxy players ──
    let managed = manage::ManagedConfig::from_config(&config, Path::new("."));
    let supervisor = manage::start_supervisor(managed.clone(), Arc::clone(&state));
    if supervisor.is_some() {
        eprintln!(
            "[startech-rs] MANAGED mode: public :{} -> backend {}:{} ({})",
            config.port,
            managed.internal_ip,
            managed.internal_port,
            managed.dir.display()
        );
    }

    // World is only needed for standalone (clean-room) mode.
    let world: Option<Arc<world::World>> = if supervisor.is_some() {
        None
    } else {
        let world_dir = Path::new(&config.level_name);
        eprintln!("[startech-rs] loading world from {} ...", world_dir.display());
        match world::World::load(world_dir) {
            Ok(w) => {
                eprintln!(
                    "[startech-rs] world ready: spawn=({}, {}, {}) seed={} gamemode={}",
                    w.level.spawn_x, w.level.spawn_y, w.level.spawn_z, w.level.seed, config.gamemode
                );
                Some(Arc::new(w))
            }
            Err(e) => {
                eprintln!("[startech-rs] world load failed: {e}");
                return;
            }
        }
    };

    eprintln!("[startech-rs] listening on {addr} (Minecraft 1.20.1 / protocol 763, offline)");

    let startup_note = managed.startup_note.clone();
    let backend_addr = managed.backend_addr();

    {
        let state = Arc::clone(&state);
        let port = config.port;
        let sup = supervisor.clone();
        thread::spawn(move || console_loop(state, port, sup));
    }

    for stream in listener.incoming() {
        if !state.running.load(Ordering::Relaxed) {
            break;
        }
        match stream {
            Ok(stream) => {
                let config = config.clone();
                let state = Arc::clone(&state);
                if supervisor.is_some() {
                    let backend = backend_addr;
                    let notice = startup_note.clone();
                    thread::spawn(move || {
                        if state.backend_ready.load(Ordering::Relaxed) {
                            if let Err(e) = manage::proxy_connection(stream, backend, &state) {
                                eprintln!("[startech-rs] proxy error: {e}");
                            }
                        } else {
                            if let Err(e) = net::serve_startup(stream, &config, &state, &notice) {
                                eprintln!("[startech-rs] startup connection error: {e}");
                            }
                        }
                    });
                } else if let Some(ref world) = world {
                    let world = Arc::clone(world);
                    thread::spawn(move || {
                        let peer = stream.peer_addr().ok();
                        let result = net::handle_client(stream, Arc::clone(&world), config, state.clone());
                        let s = world.stats();
                        eprintln!(
                            "[startech-rs] {peer:?} done; world stats: chunks_parsed={} chunks_missing={} unknown_blockstates={} dropped_top={:?}",
                            s.chunks_parsed, s.chunks_missing, s.unknown_blockstates,
                            s.dropped_blocks.iter().take(8).collect::<Vec<_>>()
                        );
                        if let Err(e) = result {
                            eprintln!("[startech-rs] connection {peer:?} ended with error: {e}");
                        }
                    });
                }
            }
            Err(e) => eprintln!("[startech-rs] accept error: {e}"),
        }
    }

    eprintln!("[startech-rs] server stopped");
}

fn console_loop(
    state: Arc<state::ServerState>,
    port: u16,
    sup: Option<manage::SupervisorHandle>,
) {
    let stdin = io::stdin();
    // NOTE: stdin may be a fifo whose writers come and go; when the last
    // writer closes we get EOF, but we must NOT exit — just wait for input.
    loop {
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if handle_console_line(&line, &state, port, &sup) {
                return;
            }
        }
        if !state.running.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Handle one console line. Returns true when the server should shut down.
fn handle_console_line(
    line: &str,
    state: &Arc<state::ServerState>,
    port: u16,
    sup: &Option<manage::SupervisorHandle>,
) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    if let Some(ref sup) = sup {
        // Managed mode: #cmd = Rust-side, anything else -> backend stdin.
        if let Some(cmd) = line.strip_prefix('#') {
            match cmd.trim() {
                "stop" => {
                    eprintln!("[startech-rs] stopping (backend + proxy)...");
                    sup.request_stop_all();
                    state.running.store(false, Ordering::Relaxed);
                    if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                        drop(s);
                    }
                    return true;
                }
                "status" => {
                    let ready = state.backend_ready.load(Ordering::Relaxed);
                    println!(
                        "mode=managed backend_ready={} backend_note={} restarts={} proxied={} players={:?}",
                        ready,
                        state.backend_note(),
                        sup.restarts(),
                        state.proxied.load(Ordering::Relaxed),
                        state.player_list()
                    );
                }
                "list" => {
                    let players = state.player_list();
                    println!("online ({}): {}", players.len(), players.join(", "));
                }
                "restart" => {
                    eprintln!("[startech-rs] restarting backend...");
                    sup.stop_backend();
                }
                other => println!("rust commands: #stop #status #list #restart (other lines go to the backend) — got: {other}"),
            }
            return false;
        }
        if !sup.forward(&line) {
            println!("backend not running; line dropped. Prefix rust commands with #.");
        }
        return false;
    }
    match line {
        "stop" => {
            state.running.store(false, Ordering::Relaxed);
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                drop(s);
            }
            return true;
        }
        "list" => {
            let players = state.player_list();
            println!("online ({}): {}", players.len(), players.join(", "));
        }
        other => println!("unknown command: {other} (try: list, stop)"),
    }
    false
}
