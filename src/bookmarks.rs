//! Marque-pages : CONFIG_DIR/bookmarks.json.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub target: Option<String>,
    pub label: String,
    pub path: String,
}

pub type Bookmarks = BTreeMap<String, Bookmark>;

fn file() -> std::path::PathBuf {
    crate::config::config_dir().join("bookmarks.json")
}

pub fn load() -> Bookmarks {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(b: &Bookmarks) {
    let _ = std::fs::create_dir_all(crate::config::config_dir());
    if let Ok(t) = serde_json::to_string_pretty(b) {
        let _ = std::fs::write(file(), t);
    }
}
