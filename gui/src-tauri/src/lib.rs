//! MSS launcher backend.
//!
//! Hosts the Tauri runtime, spawns the bundled `p2p-screenshare` CLI when
//! the user starts a share/view session, and re-emits the CLI's stdout as
//! structured events the React frontend listens to.

mod session;

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- shared state ----------

#[derive(Default)]
struct RunnerState {
    child: Mutex<Option<Child>>,
}

// ---------- serializable payloads ----------

#[derive(Serialize, Deserialize)]
struct DisplayInfo {
    id: usize,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct ShareCfg {
    port: u16,
    fps: u32,
    quality: u8,
    skip_unchanged: bool,
    displays: Vec<usize>,
}

#[derive(Serialize)]
struct DecodedAddr {
    host: String,
    port: u16,
}

#[derive(Serialize, Clone)]
struct LogPayload {
    line: String,
    kind: &'static str,
}

#[derive(Serialize, Clone)]
struct StatPayload {
    id: u8,
    fps: f32,
    kbps: f32,
}

#[derive(Serialize, Clone)]
struct ExitPayload {
    code: Option<i32>,
}

// ---------- helpers ----------

fn locate_cli(app: &AppHandle) -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "p2p-screenshare.exe"
    } else {
        "p2p-screenshare"
    };

    // 1. Explicit override for development.
    if let Ok(p) = std::env::var("MSS_CLI") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    // 2. Bundled as a Tauri resource (production install).
    if let Ok(p) = app
        .path()
        .resolve(format!("bin/{exe_name}"), tauri::path::BaseDirectory::Resource)
    {
        if p.is_file() {
            return Some(p);
        }
    }

    // 3. Sibling to the launcher executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let cand = parent.join(exe_name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }

    // 4. Dev-mode fallback: the redirected build target the workspace's
    //    .cargo/config.toml uses (path with spaces in the repo root).
    let dev_candidates = [
        PathBuf::from("C:/rust-build/p2p-screenshare-target/release").join(exe_name),
        PathBuf::from("../../target/release").join(exe_name),
        PathBuf::from("../target/release").join(exe_name),
    ];
    for cand in dev_candidates {
        if cand.is_file() {
            return Some(cand);
        }
    }

    None
}

fn stats_regex() -> &'static Regex {
    use std::sync::OnceLock;
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| Regex::new(r"m(\d+):\s*([0-9.]+)fps\s+([0-9.]+)KB/s").unwrap())
}

fn pump<R: Read + Send + 'static>(
    app: AppHandle,
    r: R,
    kind: &'static str,
    parse_stats: bool,
) {
    thread::spawn(move || {
        let r = BufReader::new(r);
        for line in r.lines() {
            let Ok(line) = line else { return };
            if parse_stats {
                for cap in stats_regex().captures_iter(&line) {
                    let id: u8 = cap[1].parse().unwrap_or(0);
                    let fps: f32 = cap[2].parse().unwrap_or(0.0);
                    let kbps: f32 = cap[3].parse().unwrap_or(0.0);
                    let _ = app.emit("stat", StatPayload { id, fps, kbps });
                }
            }
            let _ = app.emit(
                "log",
                LogPayload {
                    line,
                    kind,
                },
            );
        }
    });
}

fn watch_exit(app: AppHandle, state: Arc<RunnerState>) {
    thread::spawn(move || {
        // Poll the child's exit status; once it's gone, clear state + notify.
        loop {
            thread::sleep(std::time::Duration::from_millis(200));
            let mut guard = state.child.lock();
            let Some(child) = guard.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    *guard = None;
                    drop(guard);
                    let _ = app.emit("child_exited", ExitPayload { code });
                    let _ = app.emit(
                        "log",
                        LogPayload {
                            line: format!("[gui] child exited with status {status}"),
                            kind: "gui",
                        },
                    );
                    return;
                }
                Ok(None) => continue,
                Err(e) => {
                    *guard = None;
                    drop(guard);
                    let _ = app.emit(
                        "log",
                        LogPayload {
                            line: format!("[gui] wait error: {e}"),
                            kind: "gui",
                        },
                    );
                    let _ = app.emit("child_exited", ExitPayload { code: None });
                    return;
                }
            }
        }
    });
}

