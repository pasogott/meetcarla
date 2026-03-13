use std::path::PathBuf;
use std::process::Child;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use directories::ProjectDirs;
use rusqlite::Connection;
use tauri::{AppHandle, Manager};
use tokio::sync::{watch, Mutex};

use crate::database::Database;
use crate::helper::SwiftHelperManager;
use crate::transcription::TranscriptionRuntime;
use crate::types::PlaybackState;

pub struct ActiveRecording {
    pub meeting_id: String,
    pub started_at: String,
    pub started_instant: Instant,
    pub audio_path: PathBuf,
    pub chunk_dir: PathBuf,
    pub stop_path: PathBuf,
    pub child: Child,
    pub stopper: watch::Sender<bool>,
    pub task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub struct RecordingController {
    pub active: Option<ActiveRecording>,
}

#[derive(Clone)]
pub struct StoragePaths {
    pub meetings_dir: PathBuf,
    pub exports_dir: PathBuf,
    pub models_dir: PathBuf,
}

#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub paths: StoragePaths,
    pub recording: Arc<Mutex<RecordingController>>,
    pub playback: Arc<Mutex<PlaybackState>>,
    pub helper: SwiftHelperManager,
    pub transcription: TranscriptionRuntime,
}

impl AppState {
    pub fn new(app: &AppHandle) -> Result<Self> {
        let root = app.path().app_local_data_dir()?;
        migrate_legacy_storage(&root)?;
        let database_path = root.join("carla.sqlite");
        let database = Database::new(database_path.clone(), &root)?;
        let storage_root = resolved_storage_root(&database, &root);
        let meetings_dir = storage_root.join("meetings");
        let exports_dir = storage_root.join("exports");
        let models_dir = storage_root.join("models");
        std::fs::create_dir_all(&meetings_dir)?;
        std::fs::create_dir_all(&exports_dir)?;
        std::fs::create_dir_all(&models_dir)?;
        let _ = database.recover_interrupted_recordings(&meetings_dir)?;
        let helper = SwiftHelperManager::discover();
        let transcription = TranscriptionRuntime::discover();

        Ok(Self {
            database,
            paths: StoragePaths {
                meetings_dir,
                exports_dir,
                models_dir,
            },
            recording: Arc::new(Mutex::new(RecordingController::default())),
            playback: Arc::new(Mutex::new(PlaybackState::default())),
            helper,
            transcription,
        })
    }
}

fn resolved_storage_root(database: &Database, default_root: &std::path::Path) -> PathBuf {
    database
        .get_settings()
        .ok()
        .and_then(|settings| {
            let storage_path = settings.storage_path.trim();
            (!storage_path.is_empty()).then(|| PathBuf::from(storage_path))
        })
        .unwrap_or_else(|| default_root.to_path_buf())
}

fn migrate_legacy_storage(root: &std::path::Path) -> Result<()> {
    let Some(project_dirs) = ProjectDirs::from("ai", "carla", "Carla") else {
        return Ok(());
    };
    let legacy_root = project_dirs.data_local_dir().to_path_buf();
    if legacy_root == root || !legacy_root.exists() {
        return Ok(());
    }

    if !root.exists() {
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&legacy_root, root)?;
        rewrite_storage_paths(root, &legacy_root, root)?;
        return Ok(());
    }

    merge_directory(&legacy_root, root)?;
    rewrite_storage_paths(root, &legacy_root, root)?;
    Ok(())
}

fn merge_directory(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            merge_directory(&source_path, &destination_path)?;
        } else if !destination_path.exists() {
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn rewrite_storage_paths(
    root: &std::path::Path,
    legacy_root: &std::path::Path,
    new_root: &std::path::Path,
) -> Result<()> {
    let database_path = root.join("carla.sqlite");
    if !database_path.exists() {
        return Ok(());
    }

    let connection = Connection::open(database_path)?;
    let legacy_root = legacy_root.to_string_lossy().to_string();
    let new_root = new_root.to_string_lossy().to_string();
    connection.execute(
        "UPDATE meetings
         SET audio_file_path = REPLACE(audio_file_path, ?1, ?2)
         WHERE audio_file_path LIKE ?3",
        rusqlite::params![legacy_root, new_root, format!("{legacy_root}%")],
    )?;
    connection.execute(
        "UPDATE settings
         SET value = ?2
         WHERE key = 'storage_path' AND value = ?1",
        rusqlite::params![legacy_root, new_root],
    )?;
    Ok(())
}
