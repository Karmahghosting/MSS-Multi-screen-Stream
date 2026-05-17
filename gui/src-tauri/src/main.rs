// Prevents an extra console window on Windows release builds — the launcher
// is a GUI app, not a CLI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    mss_gui_lib::run();
}
