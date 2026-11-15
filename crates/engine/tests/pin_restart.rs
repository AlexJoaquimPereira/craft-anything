//! A Linux pin's program is remembered across an app restart by `pin.json`'s pid and start time.
//! The session's run list is empty in the new engine, so only the pin's record can find it.

#![cfg(target_os = "linux")]

use craft_anything_engine::workspace::{TargetRef, Workspace};
use craft_anything_engine::{CallOptions, Caller, Consent, Engine, pins};
use craft_anything_platform::process::Launch;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

fn call(engine: &Engine, id: &str, params: Value) -> Value {
    engine.call(id, params, CallOptions::new(Caller::Cli).with_consent(Consent::Granted)).unwrap().to_json()["result"].clone()
}

fn workspace(dir: &Path) -> Workspace {
    let ws = Workspace::create(dir, "Game", "a game", TargetRef::None).unwrap();
    let app = ws.app_dir();
    std::fs::create_dir_all(app.join("target/debug")).unwrap();
    std::fs::write(app.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
    let program = app.join("target/debug/game");
    std::fs::write(&program, b"program").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws
}

/// `/proc/<pid>/stat` field 22: the start time the production reader uses.
fn start_token(pid: u32) -> String {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let (_, rest) = text.rsplit_once(')').unwrap();
    rest.split_whitespace().nth(19).unwrap().to_string()
}

/// Make a pin, start a stand-in from its app folder, and write the v3 record a restart would read.
fn pinned_with_run(ws: &Workspace) -> (u32, std::path::PathBuf, craft_anything_platform::Launched) {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let made = call(&engine, "project_pin_create", json!({"project": ws.root}));
    let id = made["pin"]["id"].as_u64().unwrap() as u32;
    let pin = pins::get(ws, id).unwrap();
    let file = pin.dir(ws).join(pins::PIN_FILE);
    let running = Launch::new("sh").args(["-c", "sleep 30"]).cwd(pin.app_dir(ws)).start_watched().unwrap();
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    doc["version"] = json!(3);
    doc["runs"] = json!([{"pid": running.pid(), "start": start_token(running.pid())}]);
    std::fs::write(&file, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    (id, file, running)
}

#[test]
fn a_pin_running_before_the_restart_is_listed_and_can_be_stopped() {
    let d = tempfile::tempdir().unwrap();
    let ws = workspace(d.path());
    let (id, file, running) = pinned_with_run(&ws);

    // A fresh engine stands in for the restarted app: its session run list knows nothing.
    let restarted = Engine::new(craft_anything_engine::testing::session());
    let list = call(&restarted, "project_pins_list", json!({"project": ws.root}));
    assert_eq!(list["pins"][0]["running"], true, "{list}");
    assert!(list["pins"][0]["pids"].as_array().unwrap().iter().any(|p| p.as_u64() == Some(u64::from(running.pid()))), "{list}");

    let stopped = call(&restarted, "project_pin_stop", json!({"project": ws.root, "pin": id}));
    assert!(stopped["stopped"].as_u64().unwrap() >= 1, "{stopped}");
    assert!(running.wait_timeout(Duration::from_secs(2)).is_some(), "the recorded process is gone");

    let after = call(&restarted, "project_pins_list", json!({"project": ws.root}));
    assert_eq!(after["pins"][0]["running"], false, "{after}");
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["runs"], json!([]), "the stopped run is forgotten");
}

#[test]
fn a_different_start_time_is_neither_believed_nor_stopped() {
    let d = tempfile::tempdir().unwrap();
    let ws = workspace(d.path());
    let (id, file, running) = pinned_with_run(&ws);
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    doc["runs"][0]["start"] = json!("0");
    std::fs::write(&file, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();

    let restarted = Engine::new(craft_anything_engine::testing::session());
    let list = call(&restarted, "project_pins_list", json!({"project": ws.root}));
    assert_eq!(list["pins"][0]["running"], false, "a reused pid isn't the pin's process: {list}");
    let stopped = call(&restarted, "project_pin_stop", json!({"project": ws.root, "pin": id}));
    assert_eq!(stopped["stopped"], 0, "a wrong start token never kills: {stopped}");
    assert!(running.is_running(), "the stand-in is still there");
    running.stop();
}
