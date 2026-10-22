use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: nixe_theme::ThemeMode,
    pub theme: String,
    pub glow: bool,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub font_size: f64,
    pub font_family: String,
    pub line_spacing: i32,
    pub writing_width: i32,
    pub recent_vaults: Vec<PathBuf>,
    pub expanded_folders: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    pub last_vault: Option<PathBuf>,
    pub restore_tabs: bool,
    pub open_files: Vec<PathBuf>,
    pub templates_folder: String,
    pub daily_folder: String,
    pub daily_template: String,
    pub attachments_folder: String,
    pub editor_mode: String,
    pub split_enabled: bool,
    pub split_files: Vec<PathBuf>,
    pub graph: GraphSettings,
    pub collections: Vec<notes_vault::knowledge::Collection>,
    pub projects: Vec<notes_vault::knowledge::Project>,
    pub sync_profiles: Vec<serde_json::Value>,
    pub online_sync_profiles: Vec<serde_json::Value>,
    pub plugin_grants: Vec<serde_json::Value>,
    pub ai_enabled: bool,
    pub semantic_enabled: bool,
    pub integration_read: bool,
    pub integration_write: bool,
    pub glimpse_enabled: bool,
    pub history_keep: u32,
    pub history_days: u32,
    pub focus_mode: bool,
    pub typewriter: bool,
    pub workspaces: Vec<Workspace>,
    pub saved_searches: Vec<(String, String)>,
}
impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: nixe_theme::ThemeMode::Omarchy,
            theme: "tokyo-night".into(),
            glow: false,
            reduce_motion: false,
            sidebar_collapsed: false,
            font_size: 12.0,
            font_family: "JetBrains Mono".into(),
            line_spacing: 4,
            writing_width: 0,
            recent_vaults: vec![],
            expanded_folders: BTreeMap::new(),
            last_vault: None,
            restore_tabs: true,
            open_files: vec![],
            templates_folder: "Templates".into(),
            daily_folder: "Daily Notes".into(),
            daily_template: String::new(),
            attachments_folder: "Attachments".into(),
            editor_mode: "source".into(),
            split_enabled: false,
            split_files: Vec::new(),
            graph: GraphSettings::default(),
            collections: Vec::new(),
            projects: Vec::new(),
            sync_profiles: Vec::new(),
            online_sync_profiles: Vec::new(),
            plugin_grants: Vec::new(),
            ai_enabled: false,
            semantic_enabled: false,
            integration_read: false,
            integration_write: false,
            glimpse_enabled: false,
            history_keep: 200,
            history_days: 90,
            focus_mode: false,
            typewriter: false,
            workspaces: Vec::new(),
            saved_searches: Vec::new(),
        }
    }
}
nixe_core::prefs_store!(Prefs);

#[derive(Clone, Serialize, Deserialize)]
pub struct WorkspaceTab {
    pub path: PathBuf,
    pub right: bool,
    pub cursor: i32,
    #[serde(default)]
    pub selection: Option<(i32, i32)>,
    #[serde(default)]
    pub scroll: f64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    pub root: PathBuf,
    pub tabs: Vec<WorkspaceTab>,
    pub active: Option<PathBuf>,
    pub split: bool,
    pub mode: String,
    pub focus: bool,
    pub typewriter: bool,
    pub width: i32,
    pub sidebar: bool,
    pub divider: i32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphSettings {
    pub query: String,
    pub tag: String,
    pub depth: u32,
    pub orphans: bool,
    pub grouped: bool,
    pub labels: bool,
    pub zoom: f64,
    pub x: f64,
    pub y: f64,
}
impl Default for GraphSettings {
    fn default() -> Self {
        Self {
            query: String::new(),
            tag: String::new(),
            depth: 1,
            orphans: true,
            grouped: true,
            labels: true,
            zoom: 0.7,
            x: 0.0,
            y: 0.0,
        }
    }
}
