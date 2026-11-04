//! The home, saved as `user://home.json` (written by Confirm in the world map). It is the only
//! source of the home. The old `DTE_HOME` variable and the repository's `home.local.json` are
//! imported once, when `user://home.json` does not exist yet; after that they are ignored.
use std::path::Path;

use godot::classes::file_access::ModeFlags;
use godot::classes::{FileAccess, ProjectSettings};
use godot::prelude::*;
use worldmap::home::{home_json, resolve, Home};

/// The saved home.
pub const HOME_PATH: &str = "user://home.json";
/// Old home file at the repository root (the Godot project is in `godot/`).
const LEGACY_FILE: &str = "home.local.json";
/// Old environment variable, `"lat,lon"` in degrees.
const LEGACY_ENV: &str = "DTE_HOME";

/// The home in use, or `None` when none was chosen yet. Imports the old sources once.
pub fn load() -> Option<Home> {
    let saved = read_text(HOME_PATH);
    let env = std::env::var(LEGACY_ENV).ok();
    let legacy = legacy_file_text();
    let resolved = resolve(saved.as_deref(), env.as_deref(), legacy.as_deref());
    if let Some(home) = resolved.import {
        match save(home) {
            Ok(()) => godot_print!(
                "home: imported once from {LEGACY_ENV} or {LEGACY_FILE} to {HOME_PATH} ({}, {})",
                home.lat,
                home.lon
            ),
            Err(()) => {
                godot_error!("home: could not save the imported home to {HOME_PATH}");
            }
        }
    }
    resolved.home
}

/// Writes the home to `user://home.json`.
pub fn save(home: Home) -> Result<(), ()> {
    let mut file = FileAccess::open(HOME_PATH, ModeFlags::WRITE).ok_or(())?;
    if file.store_string(&home_json(home)) {
        Ok(())
    } else {
        Err(())
    }
}

fn read_text(path: &str) -> Option<String> {
    if !FileAccess::file_exists(path) {
        return None;
    }
    let file = FileAccess::open(path, ModeFlags::READ)?;
    Some(file.get_as_text().to_string())
}

/// Text of the old home file at the repository root, when it exists.
fn legacy_file_text() -> Option<String> {
    let project = ProjectSettings::singleton()
        .globalize_path("res://")
        .to_string();
    let path = Path::new(&project).join("..").join(LEGACY_FILE);
    std::fs::read_to_string(path).ok()
}
