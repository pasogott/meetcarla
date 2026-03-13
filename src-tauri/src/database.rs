use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::types::{
    AppSettings, CalendarEvent, ChatMessage, CopyContent, DetectionSettings, LlmSettings,
    MeetingDetail, MeetingJob, MeetingSpeaker, MeetingStatus, MeetingSummary, MeetingTag,
    PermissionStatus, SearchResult, Speaker, SummaryTemplate, Tag, Task, TaskExtraction,
    TranscriptSegment, TranscriptionModel, Webhook, WebhookDelivery,
};

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    fn default_model_id() -> &'static str {
        "mlx-small"
    }

    fn model_catalog() -> [(&'static str, &'static str, i64); 6] {
        [
            ("mlx-tiny", "Whisper MLX Tiny", 151),
            ("mlx-small", "Whisper MLX Small", 488),
            ("mlx-medium", "Whisper MLX Medium", 1530),
            ("whisper-base", "Whisper Base", 142),
            ("whisper-small", "Whisper Small", 466),
            ("whisper-medium", "Whisper Medium", 1530),
        ]
    }

    fn model_ids() -> [&'static str; 6] {
        [
            "mlx-tiny",
            "mlx-small",
            "mlx-medium",
            "whisper-base",
            "whisper-small",
            "whisper-medium",
        ]
    }

    fn normalize_selected_model(connection: &Connection) -> Result<String> {
        let selected_model: Option<String> = connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'selected_transcription_model'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let normalized = selected_model
            .filter(|model_id| Self::model_ids().contains(&model_id.as_str()))
            .unwrap_or_else(|| Self::default_model_id().to_string());
        connection.execute(
            "INSERT INTO settings (key, value) VALUES ('selected_transcription_model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![normalized],
        )?;
        Ok(normalized)
    }

    fn model_family(model_id: &str) -> &'static str {
        if model_id.starts_with("mlx-") {
            "mlx"
        } else {
            "whisper"
        }
    }

    fn fts_query(raw: &str) -> Option<String> {
        let tokens = raw
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(|token| format!("\"{token}\""))
            .collect::<Vec<_>>();
        if tokens.is_empty() {
            None
        } else {
            Some(tokens.join(" OR "))
        }
    }

    pub fn new(path: PathBuf, storage_path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(storage_path)?;
        let db = Self { path };
        db.initialize(storage_path)?;
        Ok(db)
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)?;
        connection.execute("PRAGMA foreign_keys = ON", [])?;
        Ok(connection)
    }

    fn initialize(&self, storage_path: &Path) -> Result<()> {
        let connection = self.connect()?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meetings (
              id TEXT PRIMARY KEY,
              title TEXT NOT NULL,
              started_at TEXT NOT NULL,
              duration_seconds INTEGER NOT NULL DEFAULT 0,
              audio_file_path TEXT,
              platform TEXT NOT NULL,
              status TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS transcript_segments (
              id TEXT PRIMARY KEY,
              meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
              start_time REAL NOT NULL,
              end_time REAL NOT NULL,
              text TEXT NOT NULL,
              speaker TEXT,
              language TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS transcript_segments_fts
            USING fts5(segment_id UNINDEXED, meeting_id UNINDEXED, meeting_title UNINDEXED, text);

            CREATE TABLE IF NOT EXISTS meeting_jobs (
              id TEXT PRIMARY KEY,
              meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
              kind TEXT NOT NULL,
              status TEXT NOT NULL,
              error_message TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS models (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              size_mb INTEGER NOT NULL,
              installed INTEGER NOT NULL DEFAULT 0,
              active INTEGER NOT NULL DEFAULT 0,
              download_progress REAL
            );

            CREATE TABLE IF NOT EXISTS permissions (
              key TEXT PRIMARY KEY,
              granted INTEGER NOT NULL DEFAULT 0
            );
        "#,
        )?;

        let defaults = [
            ("selected_input_device", "Default Microphone".to_string()),
            ("selected_output_device", "System Audio".to_string()),
            ("selected_transcription_model", "mlx-small".to_string()),
            ("primary_language", "en".to_string()),
            ("storage_path", storage_path.to_string_lossy().to_string()),
            ("launch_at_login", "false".to_string()),
        ];
        for (key, value) in defaults {
            connection.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }

        for (id, name, size_mb) in Self::model_catalog() {
            connection.execute(
                "INSERT OR IGNORE INTO models (id, name, size_mb, installed, active, download_progress) VALUES (?1, ?2, ?3, 0, 0, NULL)",
                params![id, name, size_mb],
            )?;
        }
        let supported_ids = Self::model_ids()
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(", ");
        connection.execute(
            &format!("DELETE FROM models WHERE id NOT IN ({supported_ids})"),
            [],
        )?;
        connection.execute(
            "UPDATE models SET installed = 1, active = 1 WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM models WHERE active = 1)",
            [Self::default_model_id()],
        )?;
        let selected_model = Self::normalize_selected_model(&connection)?;
        connection.execute(
            "UPDATE models
             SET active = CASE WHEN id = ?1 AND installed = 1 THEN 1 ELSE active END",
            params![selected_model],
        )?;
        connection.execute(
            "UPDATE models
             SET active = 0
             WHERE id != ?1",
            params![selected_model],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO permissions (key, granted) VALUES ('microphone', 0), ('screen_recording', 0)",
            [],
        )?;

        // Incremental migrations - add columns/tables that may not exist yet
        let _ = connection.execute_batch("ALTER TABLE meetings ADD COLUMN summary TEXT;");
        let _ = connection.execute_batch("ALTER TABLE meetings ADD COLUMN scratchpad TEXT;");
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
              id TEXT PRIMARY KEY,
              meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
              text TEXT NOT NULL,
              assignee TEXT,
              completed INTEGER DEFAULT 0,
              position INTEGER DEFAULT 0,
              created_at TEXT NOT NULL DEFAULT (datetime('now')),
              updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;

        let llm_defaults = [
            ("llm_api_key", ""),
            ("llm_provider", "anthropic"),
            ("llm_model", "claude-sonnet-4-20250514"),
            ("summary_detail_level", "extensive"),
        ];
        for (key, value) in llm_defaults {
            connection.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }

        let detection_defaults = [
            ("auto_detect_calls", "true"),
            ("auto_detect_disabled_apps", ""),
        ];
        for (key, value) in detection_defaults {
            connection.execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }

        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS templates (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                prompt_template TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_builtin INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;

        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS speakers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                voice_embedding BLOB,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS meeting_speakers (
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                speaker_label TEXT NOT NULL,
                speaker_id TEXT REFERENCES speakers(id) ON DELETE SET NULL,
                clip_path TEXT,
                PRIMARY KEY (meeting_id, speaker_label)
            );
            "#,
        )?;

        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tags (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                color TEXT NOT NULL DEFAULT '#6B7280',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS meeting_tags (
                meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
                tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (meeting_id, tag_id)
            );
            "#,
        )?;

        let _ =
            connection.execute_batch("ALTER TABLE meetings ADD COLUMN calendar_event_title TEXT;");

        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                meeting_references TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;

        // Seed built-in templates if they don't exist yet
        let builtin_templates: &[(&str, &str, &str, i64)] = &[
            (
                "builtin-standard",
                "Standard",
                "You are a professional meeting assistant. Analyze the following meeting transcript and produce structured meeting notes.\n\nWrite a thorough executive summary followed by a full set of structured meeting notes. Include all major topics, decisions, and key points discussed. Use markdown with ## and ### headings.\n\nAlso extract all action items and tasks mentioned in the meeting. For each task, identify the assignee if mentioned.\n\nYou MUST respond with valid JSON only, no other text. Use this exact structure:\n{\n  \"summary\": \"## Executive Summary\\n...\\n\\n## Full Summary\\n\\n### Topic 1\\n...\",\n  \"tasks\": [\n    {\"text\": \"task description\", \"assignee\": \"person name or null\"}\n  ]\n}\n\nTranscript:\n",
                1,
            ),
            (
                "builtin-brief",
                "Brief",
                "You are a professional meeting assistant. Analyze the following meeting transcript.\n\nWrite a short executive summary (3-5 sentences) covering the key decisions and outcomes. Do not include detailed topic breakdowns.\n\nAlso extract all action items and tasks mentioned. For each task, identify the assignee if mentioned.\n\nYou MUST respond with valid JSON only:\n{\n  \"summary\": \"## Executive Summary\\n...\",\n  \"tasks\": [\n    {\"text\": \"task description\", \"assignee\": \"person name or null\"}\n  ]\n}\n\nTranscript:\n",
                0,
            ),
            (
                "builtin-action-focused",
                "Action-Focused",
                "You are a professional meeting assistant. Analyze the following meeting transcript.\n\nFocus on extracting decisions made and action items. Structure the summary around:\n1. Key Decisions - what was decided\n2. Action Items - who needs to do what and by when\n3. Open Questions - unresolved items that need follow-up\n\nKeep narrative to a minimum. Be concise and actionable.\n\nYou MUST respond with valid JSON only:\n{\n  \"summary\": \"## Decisions\\n...\\n\\n## Action Items\\n...\\n\\n## Open Questions\\n...\",\n  \"tasks\": [\n    {\"text\": \"task description\", \"assignee\": \"person name or null\"}\n  ]\n}\n\nTranscript:\n",
                0,
            ),
        ];
        for (id, name, prompt, is_default) in builtin_templates {
            connection.execute(
                "INSERT OR IGNORE INTO templates (id, name, prompt_template, is_default, is_builtin)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                params![id, name, prompt, is_default],
            )?;
        }

        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS webhooks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                events TEXT NOT NULL DEFAULT '[]',
                secret TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS webhook_deliveries (
                id TEXT PRIMARY KEY,
                webhook_id TEXT NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                response_status INTEGER,
                response_body TEXT,
                success INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )?;

        Ok(())
    }

    pub fn get_settings(&self) -> Result<AppSettings> {
        let connection = self.connect()?;
        let selected_transcription_model = Self::normalize_selected_model(&connection)?;
        let get_value = |key: &str| -> Result<String> {
            connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .with_context(|| format!("missing settings key {key}"))
        };

        Ok(AppSettings {
            selected_input_device: get_value("selected_input_device")?,
            selected_output_device: get_value("selected_output_device")?,
            selected_transcription_model,
            primary_language: get_value("primary_language")?,
            storage_path: get_value("storage_path")?,
            launch_at_login: get_value("launch_at_login")? == "true",
        })
    }

    pub fn update_settings(&self, settings: &AppSettings) -> Result<AppSettings> {
        let connection = self.connect()?;
        let updates = [
            (
                "selected_input_device",
                settings.selected_input_device.clone(),
            ),
            (
                "selected_output_device",
                settings.selected_output_device.clone(),
            ),
            (
                "selected_transcription_model",
                settings.selected_transcription_model.clone(),
            ),
            ("primary_language", settings.primary_language.clone()),
            ("storage_path", settings.storage_path.clone()),
            (
                "launch_at_login",
                if settings.launch_at_login {
                    "true".to_string()
                } else {
                    "false".to_string()
                },
            ),
        ];
        for (key, value) in updates {
            connection.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        self.get_settings()
    }

    pub fn get_detection_settings(&self) -> Result<DetectionSettings> {
        let connection = self.connect()?;
        let get_value = |key: &str| -> Option<String> {
            connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .ok()
        };
        let enabled = get_value("auto_detect_calls")
            .map(|v| v != "false")
            .unwrap_or(true);
        let disabled_apps = get_value("auto_detect_disabled_apps")
            .map(|v| {
                if v.is_empty() {
                    Vec::new()
                } else {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                }
            })
            .unwrap_or_default();
        Ok(DetectionSettings {
            enabled,
            disabled_apps,
        })
    }

    pub fn update_detection_settings(
        &self,
        settings: &DetectionSettings,
    ) -> Result<DetectionSettings> {
        let connection = self.connect()?;
        let enabled_str = if settings.enabled { "true" } else { "false" };
        let disabled_apps_str = settings.disabled_apps.join(",");
        for (key, value) in [
            ("auto_detect_calls", enabled_str),
            ("auto_detect_disabled_apps", disabled_apps_str.as_str()),
        ] {
            connection.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        self.get_detection_settings()
    }

    pub fn list_models(&self) -> Result<Vec<TranscriptionModel>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, name, size_mb, installed, active, download_progress
             FROM models
             ORDER BY active DESC, installed DESC, name ASC",
        )?;
        let models = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(TranscriptionModel {
                    family: Self::model_family(&id).to_string(),
                    id,
                    name: row.get(1)?,
                    size_mb: row.get::<_, i64>(2)? as u64,
                    installed: row.get::<_, i64>(3)? != 0,
                    active: row.get::<_, i64>(4)? != 0,
                    download_progress: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|model| Self::model_ids().contains(&model.id.as_str()))
            .collect::<Vec<_>>();
        Ok(models)
    }

    pub fn prune_unsupported_models(&self, supported_model_ids: &[String]) -> Result<()> {
        let connection = self.connect()?;
        let supported_model_ids = supported_model_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut statement = connection.prepare("SELECT id FROM models")?;
        let stale_ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|model_id| !supported_model_ids.contains(model_id.as_str()))
            .collect::<Vec<_>>();
        for stale_id in stale_ids {
            connection.execute("DELETE FROM models WHERE id = ?1", params![stale_id])?;
        }
        Ok(())
    }

    pub fn set_model_state(
        &self,
        model_id: &str,
        installed: bool,
        active: bool,
        download_progress: Option<f64>,
    ) -> Result<()> {
        let connection = self.connect()?;
        if active {
            connection.execute("UPDATE models SET active = 0", [])?;
        }
        connection.execute(
            "UPDATE models
             SET installed = ?2, active = ?3, download_progress = ?4
             WHERE id = ?1",
            params![
                model_id,
                if installed { 1 } else { 0 },
                if active { 1 } else { 0 },
                download_progress
            ],
        )?;
        if active {
            connection.execute(
                "INSERT INTO settings (key, value) VALUES ('selected_transcription_model', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![model_id],
            )?;
        }
        Ok(())
    }

    pub fn set_active_model(&self, model_id: &str) -> Result<()> {
        let connection = self.connect()?;
        connection.execute("UPDATE models SET active = 0", [])?;
        connection.execute(
            "UPDATE models SET active = 1, installed = 1, download_progress = NULL WHERE id = ?1",
            params![model_id],
        )?;
        connection.execute(
            "INSERT INTO settings (key, value) VALUES ('selected_transcription_model', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![model_id],
        )?;
        Ok(())
    }

    pub fn get_permissions(&self) -> Result<PermissionStatus> {
        let connection = self.connect()?;
        let microphone = connection.query_row(
            "SELECT granted FROM permissions WHERE key = 'microphone'",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        let screen_recording = connection.query_row(
            "SELECT granted FROM permissions WHERE key = 'screen_recording'",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        Ok(PermissionStatus {
            microphone,
            screen_recording,
        })
    }

    pub fn set_permission(&self, key: &str, granted: bool) -> Result<PermissionStatus> {
        let connection = self.connect()?;
        connection.execute(
            "INSERT INTO permissions (key, granted) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET granted = excluded.granted",
            params![key, if granted { 1 } else { 0 }],
        )?;
        self.get_permissions()
    }

    pub fn create_meeting(
        &self,
        title: &str,
        started_at: &str,
        platform: &str,
        status: MeetingStatus,
    ) -> Result<String> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        connection.execute(
            "INSERT INTO meetings (id, title, started_at, duration_seconds, audio_file_path, platform, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, NULL, ?4, ?5, ?3, ?3)",
            params![id, title, started_at, platform, serde_json::to_string(&status)?],
        )?;
        let job_id = Uuid::new_v4().to_string();
        connection.execute(
            "INSERT INTO meeting_jobs (id, meeting_id, kind, status, error_message, created_at, updated_at)
             VALUES (?1, ?2, 'transcription', 'running', NULL, ?3, ?3)",
            params![job_id, id, started_at],
        )?;
        Ok(id)
    }

    pub fn update_meeting_state(
        &self,
        meeting_id: &str,
        duration_seconds: u64,
        status: MeetingStatus,
        audio_file_path: Option<&str>,
    ) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "UPDATE meetings
             SET duration_seconds = ?2, status = ?3, audio_file_path = COALESCE(?4, audio_file_path), updated_at = ?5
             WHERE id = ?1",
            params![
                meeting_id,
                duration_seconds as i64,
                serde_json::to_string(&status)?,
                audio_file_path,
                updated_at
            ],
        )?;
        Ok(())
    }

    pub fn set_meeting_audio_file_path(
        &self,
        meeting_id: &str,
        audio_file_path: &str,
    ) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "UPDATE meetings
             SET audio_file_path = ?2, updated_at = ?3
             WHERE id = ?1",
            params![meeting_id, audio_file_path, updated_at],
        )?;
        Ok(())
    }

    pub fn complete_job(
        &self,
        meeting_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "UPDATE meeting_jobs
             SET status = ?2, error_message = ?3, updated_at = ?4
             WHERE meeting_id = ?1 AND kind = 'transcription'",
            params![meeting_id, status, error_message, updated_at],
        )?;
        Ok(())
    }

    pub fn recover_interrupted_recordings(&self, meetings_dir: &Path) -> Result<Vec<String>> {
        let connection = self.connect()?;
        let recording_status = serde_json::to_string(&MeetingStatus::Recording)?;
        let failed_status = serde_json::to_string(&MeetingStatus::Failed)?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let interrupted_message = "Recording was interrupted before finalization completed.";
        let stale_meetings = connection
            .prepare(
                "SELECT id, duration_seconds, audio_file_path
                 FROM meetings
                 WHERE status = ?1",
            )?
            .query_map(params![recording_status], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut recovered = Vec::new();
        for (meeting_id, duration_seconds, audio_file_path) in stale_meetings {
            let fallback_m4a = meetings_dir.join(format!("{meeting_id}.m4a"));
            let fallback_mp4 = meetings_dir.join(format!("{meeting_id}.mp4"));
            let recovered_audio_path = audio_file_path
                .filter(|path| Path::new(path).exists())
                .or_else(|| {
                    fallback_m4a
                        .exists()
                        .then(|| fallback_m4a.to_string_lossy().to_string())
                })
                .or_else(|| {
                    fallback_mp4
                        .exists()
                        .then(|| fallback_mp4.to_string_lossy().to_string())
                });

            connection.execute(
                "UPDATE meetings
                 SET status = ?2, duration_seconds = ?3, audio_file_path = COALESCE(?4, audio_file_path), updated_at = ?5
                 WHERE id = ?1",
                params![
                    meeting_id,
                    failed_status,
                    duration_seconds,
                    recovered_audio_path,
                    updated_at
                ],
            )?;
            connection.execute(
                "UPDATE meeting_jobs
                 SET status = 'failed', error_message = ?2, updated_at = ?3
                 WHERE meeting_id = ?1 AND kind = 'transcription' AND status = 'running'",
                params![meeting_id, interrupted_message, updated_at],
            )?;
            recovered.push(meeting_id);
        }

        Ok(recovered)
    }

    #[allow(dead_code)]
    pub fn add_transcript_segment(
        &self,
        meeting_id: &str,
        start_time: f64,
        end_time: f64,
        text: &str,
        speaker: Option<&str>,
        language: &str,
    ) -> Result<()> {
        let mut connection = self.connect()?;
        let title: String = connection.query_row(
            "SELECT title FROM meetings WHERE id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )?;
        let transaction = connection.transaction()?;
        let segment = TranscriptSegment {
            id: Uuid::new_v4().to_string(),
            meeting_id: meeting_id.to_string(),
            start_time,
            end_time,
            text: text.to_string(),
            speaker: speaker.map(str::to_string),
            language: language.to_string(),
        };
        transaction.execute(
            "INSERT INTO transcript_segments (id, meeting_id, start_time, end_time, text, speaker, language)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                segment.id,
                segment.meeting_id,
                segment.start_time,
                segment.end_time,
                segment.text,
                segment.speaker,
                segment.language
            ],
        )?;
        transaction.execute(
            "INSERT INTO transcript_segments_fts (segment_id, meeting_id, meeting_title, text)
             VALUES (?1, ?2, ?3, ?4)",
            params![segment.id, segment.meeting_id, title, segment.text],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn append_transcript_segments(
        &self,
        meeting_id: &str,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        if segments.is_empty() {
            return Ok(());
        }
        let mut connection = self.connect()?;
        let title: String = connection.query_row(
            "SELECT title FROM meetings WHERE id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )?;
        let transaction = connection.transaction()?;
        for segment in segments {
            transaction.execute(
                "INSERT OR REPLACE INTO transcript_segments (id, meeting_id, start_time, end_time, text, speaker, language)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    segment.meeting_id,
                    segment.start_time,
                    segment.end_time,
                    segment.text,
                    segment.speaker,
                    segment.language
                ],
            )?;
            transaction.execute(
                "INSERT INTO transcript_segments_fts (segment_id, meeting_id, meeting_title, text)
                 VALUES (?1, ?2, ?3, ?4)",
                params![segment.id, segment.meeting_id, title, segment.text],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn replace_transcript_segments(
        &self,
        meeting_id: &str,
        segments: &[TranscriptSegment],
    ) -> Result<()> {
        let mut connection = self.connect()?;
        let title: String = connection.query_row(
            "SELECT title FROM meetings WHERE id = ?1",
            params![meeting_id],
            |row| row.get(0),
        )?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM transcript_segments_fts WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM transcript_segments WHERE meeting_id = ?1",
            params![meeting_id],
        )?;

        for segment in segments {
            transaction.execute(
                "INSERT INTO transcript_segments (id, meeting_id, start_time, end_time, text, speaker, language)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id,
                    segment.meeting_id,
                    segment.start_time,
                    segment.end_time,
                    segment.text,
                    segment.speaker,
                    segment.language
                ],
            )?;
            transaction.execute(
                "INSERT INTO transcript_segments_fts (segment_id, meeting_id, meeting_title, text)
                 VALUES (?1, ?2, ?3, ?4)",
                params![segment.id, segment.meeting_id, title, segment.text],
            )?;
        }

        transaction.commit()?;
        Ok(())
    }

    pub fn list_meetings(&self, query: Option<&str>) -> Result<Vec<MeetingSummary>> {
        let connection = self.connect()?;
        let ids: Option<HashSet<String>> = if let Some(term) =
            query.filter(|value| !value.trim().is_empty())
        {
            let term = term.trim();
            let title_matches = connection
                .prepare("SELECT id FROM meetings WHERE lower(title) LIKE '%' || lower(?1) || '%'")?
                .query_map(params![term], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<HashSet<_>, _>>()?;
            let mut matches = title_matches;
            if let Some(fts_query) = Self::fts_query(term) {
                if let Ok(mut statement) = connection
                    .prepare("SELECT DISTINCT meeting_id FROM transcript_segments_fts WHERE transcript_segments_fts MATCH ?1")
                {
                    for row in statement.query_map(params![fts_query], |row| row.get::<_, String>(0))? {
                        matches.insert(row?);
                    }
                }
            }
            Some(matches)
        } else {
            None
        };

        let mut statement = connection.prepare(
            "SELECT
                m.id,
                m.title,
                m.started_at,
                m.duration_seconds,
                m.audio_file_path,
                m.platform,
                m.status,
                COUNT(ts.id) as segment_count,
                m.calendar_event_title
             FROM meetings m
             LEFT JOIN transcript_segments ts ON ts.meeting_id = m.id
             GROUP BY m.id
             ORDER BY m.started_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let status: String = row.get(6)?;
            Ok(MeetingSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                started_at: row.get(2)?,
                duration_seconds: row.get::<_, i64>(3)? as u64,
                audio_file_path: row.get(4)?,
                platform: row.get(5)?,
                status: serde_json::from_str(&status).unwrap_or(MeetingStatus::Completed),
                segment_count: row.get::<_, i64>(7)? as u64,
                tags: Vec::new(),
                calendar_event_title: row.get(8)?,
            })
        })?;
        let mut meetings = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if let Some(matched_ids) = ids {
            meetings.retain(|meeting| matched_ids.contains(&meeting.id));
        }
        // Fetch tags for each meeting
        for meeting in &mut meetings {
            meeting.tags = self.list_meeting_tags(&meeting.id)?;
        }
        Ok(meetings)
    }

    pub fn get_meeting_detail(&self, meeting_id: &str) -> Result<MeetingDetail> {
        let connection = self.connect()?;
        let (summary, summary_text, scratchpad) = connection
            .query_row(
                "SELECT id, title, started_at, duration_seconds, audio_file_path, platform, status,
                    (SELECT COUNT(*) FROM transcript_segments WHERE meeting_id = meetings.id),
                    summary, scratchpad, calendar_event_title
                 FROM meetings
                 WHERE id = ?1",
                params![meeting_id],
                |row| {
                    let status: String = row.get(6)?;
                    let meeting_summary = MeetingSummary {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        started_at: row.get(2)?,
                        duration_seconds: row.get::<_, i64>(3)? as u64,
                        audio_file_path: row.get(4)?,
                        platform: row.get(5)?,
                        status: serde_json::from_str(&status).unwrap_or(MeetingStatus::Completed),
                        segment_count: row.get::<_, i64>(7)? as u64,
                        tags: Vec::new(),
                        calendar_event_title: row.get(10)?,
                    };
                    let summary_text: Option<String> = row.get(8)?;
                    let scratchpad: Option<String> = row.get(9)?;
                    Ok((meeting_summary, summary_text, scratchpad))
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("meeting not found"))?;

        let mut segments_statement = connection.prepare(
            "SELECT id, meeting_id, start_time, end_time, text, speaker, language
             FROM transcript_segments
             WHERE meeting_id = ?1
             ORDER BY start_time ASC",
        )?;
        let transcript_segments = segments_statement
            .query_map(params![meeting_id], |row| {
                Ok(TranscriptSegment {
                    id: row.get(0)?,
                    meeting_id: row.get(1)?,
                    start_time: row.get(2)?,
                    end_time: row.get(3)?,
                    text: row.get(4)?,
                    speaker: row.get(5)?,
                    language: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut jobs_statement = connection.prepare(
            "SELECT id, meeting_id, kind, status, error_message, created_at, updated_at
             FROM meeting_jobs
             WHERE meeting_id = ?1
             ORDER BY created_at DESC",
        )?;
        let jobs = jobs_statement
            .query_map(params![meeting_id], |row| {
                Ok(MeetingJob {
                    id: row.get(0)?,
                    meeting_id: row.get(1)?,
                    kind: row.get(2)?,
                    status: row.get(3)?,
                    error_message: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let tasks = self.list_tasks(meeting_id)?;
        let speakers = self.list_meeting_speakers(meeting_id)?;
        let tags = self.list_meeting_tags(meeting_id)?;

        let mut summary = summary;
        summary.tags = tags;

        Ok(MeetingDetail {
            summary,
            transcript_segments,
            jobs,
            summary_text,
            scratchpad,
            tasks,
            speakers,
        })
    }

    pub fn rename_meeting(&self, meeting_id: &str, title: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "UPDATE meetings SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting_id, title, updated_at],
        )?;
        connection.execute(
            "UPDATE transcript_segments_fts SET meeting_title = ?2 WHERE meeting_id = ?1",
            params![meeting_id, title],
        )?;
        Ok(())
    }

    pub fn delete_meeting(&self, meeting_id: &str) -> Result<Option<String>> {
        let mut connection = self.connect()?;
        let audio_path = connection
            .query_row(
                "SELECT audio_file_path FROM meetings WHERE id = ?1",
                params![meeting_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM transcript_segments_fts WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM transcript_segments WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM meeting_jobs WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute("DELETE FROM meetings WHERE id = ?1", params![meeting_id])?;
        transaction.commit()?;
        Ok(audio_path)
    }

    pub fn delete_transcript(&self, meeting_id: &str) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        transaction.execute(
            "DELETE FROM transcript_segments_fts WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "DELETE FROM transcript_segments WHERE meeting_id = ?1",
            params![meeting_id],
        )?;
        transaction.execute(
            "UPDATE meeting_jobs
             SET status = 'deleted', error_message = 'Transcript deleted by user.', updated_at = ?2
             WHERE meeting_id = ?1 AND kind = 'transcription'",
            params![meeting_id, updated_at],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list_tasks(&self, meeting_id: &str) -> Result<Vec<Task>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, meeting_id, text, assignee, completed, position, created_at, updated_at
             FROM tasks
             WHERE meeting_id = ?1
             ORDER BY position ASC, created_at ASC",
        )?;
        let tasks = statement
            .query_map(params![meeting_id], |row| {
                Ok(Task {
                    id: row.get(0)?,
                    meeting_id: row.get(1)?,
                    text: row.get(2)?,
                    assignee: row.get(3)?,
                    completed: row.get::<_, i64>(4)? != 0,
                    position: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn create_task(
        &self,
        meeting_id: &str,
        text: &str,
        assignee: Option<&str>,
    ) -> Result<Task> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let position: i32 = connection
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM tasks WHERE meeting_id = ?1",
                params![meeting_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        connection.execute(
            "INSERT INTO tasks (id, meeting_id, text, assignee, completed, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?6)",
            params![id, meeting_id, text, assignee, position, now],
        )?;
        Ok(Task {
            id,
            meeting_id: meeting_id.to_string(),
            text: text.to_string(),
            assignee: assignee.map(str::to_string),
            completed: false,
            position,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_task(
        &self,
        id: &str,
        text: &str,
        assignee: Option<&str>,
        completed: bool,
    ) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE tasks SET text = ?2, assignee = ?3, completed = ?4, updated_at = ?5 WHERE id = ?1",
            params![id, text, assignee, if completed { 1 } else { 0 }, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("task not found"));
        }
        Ok(())
    }

    pub fn delete_task(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(anyhow!("task not found"));
        }
        Ok(())
    }

    pub fn toggle_task(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE tasks SET completed = CASE WHEN completed = 0 THEN 1 ELSE 0 END, updated_at = ?2
             WHERE id = ?1",
            params![id, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("task not found"));
        }
        Ok(())
    }

    pub fn save_summary(&self, meeting_id: &str, summary: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE meetings SET summary = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting_id, summary, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("meeting not found"));
        }
        Ok(())
    }

    pub fn save_scratchpad(&self, meeting_id: &str, content: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE meetings SET scratchpad = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting_id, content, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("meeting not found"));
        }
        Ok(())
    }

    pub fn save_extracted_tasks(&self, meeting_id: &str, tasks: &[TaskExtraction]) -> Result<()> {
        let mut existing = self
            .list_tasks(meeting_id)?
            .into_iter()
            .map(|task| {
                (
                    task.text.trim().to_string(),
                    task.assignee
                        .as_deref()
                        .map(str::trim)
                        .filter(|value: &&str| !value.is_empty())
                        .map(str::to_string),
                )
            })
            .collect::<HashSet<_>>();

        for task in tasks {
            let text = task.text.trim();
            if text.is_empty() {
                continue;
            }

            let assignee = task
                .assignee
                .as_deref()
                .map(str::trim)
                .filter(|value: &&str| !value.is_empty())
                .map(str::to_string);
            let key = (text.to_string(), assignee.clone());
            if existing.contains(&key) {
                continue;
            }

            self.create_task(meeting_id, text, assignee.as_deref())?;
            existing.insert(key);
        }

        Ok(())
    }

    pub fn get_llm_settings(&self) -> Result<LlmSettings> {
        let connection = self.connect()?;
        let get_value = |key: &str| -> Result<String> {
            connection
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
                .with_context(|| format!("missing settings key {key}"))
        };
        Ok(LlmSettings {
            api_key: get_value("llm_api_key")?,
            provider: get_value("llm_provider")?,
            model: get_value("llm_model")?,
            detail_level: get_value("summary_detail_level")?,
        })
    }

    pub fn update_llm_settings(&self, settings: &LlmSettings) -> Result<LlmSettings> {
        let connection = self.connect()?;
        let pairs: [(&str, &str); 4] = [
            ("llm_api_key", &settings.api_key),
            ("llm_provider", &settings.provider),
            ("llm_model", &settings.model),
            ("summary_detail_level", &settings.detail_level),
        ];
        for (key, value) in &pairs {
            connection.execute(
                "UPDATE settings SET value = ?1 WHERE key = ?2",
                params![value, key],
            )?;
        }
        self.get_llm_settings()
    }

    pub fn list_templates(&self) -> Result<Vec<SummaryTemplate>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, name, prompt_template, is_default, is_builtin, created_at, updated_at
             FROM templates
             ORDER BY is_default DESC, is_builtin DESC, created_at ASC",
        )?;
        let templates = statement
            .query_map([], |row| {
                Ok(SummaryTemplate {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    prompt_template: row.get(2)?,
                    is_default: row.get::<_, i64>(3)? != 0,
                    is_builtin: row.get::<_, i64>(4)? != 0,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(templates)
    }

    pub fn create_template(&self, name: &str, prompt_template: &str) -> Result<SummaryTemplate> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO templates (id, name, prompt_template, is_default, is_builtin, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, 0, ?4, ?4)",
            params![id, name, prompt_template, now],
        )?;
        Ok(SummaryTemplate {
            id,
            name: name.to_string(),
            prompt_template: prompt_template.to_string(),
            is_default: false,
            is_builtin: false,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_template(&self, id: &str, name: &str, prompt_template: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE templates
             SET name = ?2, prompt_template = ?3, updated_at = ?4
             WHERE id = ?1 AND is_builtin = 0",
            params![id, name, prompt_template, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("template not found or is a built-in template"));
        }
        Ok(())
    }

    pub fn delete_template(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let is_builtin: Option<i64> = connection
            .query_row(
                "SELECT is_builtin FROM templates WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        match is_builtin {
            None => return Err(anyhow!("template not found")),
            Some(1) => return Err(anyhow!("built-in templates cannot be deleted")),
            _ => {}
        }
        connection.execute("DELETE FROM templates WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_default_template(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let exists: bool = connection
            .query_row("SELECT 1 FROM templates WHERE id = ?1", params![id], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Err(anyhow!("template not found"));
        }
        connection.execute("UPDATE templates SET is_default = 0", [])?;
        connection.execute(
            "UPDATE templates SET is_default = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn get_default_template(&self) -> Result<SummaryTemplate> {
        let connection = self.connect()?;
        connection
            .query_row(
                "SELECT id, name, prompt_template, is_default, is_builtin, created_at, updated_at
                 FROM templates
                 WHERE is_default = 1
                 LIMIT 1",
                [],
                |row| {
                    Ok(SummaryTemplate {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        prompt_template: row.get(2)?,
                        is_default: row.get::<_, i64>(3)? != 0,
                        is_builtin: row.get::<_, i64>(4)? != 0,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("no default template configured"))
    }

    pub fn get_template_by_id(&self, id: &str) -> Result<SummaryTemplate> {
        let connection = self.connect()?;
        connection
            .query_row(
                "SELECT id, name, prompt_template, is_default, is_builtin, created_at, updated_at
                 FROM templates
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(SummaryTemplate {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        prompt_template: row.get(2)?,
                        is_default: row.get::<_, i64>(3)? != 0,
                        is_builtin: row.get::<_, i64>(4)? != 0,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("template not found"))
    }

    pub fn search_transcripts(&self, query: &str) -> Result<Vec<SearchResult>> {
        let Some(fts_query) = Self::fts_query(query.trim()) else {
            return Ok(Vec::new());
        };
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
               f.meeting_id,
               f.meeting_title,
               f.segment_id,
               s.start_time,
               s.end_time,
               snippet(transcript_segments_fts, 3, '[', ']', '…', 12)
             FROM transcript_segments_fts f
             JOIN transcript_segments s ON s.id = f.segment_id
             WHERE transcript_segments_fts MATCH ?1
             ORDER BY s.start_time ASC
             LIMIT 25",
        )?;
        let results = statement
            .query_map(params![fts_query], |row| {
                Ok(SearchResult {
                    meeting_id: row.get(0)?,
                    meeting_title: row.get(1)?,
                    segment_id: row.get(2)?,
                    start_time: row.get(3)?,
                    end_time: row.get(4)?,
                    snippet: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(results)
    }

    pub fn list_speakers(&self) -> Result<Vec<Speaker>> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare("SELECT id, name, created_at, updated_at FROM speakers ORDER BY name ASC")?;
        let speakers = statement
            .query_map([], |row| {
                Ok(Speaker {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(speakers)
    }

    pub fn create_speaker(&self, name: &str) -> Result<Speaker> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO speakers (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, name, now],
        )?;
        Ok(Speaker {
            id,
            name: name.to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn rename_speaker(&self, id: &str, name: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE speakers SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("speaker not found"));
        }
        Ok(())
    }

    pub fn delete_speaker(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute("DELETE FROM speakers WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(anyhow!("speaker not found"));
        }
        Ok(())
    }

    pub fn list_meeting_speakers(&self, meeting_id: &str) -> Result<Vec<MeetingSpeaker>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT ms.meeting_id, ms.speaker_label, ms.speaker_id, sp.name, ms.clip_path
             FROM meeting_speakers ms
             LEFT JOIN speakers sp ON sp.id = ms.speaker_id
             WHERE ms.meeting_id = ?1
             ORDER BY ms.speaker_label ASC",
        )?;
        let speakers = statement
            .query_map(params![meeting_id], |row| {
                Ok(MeetingSpeaker {
                    meeting_id: row.get(0)?,
                    speaker_label: row.get(1)?,
                    speaker_id: row.get(2)?,
                    speaker_name: row.get(3)?,
                    clip_path: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(speakers)
    }

    pub fn assign_speaker(
        &self,
        meeting_id: &str,
        speaker_label: &str,
        speaker_id: &str,
    ) -> Result<()> {
        let connection = self.connect()?;
        connection.execute(
            "UPDATE meeting_speakers SET speaker_id = ?3
             WHERE meeting_id = ?1 AND speaker_label = ?2",
            params![meeting_id, speaker_label, speaker_id],
        )?;
        Ok(())
    }

    pub fn save_meeting_speakers(
        &self,
        meeting_id: &str,
        speakers: Vec<(String, Option<String>)>,
    ) -> Result<()> {
        let connection = self.connect()?;
        for (speaker_label, clip_path) in speakers {
            connection.execute(
                "INSERT INTO meeting_speakers (meeting_id, speaker_label, clip_path)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(meeting_id, speaker_label) DO UPDATE SET clip_path = excluded.clip_path",
                params![meeting_id, speaker_label, clip_path],
            )?;
        }
        Ok(())
    }

    pub fn list_chat_messages(&self, limit: u32, offset: u32) -> Result<Vec<ChatMessage>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, role, content, meeting_references, created_at
             FROM chat_messages
             ORDER BY created_at ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let messages = statement
            .query_map(params![limit, offset], |row| {
                let refs_json: Option<String> = row.get(3)?;
                Ok(ChatMessage {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    meeting_references: refs_json
                        .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok()),
                    created_at: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(messages)
    }

    pub fn save_chat_message(
        &self,
        role: &str,
        content: &str,
        meeting_references: Option<&str>,
    ) -> Result<ChatMessage> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO chat_messages (id, role, content, meeting_references, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, role, content, meeting_references, now],
        )?;
        Ok(ChatMessage {
            id,
            role: role.to_string(),
            content: content.to_string(),
            meeting_references: meeting_references
                .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok()),
            created_at: now,
        })
    }

    pub fn clear_chat_history(&self) -> Result<()> {
        let connection = self.connect()?;
        connection.execute("DELETE FROM chat_messages", [])?;
        Ok(())
    }

    // ---- Tags ----

    pub fn list_tags(&self) -> Result<Vec<Tag>> {
        let connection = self.connect()?;
        let mut statement =
            connection.prepare("SELECT id, name, color, created_at FROM tags ORDER BY name ASC")?;
        let tags = statement
            .query_map([], |row| {
                Ok(Tag {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    color: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    pub fn create_tag(&self, name: &str, color: &str) -> Result<Tag> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO tags (id, name, color, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, color, now],
        )?;
        Ok(Tag {
            id,
            name: name.to_string(),
            color: color.to_string(),
            created_at: now,
        })
    }

    pub fn update_tag(&self, id: &str, name: &str, color: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute(
            "UPDATE tags SET name = ?2, color = ?3 WHERE id = ?1",
            params![id, name, color],
        )?;
        if rows == 0 {
            return Err(anyhow!("tag not found"));
        }
        Ok(())
    }

    pub fn delete_tag(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute("DELETE FROM tags WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(anyhow!("tag not found"));
        }
        Ok(())
    }

    pub fn add_tag_to_meeting(&self, meeting_id: &str, tag_id: &str) -> Result<()> {
        let connection = self.connect()?;
        connection.execute(
            "INSERT OR IGNORE INTO meeting_tags (meeting_id, tag_id) VALUES (?1, ?2)",
            params![meeting_id, tag_id],
        )?;
        Ok(())
    }

    pub fn remove_tag_from_meeting(&self, meeting_id: &str, tag_id: &str) -> Result<()> {
        let connection = self.connect()?;
        connection.execute(
            "DELETE FROM meeting_tags WHERE meeting_id = ?1 AND tag_id = ?2",
            params![meeting_id, tag_id],
        )?;
        Ok(())
    }

    pub fn list_meeting_tags(&self, meeting_id: &str) -> Result<Vec<MeetingTag>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT mt.meeting_id, mt.tag_id, t.name, t.color
             FROM meeting_tags mt
             JOIN tags t ON t.id = mt.tag_id
             WHERE mt.meeting_id = ?1
             ORDER BY t.name ASC",
        )?;
        let tags = statement
            .query_map(params![meeting_id], |row| {
                Ok(MeetingTag {
                    meeting_id: row.get(0)?,
                    tag_id: row.get(1)?,
                    tag_name: row.get(2)?,
                    tag_color: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(tags)
    }

    pub fn list_meetings_by_tag(&self, tag_id: &str) -> Result<Vec<MeetingSummary>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT
                m.id,
                m.title,
                m.started_at,
                m.duration_seconds,
                m.audio_file_path,
                m.platform,
                m.status,
                COUNT(ts.id) as segment_count,
                m.calendar_event_title
             FROM meetings m
             JOIN meeting_tags mtt ON mtt.meeting_id = m.id
             LEFT JOIN transcript_segments ts ON ts.meeting_id = m.id
             WHERE mtt.tag_id = ?1
             GROUP BY m.id
             ORDER BY m.started_at DESC",
        )?;
        let rows = statement.query_map(params![tag_id], |row| {
            let status: String = row.get(6)?;
            Ok(MeetingSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                started_at: row.get(2)?,
                duration_seconds: row.get::<_, i64>(3)? as u64,
                audio_file_path: row.get(4)?,
                platform: row.get(5)?,
                status: serde_json::from_str(&status).unwrap_or(MeetingStatus::Completed),
                segment_count: row.get::<_, i64>(7)? as u64,
                tags: Vec::new(),
                calendar_event_title: row.get(8)?,
            })
        })?;
        let mut meetings = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        for meeting in &mut meetings {
            meeting.tags = self.list_meeting_tags(&meeting.id)?;
        }
        Ok(meetings)
    }

    // ---- Calendar ----

    pub fn link_meeting_to_calendar(
        &self,
        meeting_id: &str,
        calendar_event_title: &str,
    ) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE meetings SET calendar_event_title = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting_id, calendar_event_title, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("meeting not found"));
        }
        Ok(())
    }

    // ---- Copy with formatting ----

    pub fn copy_summary(&self, meeting_id: &str) -> Result<CopyContent> {
        let detail = self.get_meeting_detail(meeting_id)?;
        let plain_text = detail.summary_text.unwrap_or_default();
        let html = markdown_to_html(&plain_text);
        Ok(CopyContent { plain_text, html })
    }

    pub fn copy_transcript(&self, meeting_id: &str) -> Result<CopyContent> {
        let detail = self.get_meeting_detail(meeting_id)?;
        let segments = &detail.transcript_segments;
        if segments.is_empty() {
            return Ok(CopyContent {
                plain_text: String::new(),
                html: String::new(),
            });
        }
        let plain_lines: Vec<String> = segments
            .iter()
            .map(|s| {
                let minutes = (s.start_time / 60.0) as u64;
                let secs = (s.start_time % 60.0) as u64;
                let speaker = s.speaker.as_deref().unwrap_or("Speaker");
                format!("[{minutes:02}:{secs:02}] {speaker}: {}", s.text)
            })
            .collect();
        let plain_text = plain_lines.join("\n");

        let html_lines: Vec<String> = segments
            .iter()
            .map(|s| {
                let minutes = (s.start_time / 60.0) as u64;
                let secs = (s.start_time % 60.0) as u64;
                let speaker = s.speaker.as_deref().unwrap_or("Speaker");
                format!(
                    "<p><span style=\"color:#6B7280;font-size:0.85em\">[{minutes:02}:{secs:02}]</span> <strong>{}</strong>: {}</p>",
                    html_escape(speaker),
                    html_escape(&s.text)
                )
            })
            .collect();
        let html = html_lines.join("\n");

        Ok(CopyContent { plain_text, html })
    }

    pub fn copy_tasks(&self, meeting_id: &str) -> Result<CopyContent> {
        let tasks = self.list_tasks(meeting_id)?;
        if tasks.is_empty() {
            return Ok(CopyContent {
                plain_text: String::new(),
                html: String::new(),
            });
        }
        let plain_lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                let check = if t.completed { "[x]" } else { "[ ]" };
                match &t.assignee {
                    Some(assignee) => format!("{check} {} (@{assignee})", t.text),
                    None => format!("{check} {}", t.text),
                }
            })
            .collect();
        let plain_text = plain_lines.join("\n");

        let html_items: Vec<String> = tasks
            .iter()
            .map(|t| {
                let checked = if t.completed { " checked" } else { "" };
                let label = match &t.assignee {
                    Some(assignee) => format!(
                        "{} <span style=\"color:#6B7280\">(@{})</span>",
                        html_escape(&t.text),
                        html_escape(assignee)
                    ),
                    None => html_escape(&t.text),
                };
                format!(
                    "<li style=\"list-style:none\"><input type=\"checkbox\"{checked} disabled> {label}</li>"
                )
            })
            .collect();
        let html = format!("<ul style=\"padding:0\">{}</ul>", html_items.join("\n"));

        Ok(CopyContent { plain_text, html })
    }

    // ---- Edit commands ----

    pub fn update_summary(&self, meeting_id: &str, summary: &str) -> Result<()> {
        let connection = self.connect()?;
        let updated_at = chrono::Utc::now().to_rfc3339();
        let rows = connection.execute(
            "UPDATE meetings SET summary = ?2, updated_at = ?3 WHERE id = ?1",
            params![meeting_id, summary, updated_at],
        )?;
        if rows == 0 {
            return Err(anyhow!("meeting not found"));
        }
        Ok(())
    }

    pub fn update_transcript_segment(&self, segment_id: &str, text: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute(
            "UPDATE transcript_segments SET text = ?2 WHERE id = ?1",
            params![segment_id, text],
        )?;
        if rows == 0 {
            return Err(anyhow!("segment not found"));
        }
        // Update FTS index
        let meeting_id: Option<String> = connection
            .query_row(
                "SELECT meeting_id FROM transcript_segments WHERE id = ?1",
                params![segment_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(mid) = meeting_id {
            connection.execute(
                "UPDATE transcript_segments_fts SET text = ?2 WHERE segment_id = ?1",
                params![segment_id, text],
            )?;
            let _ = mid;
        }
        Ok(())
    }

    // ---- Webhooks ----

    pub fn list_webhooks(&self) -> Result<Vec<Webhook>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, name, url, events, secret, enabled, created_at, updated_at
             FROM webhooks
             ORDER BY created_at ASC",
        )?;
        let webhooks = statement
            .query_map([], |row| {
                let events_json: String = row.get(3)?;
                let enabled: i64 = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    events_json,
                    row.get::<_, Option<String>>(4)?,
                    enabled,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let result = webhooks
            .into_iter()
            .map(
                |(id, name, url, events_json, secret, enabled, created_at, updated_at)| {
                    let events: Vec<String> =
                        serde_json::from_str(&events_json).unwrap_or_default();
                    Webhook {
                        id,
                        name,
                        url,
                        events,
                        secret,
                        enabled: enabled != 0,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect();
        Ok(result)
    }

    pub fn create_webhook(
        &self,
        name: &str,
        url: &str,
        events: &[String],
        secret: Option<&str>,
    ) -> Result<Webhook> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let events_json = serde_json::to_string(events)?;
        connection.execute(
            "INSERT INTO webhooks (id, name, url, events, secret, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
            params![id, name, url, events_json, secret, now],
        )?;
        Ok(Webhook {
            id,
            name: name.to_string(),
            url: url.to_string(),
            events: events.to_vec(),
            secret: secret.map(|s| s.to_string()),
            enabled: true,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn update_webhook(
        &self,
        id: &str,
        name: &str,
        url: &str,
        events: &[String],
        secret: Option<&str>,
        enabled: bool,
    ) -> Result<()> {
        let connection = self.connect()?;
        let now = chrono::Utc::now().to_rfc3339();
        let events_json = serde_json::to_string(events)?;
        let rows = connection.execute(
            "UPDATE webhooks SET name = ?2, url = ?3, events = ?4, secret = ?5, enabled = ?6, updated_at = ?7 WHERE id = ?1",
            params![id, name, url, events_json, secret, enabled as i64, now],
        )?;
        if rows == 0 {
            return Err(anyhow!("webhook not found"));
        }
        Ok(())
    }

    pub fn delete_webhook(&self, id: &str) -> Result<()> {
        let connection = self.connect()?;
        let rows = connection.execute("DELETE FROM webhooks WHERE id = ?1", params![id])?;
        if rows == 0 {
            return Err(anyhow!("webhook not found"));
        }
        Ok(())
    }

    pub fn get_webhook(&self, id: &str) -> Result<Webhook> {
        let connection = self.connect()?;
        let (name, url, events_json, secret, enabled, created_at, updated_at) = connection.query_row(
            "SELECT name, url, events, secret, enabled, created_at, updated_at FROM webhooks WHERE id = ?1",
            params![id],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            )),
        ).with_context(|| "webhook not found")?;
        let events: Vec<String> = serde_json::from_str(&events_json).unwrap_or_default();
        Ok(Webhook {
            id: id.to_string(),
            name,
            url,
            events,
            secret,
            enabled: enabled != 0,
            created_at,
            updated_at,
        })
    }

    pub fn list_webhooks_for_event(&self, event_type: &str) -> Result<Vec<Webhook>> {
        let all = self.list_webhooks()?;
        Ok(all
            .into_iter()
            .filter(|w| w.enabled && w.events.iter().any(|e| e == event_type))
            .collect())
    }

    pub fn save_webhook_delivery(
        &self,
        webhook_id: &str,
        event_type: &str,
        payload: &str,
        response_status: Option<i32>,
        response_body: Option<&str>,
        success: bool,
    ) -> Result<()> {
        let connection = self.connect()?;
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        connection.execute(
            "INSERT INTO webhook_deliveries (id, webhook_id, event_type, payload, response_status, response_body, success, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, webhook_id, event_type, payload, response_status, response_body, success as i64, now],
        )?;
        Ok(())
    }

    pub fn list_webhook_deliveries(
        &self,
        webhook_id: &str,
        limit: u32,
    ) -> Result<Vec<WebhookDelivery>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, webhook_id, event_type, payload, response_status, response_body, success, created_at
             FROM webhook_deliveries
             WHERE webhook_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let deliveries = statement
            .query_map(params![webhook_id, limit], |row| {
                let success: i64 = row.get(6)?;
                Ok(WebhookDelivery {
                    id: row.get(0)?,
                    webhook_id: row.get(1)?,
                    event_type: row.get(2)?,
                    payload: row.get(3)?,
                    response_status: row.get(4)?,
                    response_body: row.get(5)?,
                    success: success != 0,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(deliveries)
    }
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn markdown_to_html_public(markdown: &str) -> String {
    markdown_to_html(markdown)
}

fn markdown_to_html(markdown: &str) -> String {
    let mut html = String::new();
    let mut in_list = false;

    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("### ") {
            if in_list {
                html.push_str("</ul>\n");
                in_list = false;
            }
            html.push_str(&format!("<h3>{}</h3>\n", apply_inline(rest)));
        } else if let Some(rest) = line.strip_prefix("## ") {
            if in_list {
                html.push_str("</ul>\n");
                in_list = false;
            }
            html.push_str(&format!("<h2>{}</h2>\n", apply_inline(rest)));
        } else if let Some(rest) = line.strip_prefix("# ") {
            if in_list {
                html.push_str("</ul>\n");
                in_list = false;
            }
            html.push_str(&format!("<h1>{}</h1>\n", apply_inline(rest)));
        } else if let Some(rest) = line
            .strip_prefix("- [x] ")
            .or_else(|| line.strip_prefix("- [X] "))
        {
            if !in_list {
                html.push_str("<ul style=\"padding:0\">\n");
                in_list = true;
            }
            html.push_str(&format!(
                "<li style=\"list-style:none\"><input type=\"checkbox\" checked disabled> {}</li>\n",
                apply_inline(rest)
            ));
        } else if let Some(rest) = line.strip_prefix("- [ ] ") {
            if !in_list {
                html.push_str("<ul style=\"padding:0\">\n");
                in_list = true;
            }
            html.push_str(&format!(
                "<li style=\"list-style:none\"><input type=\"checkbox\" disabled> {}</li>\n",
                apply_inline(rest)
            ));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            if !in_list {
                html.push_str("<ul>\n");
                in_list = true;
            }
            html.push_str(&format!("<li>{}</li>\n", apply_inline(rest)));
        } else if line.trim().is_empty() {
            if in_list {
                html.push_str("</ul>\n");
                in_list = false;
            }
        } else {
            if in_list {
                html.push_str("</ul>\n");
                in_list = false;
            }
            html.push_str(&format!("<p>{}</p>\n", apply_inline(line)));
        }
    }

    if in_list {
        html.push_str("</ul>\n");
    }

    html
}

fn apply_inline(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("**") {
        result.push_str(html_escape(&remaining[..start]).as_str());
        remaining = &remaining[start + 2..];
        if let Some(end) = remaining.find("**") {
            result.push_str(&format!(
                "<strong>{}</strong>",
                html_escape(&remaining[..end])
            ));
            remaining = &remaining[end + 2..];
        } else {
            result.push_str("**");
        }
    }
    result.push_str(html_escape(remaining).as_str());
    result
}

pub fn fetch_calendar_events(date: &str) -> Result<Vec<CalendarEvent>> {
    // Uses osascript to query Calendar.app for events on the given date.
    // date format expected: "YYYY-MM-DD"
    // TODO: Upgrade to EventKit via Swift helper for more reliable access and
    // attendee/video-link heuristics once calendar entitlement is configured.
    let script = format!(
        r#"
        set targetDateStr to "{date}"
        set yr to (text 1 thru 4 of targetDateStr) as integer
        set mo to (text 6 thru 7 of targetDateStr) as integer
        set dy to (text 9 thru 10 of targetDateStr) as integer
        set startDate to current date
        set year of startDate to yr
        set month of startDate to mo
        set day of startDate to dy
        set hours of startDate to 0
        set minutes of startDate to 0
        set seconds of startDate to 0
        set endDate to startDate + (1 * days)
        set output to ""
        tell application "Calendar"
            repeat with cal in calendars
                set evts to (every event of cal whose start date >= startDate and start date < endDate)
                repeat with evt in evts
                    set evtTitle to summary of evt
                    set evtStart to (start date of evt) as string
                    set evtEnd to (end date of evt) as string
                    set output to output & evtTitle & "|" & evtStart & "|" & evtEnd & "||"
                end repeat
            end repeat
        end tell
        return output
        "#
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("osascript failed: {stderr}"));
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut events = Vec::new();

    for entry in raw.split("||").filter(|s| !s.trim().is_empty()) {
        let parts: Vec<&str> = entry.splitn(3, '|').collect();
        if parts.len() < 3 {
            continue;
        }
        let title = parts[0].trim().to_string();
        let start_time = parts[1].trim().to_string();
        let end_time = parts[2].trim().to_string();
        let lower = title.to_lowercase();
        let is_meeting = lower.contains("meet")
            || lower.contains("call")
            || lower.contains("sync")
            || lower.contains("zoom")
            || lower.contains("teams")
            || lower.contains("standup")
            || lower.contains("review");
        events.push(CalendarEvent {
            title,
            start_time,
            end_time,
            is_meeting,
        });
    }

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::Database;
    use crate::types::{MeetingStatus, TranscriptSegment};
    use rusqlite::params;

    #[test]
    fn search_and_detail_round_trip() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let meeting_id = db
            .create_meeting(
                "Roadmap sync",
                "2026-03-09T10:00:00Z",
                "macOS",
                MeetingStatus::Recording,
            )
            .unwrap();
        db.add_transcript_segment(
            &meeting_id,
            0.0,
            4.0,
            "We need local transcript search for sales calls.",
            Some("Speaker 1"),
            "en",
        )
        .unwrap();
        db.rename_meeting(&meeting_id, "Sales sync").unwrap();

        let search = db.search_transcripts("local").unwrap();
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].meeting_title, "Sales sync");

        let detail = db.get_meeting_detail(&meeting_id).unwrap();
        assert_eq!(detail.summary.title, "Sales sync");
        assert_eq!(detail.transcript_segments.len(), 1);
    }

    #[test]
    fn settings_and_models_drop_unsupported_model_ids() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let connection = db.connect().unwrap();
        connection
            .execute(
                "INSERT INTO models (id, name, size_mb, installed, active, download_progress)
                 VALUES ('mlx-large', 'Whisper MLX Large', 3000, 0, 0, NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE settings SET value = 'mlx-large' WHERE key = 'selected_transcription_model'",
                [],
            )
            .unwrap();
        drop(connection);

        let settings = db.get_settings().unwrap();
        assert_eq!(settings.selected_transcription_model, "mlx-small");

        let models = db.list_models().unwrap();
        assert!(models.iter().all(|model| model.id != "mlx-large"));
    }

    #[test]
    fn recover_interrupted_recordings_marks_stale_meetings_failed() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let meeting_id = db
            .create_meeting(
                "Interrupted sync",
                "2026-03-09T10:00:00Z",
                "macOS",
                MeetingStatus::Recording,
            )
            .unwrap();
        let audio_path = tempdir
            .path()
            .join("meetings")
            .join(format!("{meeting_id}.mp4"));
        std::fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        std::fs::write(&audio_path, b"mp4").unwrap();

        let recovered = db
            .recover_interrupted_recordings(&tempdir.path().join("meetings"))
            .unwrap();
        assert_eq!(recovered, vec![meeting_id.clone()]);

        let detail = db.get_meeting_detail(&meeting_id).unwrap();
        assert_eq!(detail.summary.status, MeetingStatus::Failed);
        assert_eq!(
            detail.summary.audio_file_path.as_deref(),
            Some(audio_path.to_string_lossy().as_ref())
        );
        assert_eq!(detail.jobs[0].status, "failed");
    }

    #[test]
    fn delete_meeting_removes_transcript_segments_and_jobs() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let meeting_id = db
            .create_meeting(
                "Delete me",
                "2026-03-09T10:00:00Z",
                "macOS",
                MeetingStatus::Completed,
            )
            .unwrap();
        let audio_path = tempdir
            .path()
            .join("meetings")
            .join(format!("{meeting_id}.m4a"));
        std::fs::create_dir_all(audio_path.parent().unwrap()).unwrap();
        std::fs::write(&audio_path, b"m4a").unwrap();
        db.update_meeting_state(
            &meeting_id,
            8,
            MeetingStatus::Completed,
            Some(audio_path.to_string_lossy().as_ref()),
        )
        .unwrap();
        db.append_transcript_segments(
            &meeting_id,
            &[TranscriptSegment {
                id: "segment-delete".into(),
                meeting_id: meeting_id.clone(),
                start_time: 0.0,
                end_time: 1.0,
                text: "This transcript should disappear.".into(),
                speaker: Some("Speaker".into()),
                language: "en".into(),
            }],
        )
        .unwrap();
        db.complete_job(&meeting_id, "completed", None).unwrap();

        let deleted_audio_path = db.delete_meeting(&meeting_id).unwrap();
        assert_eq!(
            deleted_audio_path.as_deref(),
            Some(audio_path.to_string_lossy().as_ref())
        );
        assert!(db.get_meeting_detail(&meeting_id).is_err());
        assert!(db.search_transcripts("disappear").unwrap().is_empty());

        let connection = db.connect().unwrap();
        let transcript_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM transcript_segments WHERE meeting_id = ?1",
                params![meeting_id],
                |row| row.get(0),
            )
            .unwrap();
        let job_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM meeting_jobs WHERE meeting_id = ?1",
                params![meeting_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(transcript_count, 0);
        assert_eq!(job_count, 0);
    }

    #[test]
    fn delete_transcript_removes_segments_but_keeps_meeting() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let meeting_id = db
            .create_meeting(
                "Delete transcript only",
                "2026-03-09T10:00:00Z",
                "macOS",
                MeetingStatus::Completed,
            )
            .unwrap();
        db.add_transcript_segment(
            &meeting_id,
            0.0,
            1.0,
            "This transcript should be removed without deleting the meeting.",
            Some("Speaker"),
            "en",
        )
        .unwrap();
        db.complete_job(&meeting_id, "completed", None).unwrap();

        db.delete_transcript(&meeting_id).unwrap();

        let detail = db.get_meeting_detail(&meeting_id).unwrap();
        assert_eq!(detail.summary.id, meeting_id);
        assert!(detail.transcript_segments.is_empty());
        assert!(db.search_transcripts("removed").unwrap().is_empty());
        assert_eq!(detail.jobs[0].status, "deleted");
        assert_eq!(
            detail.jobs[0].error_message.as_deref(),
            Some("Transcript deleted by user.")
        );
    }

    #[test]
    fn save_extracted_tasks_skips_duplicate_regenerated_tasks() {
        let tempdir = tempfile::tempdir().unwrap();
        let db = Database::new(tempdir.path().join("carla.sqlite"), tempdir.path()).unwrap();
        let meeting_id = db
            .create_meeting(
                "Deduplicate extracted tasks",
                "2026-03-09T10:00:00Z",
                "macOS",
                MeetingStatus::Completed,
            )
            .unwrap();
        let extracted = vec![crate::types::TaskExtraction {
            text: "Send follow-up".into(),
            assignee: Some("Pascal".into()),
        }];

        db.save_extracted_tasks(&meeting_id, &extracted).unwrap();
        db.save_extracted_tasks(&meeting_id, &extracted).unwrap();

        let tasks = db.list_tasks(&meeting_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "Send follow-up");
        assert_eq!(tasks[0].assignee.as_deref(), Some("Pascal"));
    }
}