// ---------- commands ----------

#[tauri::command]
fn list_displays(app: AppHandle) -> Result<Vec<DisplayInfo>, String> {
    let cli = locate_cli(&app).ok_or_else(|| "p2p-screenshare binary not found".to_string())?;
    let out = Command::new(&cli)
        .arg("displays")
        .output()
        .map_err(|e| format!("spawn failed: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        return Err(format!("displays exited {}: {stderr}", out.status));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json = stdout.trim().lines().last().unwrap_or("[]");
    serde_json::from_str::<Vec<DisplayInfo>>(json)
        .map_err(|e| format!("parse displays JSON: {e}"))
}

#[tauri::command]
fn current_code(port: u16) -> Option<String> {
    let ip = session::local_lan_ip()?;
    Some(session::encode(ip, port))
}

#[tauri::command]
fn decode_code(code: String) -> Option<DecodedAddr> {
    session::decode(&code).map(|(ip, port)| DecodedAddr {
        host: ip.to_string(),
        port,
    })
}

#[tauri::command]
fn is_running(state: State<'_, Arc<RunnerState>>) -> bool {
    let mut guard = state.child.lock();
    match guard.as_mut() {
        Some(c) => match c.try_wait() {
            Ok(Some(_)) => {
                *guard = None;
                false
            }
            _ => true,
        },
        None => false,
    }
}

#[tauri::command]
fn start_share(
    app: AppHandle,
    state: State<'_, Arc<RunnerState>>,
    cfg: ShareCfg,
) -> Result<(), String> {
    {
        let guard = state.child.lock();
        if guard.is_some() {
            return Err("a session is already running".into());
        }
    }
    let cli = locate_cli(&app).ok_or_else(|| "p2p-screenshare binary not found".to_string())?;
    let bind = format!("0.0.0.0:{}", cfg.port);
    let mut cmd = Command::new(&cli);
    cmd.arg("share")
        .arg("--bind")
        .arg(&bind)
        .arg("--fps")
        .arg(cfg.fps.to_string())
        .arg("--quality")
        .arg(cfg.quality.to_string())
        .arg("--skip-unchanged")
        .arg(cfg.skip_unchanged.to_string());
    if !cfg.displays.is_empty() {
        let list = cfg
            .displays
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(",");
        cmd.arg("--displays").arg(list);
    }
    spawn_and_track(app, state.inner().clone(), cmd, "share")
}

#[tauri::command]
fn start_view(
    app: AppHandle,
    state: State<'_, Arc<RunnerState>>,
    connect: String,
) -> Result<(), String> {
    {
        let guard = state.child.lock();
        if guard.is_some() {
            return Err("a session is already running".into());
        }
    }
    let cli = locate_cli(&app).ok_or_else(|| "p2p-screenshare binary not found".to_string())?;
    let mut cmd = Command::new(&cli);
    cmd.arg("view").arg("--connect").arg(connect);
    spawn_and_track(app, state.inner().clone(), cmd, "view")
}

#[tauri::command]
fn stop(state: State<'_, Arc<RunnerState>>) -> Result<(), String> {
    let mut guard = state.child.lock();
    if let Some(mut c) = guard.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(())
}

fn spawn_and_track(
    app: AppHandle,
    state: Arc<RunnerState>,
    mut cmd: Command,
    mode: &'static str,
) -> Result<(), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW — keep child silent on Windows release builds.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let _ = app.emit(
        "log",
        LogPayload {
            line: format!("[gui] launching {mode}"),
            kind: "gui",
        },
    );

    if let Some(out) = child.stdout.take() {
        pump(app.clone(), out, "stdout", true);
    }
    if let Some(err) = child.stderr.take() {
        pump(app.clone(), err, "stderr", true);
    }

    *state.child.lock() = Some(child);
    watch_exit(app, state);
    Ok(())
}

// ---------- entry point ----------

pub fn run() {
    let runner = Arc::new(RunnerState::default());

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(runner)
        .invoke_handler(tauri::generate_handler![
            list_displays,
            current_code,
            decode_code,
            start_share,
            start_view,
            stop,
            is_running,
        ])
        .setup(|app| {
            let _ = app
                .get_webview_window("main")
                .map(|w| w.set_title("MSS — Multi-Screen Stream"));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
