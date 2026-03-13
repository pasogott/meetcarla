use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use uuid::Uuid;

use crate::types::{SpeakerClip, TranscriptSegment, TranscriptionModel};

#[derive(Clone)]
pub struct TranscriptionRuntime {
    python_candidates: Vec<PathBuf>,
    script_candidates: Vec<PathBuf>,
    ffmpeg_candidates: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ModelListResponse {
    models: Vec<TranscriptionModel>,
}

#[derive(Debug, Deserialize)]
struct SegmentPayload {
    id: String,
    start: f64,
    end: f64,
    text: String,
    speaker: Option<String>,
    language: String,
}

#[derive(Debug, Deserialize)]
struct SpeakerClipsResponse {
    clips: Vec<SpeakerClip>,
}

#[derive(Debug, Deserialize)]
pub struct TranscriptionOutput {
    pub text: String,
    pub language: String,
    segments: Vec<SegmentPayload>,
}

impl TranscriptionOutput {
    pub fn into_segments(self, meeting_id: &str) -> Vec<TranscriptSegment> {
        self.segments
            .into_iter()
            .filter(|segment| !segment.text.trim().is_empty())
            .map(|segment| TranscriptSegment {
                id: format!("{meeting_id}-{}-{}", segment.id, Uuid::new_v4()),
                meeting_id: meeting_id.to_string(),
                start_time: segment.start,
                end_time: segment.end.max(segment.start),
                text: segment.text,
                speaker: segment.speaker,
                language: segment.language,
            })
            .collect()
    }
}

impl TranscriptionRuntime {
    pub fn discover() -> Self {
        let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| manifest_root.clone());
        let current_exe = env::current_exe().ok();
        let current_dir = env::current_dir().ok();

        let mut python_candidates = Vec::new();
        let mut script_candidates = Vec::new();
        let mut ffmpeg_candidates = Vec::new();

        if let Some(current_exe) = current_exe {
            if let Some(macos_dir) = current_exe.parent() {
                python_candidates.extend([
                    macos_dir.join("../Resources/.venv/bin/python3"),
                    macos_dir.join("../Resources/.venv/bin/python"),
                ]);
                script_candidates.push(macos_dir.join("../Resources/transcription_runtime.py"));
                ffmpeg_candidates.extend([
                    macos_dir.join("ffmpeg"),
                    macos_dir.join("../Resources/ffmpeg"),
                ]);
            }
        }

        python_candidates.extend([
            workspace_root.join(".venv/bin/python3"),
            workspace_root.join(".venv/bin/python"),
            manifest_root.join("../.venv/bin/python3"),
            manifest_root.join("../.venv/bin/python"),
        ]);
        script_candidates.extend([
            workspace_root.join("scripts/transcription_runtime.py"),
            manifest_root.join("../scripts/transcription_runtime.py"),
        ]);
        ffmpeg_candidates.extend([
            manifest_root.join("bin/ffmpeg"),
            workspace_root.join("src-tauri/bin/ffmpeg"),
            workspace_root.join(".local/bin/ffmpeg"),
        ]);

        if let Some(home) = env::var_os("HOME") {
            ffmpeg_candidates.extend([
                PathBuf::from(&home).join(".local/bin/ffmpeg"),
                PathBuf::from("/opt/homebrew/bin/ffmpeg"),
            ]);
        }

        if let Some(current_dir) = current_dir {
            python_candidates.extend([
                current_dir.join(".venv/bin/python3"),
                current_dir.join(".venv/bin/python"),
            ]);
            script_candidates.push(current_dir.join("scripts/transcription_runtime.py"));
            ffmpeg_candidates.push(current_dir.join("src-tauri/bin/ffmpeg"));
        }

        Self {
            python_candidates,
            script_candidates,
            ffmpeg_candidates,
        }
    }

    fn python_path(&self) -> Result<PathBuf> {
        self.python_candidates
            .iter()
            .find(|path| path.exists())
            .cloned()
            .ok_or_else(|| anyhow!("Python runtime not found for local transcription."))
    }

    fn script_path(&self) -> Result<PathBuf> {
        self.script_candidates
            .iter()
            .find(|path| path.exists())
            .cloned()
            .ok_or_else(|| anyhow!("transcription_runtime.py not found."))
    }

    fn ffmpeg_dir(&self) -> Option<PathBuf> {
        self.ffmpeg_candidates
            .iter()
            .find(|path| path.exists())
            .and_then(|path| path.parent().map(Path::to_path_buf))
    }

    fn ffmpeg_path(&self) -> Result<PathBuf> {
        self.ffmpeg_candidates
            .iter()
            .find(|path| path.exists())
            .cloned()
            .ok_or_else(|| anyhow!("ffmpeg not found for local media processing."))
    }

    fn path_with_ffmpeg(&self) -> OsString {
        let existing = env::var_os("PATH").unwrap_or_default();
        if let Some(ffmpeg_dir) = self.ffmpeg_dir() {
            let mut parts = vec![ffmpeg_dir];
            parts.extend(env::split_paths(&existing));
            env::join_paths(parts).unwrap_or(existing)
        } else {
            existing
        }
    }

    fn command(&self) -> Result<Command> {
        let python = self.python_path()?;
        let script = self.script_path()?;
        let mut command = Command::new(python);
        command.arg(script);
        command.env("PATH", self.path_with_ffmpeg());
        Ok(command)
    }

    pub fn list_models(
        &self,
        models_dir: &Path,
        active_model: &str,
    ) -> Result<Vec<TranscriptionModel>> {
        let mut command = self.command()?;
        let output = command
            .arg("list-models")
            .arg("--models-dir")
            .arg(models_dir)
            .arg("--active-model")
            .arg(active_model)
            .output()
            .context("failed to list local transcription models")?;
        if !output.status.success() {
            return Err(anyhow!(String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()));
        }
        let response: ModelListResponse = serde_json::from_slice(&output.stdout)?;
        Ok(response.models)
    }

    pub fn download_model(
        &self,
        models_dir: &Path,
        model_id: &str,
        active_model: &str,
    ) -> Result<()> {
        let mut command = self.command()?;
        let output = command
            .arg("download-model")
            .arg("--models-dir")
            .arg(models_dir)
            .arg("--model-id")
            .arg(model_id)
            .arg("--active-model")
            .arg(active_model)
            .output()
            .with_context(|| format!("failed to download model {model_id}"))?;
        if !output.status.success() {
            return Err(anyhow!(String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()));
        }
        Ok(())
    }

    pub fn transcribe(
        &self,
        models_dir: &Path,
        model_id: &str,
        audio_path: &Path,
        output_path: &Path,
        language: &str,
        diarize: bool,
    ) -> Result<TranscriptionOutput> {
        let mut command = self.command()?;
        let cmd = command
            .arg("transcribe")
            .arg("--models-dir")
            .arg(models_dir)
            .arg("--model-id")
            .arg(model_id)
            .arg("--audio-path")
            .arg(audio_path)
            .arg("--output-path")
            .arg(output_path)
            .arg("--language")
            .arg(language);
        if diarize {
            cmd.arg("--diarize");
        }
        let output = cmd
            .output()
            .with_context(|| format!("failed to transcribe with model {model_id}"))?;
        if !output.status.success() {
            return Err(anyhow!(String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()));
        }

        let payload = std::fs::read_to_string(output_path).with_context(|| {
            format!(
                "failed to read transcription output {}",
                output_path.display()
            )
        })?;
        serde_json::from_str(&payload).context("failed to parse transcription output")
    }

    pub fn extract_speaker_clips(
        &self,
        audio_path: &Path,
        transcript_path: &Path,
        output_dir: &Path,
    ) -> Result<Vec<SpeakerClip>> {
        let mut command = self.command()?;
        let output = command
            .arg("extract-speaker-clips")
            .arg("--audio-path")
            .arg(audio_path)
            .arg("--transcript-path")
            .arg(transcript_path)
            .arg("--output-dir")
            .arg(output_dir)
            .output()
            .context("failed to extract speaker clips")?;
        if !output.status.success() {
            return Err(anyhow!(String::from_utf8_lossy(&output.stderr)
                .trim()
                .to_string()));
        }
        let response: SpeakerClipsResponse = serde_json::from_slice(&output.stdout)
            .context("failed to parse speaker clips output")?;
        Ok(response.clips)
    }

    pub fn extract_playback_audio(&self, media_path: &Path, output_path: &Path) -> Result<()> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let ffmpeg = self.ffmpeg_path()?;
        let copy_attempt = Command::new(&ffmpeg)
            .env("PATH", self.path_with_ffmpeg())
            .args(["-y", "-i"])
            .arg(media_path)
            .args(["-vn", "-c:a", "copy"])
            .arg(output_path)
            .output()
            .with_context(|| {
                format!(
                    "failed to extract playback audio from {}",
                    media_path.display()
                )
            })?;
        if copy_attempt.status.success() {
            return Ok(());
        }

        let reencode_attempt = Command::new(&ffmpeg)
            .env("PATH", self.path_with_ffmpeg())
            .args(["-y", "-i"])
            .arg(media_path)
            .args(["-vn", "-c:a", "aac", "-b:a", "192k"])
            .arg(output_path)
            .output()
            .with_context(|| {
                format!(
                    "failed to re-encode playback audio from {}",
                    media_path.display()
                )
            })?;
        if !reencode_attempt.status.success() {
            return Err(anyhow!(
                "{}",
                String::from_utf8_lossy(&reencode_attempt.stderr)
                    .trim()
                    .to_string()
            ));
        }
        Ok(())
    }
}
