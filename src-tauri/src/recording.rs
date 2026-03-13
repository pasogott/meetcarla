use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::state::{ActiveRecording, AppState};
use crate::summarization;
use crate::types::{
    AlertEvent, MeetingStatus, PlaybackState, ProcessingStep, RecordingState, RecordingStatus,
    SpeakerClip,
};

const LIVE_CHUNK_SECONDS: u64 = 5;

pub async fn get_recording_status(state: &AppState) -> RecordingStatus {
    let recording = state.recording.lock().await;
    if let Some(active) = &recording.active {
        RecordingStatus {
            state: RecordingState::Recording,
            meeting_id: Some(active.meeting_id.clone()),
            started_at: Some(active.started_at.clone()),
            duration_seconds: active.started_instant.elapsed().as_secs(),
        }
    } else {
        RecordingStatus {
            state: RecordingState::Idle,
            meeting_id: None,
            started_at: None,
            duration_seconds: 0,
        }
    }
}

pub async fn start_recording(state: AppState, app: AppHandle) -> Result<RecordingStatus> {
    let permissions = state.helper.check_permissions()?;
    if !permissions.microphone || !permissions.screen_recording {
        return Err(anyhow!(
            "Microphone and screen recording permissions are required before recording."
        ));
    }

    let mut recording = state.recording.lock().await;
    if recording.active.is_some() {
        return Err(anyhow!("A recording is already in progress."));
    }

    let started_at = chrono::Utc::now().to_rfc3339();
    let title = format!("Meeting {}", chrono::Local::now().format("%b %-d, %H:%M"));
    let meeting_id =
        state
            .database
            .create_meeting(&title, &started_at, "macOS", MeetingStatus::Recording)?;
    let audio_path = state.paths.meetings_dir.join(format!("{meeting_id}.mp4"));
    let chunk_dir = state
        .paths
        .meetings_dir
        .join(format!("{meeting_id}_chunks"));
    let stop_path = state.paths.meetings_dir.join(format!("{meeting_id}.stop"));
    let settings = state.database.get_settings()?;
    let device_id = if settings.selected_input_device.is_empty() {
        None
    } else {
        Some(settings.selected_input_device.clone())
    };
    let child = state
        .helper
        .spawn_meeting_recording(&audio_path, &stop_path, &chunk_dir, device_id.as_deref())
        .inspect_err(|_| {
            let _ = state.database.delete_meeting(&meeting_id);
            let _ = std::fs::remove_file(&audio_path);
            let _ = std::fs::remove_file(&stop_path);
            let _ = std::fs::remove_dir_all(&chunk_dir);
        })?;
    let (stop_tx, stop_rx) = watch::channel(false);
    let task = spawn_streaming_loop(
        state.clone(),
        app.clone(),
        meeting_id.clone(),
        chunk_dir.clone(),
        stop_rx,
    );
    let started_instant = Instant::now();
    recording.active = Some(ActiveRecording {
        meeting_id: meeting_id.clone(),
        started_at: started_at.clone(),
        started_instant,
        audio_path,
        chunk_dir,
        stop_path,
        child,
        stopper: stop_tx,
        task,
    });
    drop(recording);

    let status = RecordingStatus {
        state: RecordingState::Recording,
        meeting_id: Some(meeting_id),
        started_at: Some(started_at),
        duration_seconds: 0,
    };
    app.emit("recording-state-changed", &status)?;
    let _ = crate::app::show_recording_notch(&app);
    Ok(status)
}

pub async fn stop_recording(state: AppState, app: AppHandle) -> Result<RecordingStatus> {
    let active = {
        let mut recording = state.recording.lock().await;
        recording.active.take()
    };

    let Some(active) = active else {
        return Err(anyhow!("No active recording to stop."));
    };

    let stop_result: Result<RecordingStatus> = async {
        let _ = crate::app::hide_recording_notch(&app);
        app.emit(
            "recording-state-changed",
            &RecordingStatus {
                state: RecordingState::Finalizing,
                meeting_id: Some(active.meeting_id.clone()),
                started_at: Some(active.started_at.clone()),
                duration_seconds: active.started_instant.elapsed().as_secs(),
            },
        )?;

        let _ = active.stopper.send(true);
        let _ = std::fs::write(&active.stop_path, b"stop");
        wait_for_recording_process(active.child).await?;
        let _ = active.task.await;

        let duration_seconds = active.started_instant.elapsed().as_secs().max(1);
        let raw_capture_path = active.audio_path.clone();
        let playback_audio_path = state
            .paths
            .meetings_dir
            .join(format!("{}.m4a", active.meeting_id));
        let extract_runtime = state.transcription.clone();
        let raw_capture_for_extract = raw_capture_path.clone();
        let playback_audio_for_extract = playback_audio_path.clone();
        let extracted_playback = tokio::task::spawn_blocking(move || {
            extract_runtime
                .extract_playback_audio(&raw_capture_for_extract, &playback_audio_for_extract)
        })
        .await
        .map_err(|error| anyhow!("playback extraction join error: {error}"))?;
        let playback_media_path = if extracted_playback.is_ok() {
            playback_audio_path
        } else {
            raw_capture_path.clone()
        };
        let _ = process_live_transcript_chunks(
            &state,
            &app,
            &active.meeting_id,
            &active.chunk_dir,
            &mut HashSet::new(),
        )
        .await;
        state.database.update_meeting_state(
            &active.meeting_id,
            duration_seconds,
            MeetingStatus::Processing,
            Some(playback_media_path.to_string_lossy().as_ref()),
        )?;
        let _ = std::fs::remove_file(&active.stop_path);

        let playback = PlaybackState {
            meeting_id: Some(active.meeting_id.clone()),
            media_path: Some(playback_media_path.to_string_lossy().to_string()),
            position_seconds: 0.0,
            duration_seconds: duration_seconds as f64,
            is_playing: false,
            error: None,
        };
        *state.playback.lock().await = playback.clone();
        app.emit("playback-state-changed", &playback)?;
        emit_meeting_updated(&app, &active.meeting_id);
        app.emit(
            "user-alert",
            &AlertEvent {
                level: "info".into(),
                title: "Recording saved".into(),
                message:
                    "Carla saved the meeting recording locally and started local transcription."
                        .into(),
            },
        )?;

        spawn_post_recording_transcription(
            state.clone(),
            app.clone(),
            active.meeting_id.clone(),
            raw_capture_path,
            duration_seconds,
        );

        let status = RecordingStatus {
            state: RecordingState::Idle,
            meeting_id: None,
            started_at: None,
            duration_seconds: 0,
        };
        app.emit("recording-state-changed", &status)?;
        Ok(status)
    }
    .await;

    if let Err(error) = &stop_result {
        let duration_seconds = active.started_instant.elapsed().as_secs().max(1);
        let fallback_audio_path = if active.audio_path.exists() {
            Some(active.audio_path.to_string_lossy().to_string())
        } else {
            None
        };
        let _ = state.database.update_meeting_state(
            &active.meeting_id,
            duration_seconds,
            MeetingStatus::Failed,
            fallback_audio_path.as_deref(),
        );
        let _ = state.database.complete_job(
            &active.meeting_id,
            "failed",
            Some(&format!("Stopping the recording failed: {error}")),
        );
        let _ = std::fs::remove_file(&active.stop_path);
        emit_meeting_updated(&app, &active.meeting_id);
        let _ = app.emit(
            "user-alert",
            &AlertEvent {
                level: "error".into(),
                title: "Stop failed".into(),
                message: format!("The recording stopped with an error: {error}"),
            },
        );
        let _ = app.emit(
            "recording-state-changed",
            &RecordingStatus {
                state: RecordingState::Idle,
                meeting_id: None,
                started_at: None,
                duration_seconds: 0,
            },
        );
    }

    stop_result
}

fn spawn_streaming_loop(
    state: AppState,
    app: AppHandle,
    meeting_id: String,
    chunk_dir: PathBuf,
    mut stop_rx: watch::Receiver<bool>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let started = Instant::now();
        let mut processed_chunks = HashSet::new();
        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if *stop_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    let elapsed = started.elapsed().as_secs();
                    let _ = state.database.update_meeting_state(
                        &meeting_id,
                        elapsed,
                        MeetingStatus::Recording,
                        None,
                    );
                    let _ = process_live_transcript_chunks(
                        &state,
                        &app,
                        &meeting_id,
                        &chunk_dir,
                        &mut processed_chunks,
                    )
                    .await;
                    let _ = app.emit(
                        "recording-state-changed",
                        &RecordingStatus {
                            state: RecordingState::Recording,
                            meeting_id: Some(meeting_id.clone()),
                            started_at: None,
                            duration_seconds: elapsed,
                        },
                    );
                }
            }
        }
    })
}

async fn wait_for_recording_process(mut child: std::process::Child) -> Result<()> {
    for _ in 0..300 {
        if let Some(status) = child.try_wait()? {
            if status.success() {
                return Ok(());
            }
            return Err(anyhow!("meeting recorder exited with status {status}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    child.kill()?;
    let status = child.wait()?;
    Err(anyhow!(
        "meeting recorder did not stop gracefully and was terminated: {status}"
    ))
}

fn emit_meeting_updated(app: &AppHandle, meeting_id: &str) {
    let _ = app.emit("meeting-updated", meeting_id.to_string());
}

fn spawn_post_recording_transcription(
    state: AppState,
    app: AppHandle,
    meeting_id: String,
    audio_path: std::path::PathBuf,
    duration_seconds: u64,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = run_post_recording_transcription(
            state.clone(),
            app.clone(),
            meeting_id.clone(),
            audio_path.clone(),
            duration_seconds,
        )
        .await
        {
            let _ = state.database.update_meeting_state(
                &meeting_id,
                duration_seconds,
                MeetingStatus::Failed,
                Some(audio_path.to_string_lossy().as_ref()),
            );
            let error_message = error.to_string();
            let _ = state
                .database
                .complete_job(&meeting_id, "failed", Some(&error_message));
            emit_meeting_updated(&app, &meeting_id);
            let _ = app.emit(
                "user-alert",
                &AlertEvent {
                    level: "error".into(),
                    title: "Transcription failed".into(),
                    message: error_message,
                },
            );
        }
    });
}

async fn process_live_transcript_chunks(
    state: &AppState,
    app: &AppHandle,
    meeting_id: &str,
    chunk_dir: &Path,
    processed_chunks: &mut HashSet<u64>,
) -> Result<()> {
    if !chunk_dir.exists() {
        return Ok(());
    }

    let mut chunk_indexes = std::fs::read_dir(chunk_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if !file_name.ends_with(".done") {
                return None;
            }
            file_name
                .strip_prefix("chunk-")
                .and_then(|value| value.strip_suffix(".done"))
                .and_then(|value| value.parse::<u64>().ok())
        })
        .collect::<Vec<_>>();
    chunk_indexes.sort_unstable();

    for chunk_index in chunk_indexes {
        if processed_chunks.contains(&chunk_index) {
            continue;
        }

        let mic_chunk_path = chunk_dir.join(format!("chunk-{chunk_index:06}.mic.caf"));
        if !mic_chunk_path.exists() {
            processed_chunks.insert(chunk_index);
            continue;
        }

        let settings = state.database.get_settings()?;
        let candidate_model_ids = transcription_candidate_model_ids(state)?;
        if candidate_model_ids.is_empty() {
            return Ok(());
        }

        let transcript_path = chunk_dir.join(format!("chunk-{chunk_index:06}.transcript.json"));
        let mut transcribed = None;
        for model_id in candidate_model_ids {
            let runtime = state.transcription.clone();
            let models_dir = state.paths.models_dir.clone();
            let mic_chunk_for_attempt = mic_chunk_path.clone();
            let transcript_for_attempt = transcript_path.clone();
            let language = settings.primary_language.clone();
            let model_id_for_attempt = model_id.clone();
            let attempt = tokio::task::spawn_blocking(move || {
                runtime.transcribe(
                    &models_dir,
                    &model_id_for_attempt,
                    &mic_chunk_for_attempt,
                    &transcript_for_attempt,
                    &language,
                    false,
                )
            })
            .await
            .map_err(|error| anyhow!("live transcription task join error: {error}"))?;

            match attempt {
                Ok(output) => {
                    transcribed = Some((model_id, output));
                    break;
                }
                Err(_) => continue,
            }
        }

        if let Some((_model_id, output)) = transcribed {
            let chunk_offset = chunk_index as f64 * LIVE_CHUNK_SECONDS as f64;
            let mut segments = output.into_segments(meeting_id);
            for segment in &mut segments {
                segment.start_time += chunk_offset;
                segment.end_time += chunk_offset;
            }
            state
                .database
                .append_transcript_segments(meeting_id, &segments)?;
            emit_meeting_updated(app, meeting_id);
        }

        processed_chunks.insert(chunk_index);
        let _ = std::fs::remove_file(chunk_dir.join(format!("chunk-{chunk_index:06}.done")));
        let _ = std::fs::remove_file(transcript_path);
    }

    Ok(())
}

async fn run_post_recording_transcription(
    state: AppState,
    app: AppHandle,
    meeting_id: String,
    audio_path: std::path::PathBuf,
    duration_seconds: u64,
) -> Result<()> {
    let settings = state.database.get_settings()?;
    let candidate_model_ids = transcription_candidate_model_ids(&state)?;

    if candidate_model_ids.is_empty() {
        return Err(anyhow!(
            "No transcription model is installed. Open Settings and download an MLX or Whisper model."
        ));
    }

    let mut errors = Vec::new();
    let transcript_path = state
        .paths
        .meetings_dir
        .join(format!("{meeting_id}.transcript.json"));

    for model_id in candidate_model_ids {
        let runtime = state.transcription.clone();
        let models_dir = state.paths.models_dir.clone();
        let audio_path_for_attempt = audio_path.clone();
        let transcript_path_for_attempt = transcript_path.clone();
        let language = settings.primary_language.clone();
        let model_id_for_attempt = model_id.clone();
        let attempt = tokio::task::spawn_blocking(move || {
            runtime.transcribe(
                &models_dir,
                &model_id_for_attempt,
                &audio_path_for_attempt,
                &transcript_path_for_attempt,
                &language,
                true,
            )
        })
        .await
        .map_err(|error| anyhow!("transcription task join error: {error}"))?;

        match attempt {
            Ok(output) => {
                let transcript_text = output.text.clone();
                let transcript_language = output.language.clone();
                let segments = output.into_segments(&meeting_id);
                state
                    .database
                    .replace_transcript_segments(&meeting_id, &segments)?;
                state.database.update_meeting_state(
                    &meeting_id,
                    duration_seconds,
                    MeetingStatus::Completed,
                    Some(audio_path.to_string_lossy().as_ref()),
                )?;
                state
                    .database
                    .complete_job(&meeting_id, "completed", None)?;
                emit_meeting_updated(&app, &meeting_id);
                let _ = app.emit(
                    "user-alert",
                    &AlertEvent {
                        level: "success".into(),
                        title: "Transcript ready".into(),
                        message: format!(
                            "Local transcription finished with {model_id} in {transcript_language}. {} segment(s) saved.",
                            segments.len()
                        ),
                    },
                );
                let webhook_db = state.database.clone();
                let webhook_meeting_id = meeting_id.clone();
                tauri::async_runtime::spawn(async move {
                    let payload = serde_json::json!({
                        "event": "meeting.completed",
                        "meeting_id": webhook_meeting_id,
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    });
                    let _ = crate::webhooks::dispatch_webhook_event(
                        &webhook_db,
                        "meeting.completed",
                        &payload,
                    )
                    .await;
                });
                // Extract speaker clips if the transcript has multiple distinct speakers
                let unique_speakers: std::collections::HashSet<_> = segments
                    .iter()
                    .filter_map(|s| s.speaker.as_deref())
                    .collect();
                if unique_speakers.len() > 1 {
                    spawn_speaker_clip_extraction(
                        state.clone(),
                        app.clone(),
                        meeting_id.clone(),
                        audio_path.clone(),
                        transcript_path.clone(),
                    );
                }

                if transcript_text.trim().is_empty() {
                    let _ = app.emit(
                        "user-alert",
                        &AlertEvent {
                            level: "warning".into(),
                            title: "Empty transcript".into(),
                            message: "The recording was processed, but no spoken transcript text was detected.".into(),
                        },
                    );
                } else {
                    // Trigger AI summarization if an API key is configured
                    if let Ok(llm) = state.database.get_llm_settings() {
                        if !llm.api_key.is_empty() {
                            if let Ok(template) = state.database.get_default_template() {
                                spawn_post_transcription_summarization(
                                    state.clone(),
                                    app.clone(),
                                    meeting_id.clone(),
                                    transcript_text.clone(),
                                    llm.api_key,
                                    llm.provider,
                                    llm.model,
                                    template.prompt_template,
                                );
                            }
                        }
                    }
                }
                let _ = std::fs::remove_file(&transcript_path);
                return Ok(());
            }
            Err(error) => errors.push(format!("{model_id}: {error}")),
        }
    }

    Err(anyhow!(errors.join(" | ")))
}

#[allow(clippy::too_many_arguments)]
fn spawn_post_transcription_summarization(
    state: AppState,
    app: AppHandle,
    meeting_id: String,
    transcript_text: String,
    api_key: String,
    provider: String,
    model_name: String,
    prompt_template: String,
) {
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(
            "processing-step-changed",
            &ProcessingStep {
                step: "summarizing".into(),
                progress: 0.1,
            },
        );

        match summarization::generate_summary(
            &transcript_text,
            &prompt_template,
            &api_key,
            &provider,
            &model_name,
        )
        .await
        {
            Ok(result) => {
                let _ = app.emit(
                    "processing-step-changed",
                    &ProcessingStep {
                        step: "extracting_tasks".into(),
                        progress: 0.8,
                    },
                );
                let _ = state.database.save_summary(&meeting_id, &result.summary);
                let _ = state
                    .database
                    .save_extracted_tasks(&meeting_id, &result.tasks);
                let _ = app.emit(
                    "processing-step-changed",
                    &ProcessingStep {
                        step: "completed".into(),
                        progress: 1.0,
                    },
                );
                emit_meeting_updated(&app, &meeting_id);
                let _ = app.emit(
                    "user-alert",
                    &AlertEvent {
                        level: "success".into(),
                        title: "Summary ready".into(),
                        message: format!(
                            "AI summary generated with {} task(s) extracted.",
                            result.tasks.len()
                        ),
                    },
                );
                let webhook_db = state.database.clone();
                let webhook_meeting_id = meeting_id.clone();
                tauri::async_runtime::spawn(async move {
                    let payload = serde_json::json!({
                        "event": "meeting.summarized",
                        "meeting_id": webhook_meeting_id,
                        "timestamp": chrono::Utc::now().to_rfc3339()
                    });
                    let _ = crate::webhooks::dispatch_webhook_event(
                        &webhook_db,
                        "meeting.summarized",
                        &payload,
                    )
                    .await;
                });
            }
            Err(error) => {
                let _ = app.emit(
                    "user-alert",
                    &AlertEvent {
                        level: "warning".into(),
                        title: "Summary failed".into(),
                        message: format!("AI summarization failed: {error}"),
                    },
                );
            }
        }
    });
}

fn spawn_speaker_clip_extraction(
    state: AppState,
    app: AppHandle,
    meeting_id: String,
    audio_path: std::path::PathBuf,
    transcript_path: std::path::PathBuf,
) {
    tauri::async_runtime::spawn(async move {
        let clips_dir = state
            .paths
            .meetings_dir
            .join(format!("{meeting_id}_speakers"));
        let runtime = state.transcription.clone();
        let audio_for_clips = audio_path.clone();
        let transcript_for_clips = transcript_path.clone();
        let clips_dir_for_task = clips_dir.clone();

        let result = tokio::task::spawn_blocking(move || {
            runtime.extract_speaker_clips(
                &audio_for_clips,
                &transcript_for_clips,
                &clips_dir_for_task,
            )
        })
        .await;

        match result {
            Ok(Ok(clips)) => {
                let speaker_entries: Vec<(String, Option<String>)> = clips
                    .iter()
                    .map(|clip: &SpeakerClip| {
                        let clip_path = clips_dir.join(&clip.file).to_string_lossy().to_string();
                        (clip.speaker.clone(), Some(clip_path))
                    })
                    .collect();
                let _ = state
                    .database
                    .save_meeting_speakers(&meeting_id, speaker_entries);
                let _ = app.emit("speakers-extracted", meeting_id.clone());
                emit_meeting_updated(&app, &meeting_id);
            }
            Ok(Err(error)) => {
                eprintln!("[speaker-clips] extraction failed for {meeting_id}: {error}");
            }
            Err(error) => {
                eprintln!("[speaker-clips] extraction task panicked for {meeting_id}: {error}");
            }
        }
    });
}

fn transcription_candidate_model_ids(state: &AppState) -> Result<Vec<String>> {
    let settings = state.database.get_settings()?;
    let models = state.database.list_models()?;
    let mut candidate_model_ids = Vec::new();

    if let Some(selected) = models
        .iter()
        .find(|model| model.id == settings.selected_transcription_model && model.installed)
    {
        candidate_model_ids.push(selected.id.clone());
    }

    if let Some(active) = models.iter().find(|model| model.active && model.installed) {
        if !candidate_model_ids.iter().any(|id| id == &active.id) {
            candidate_model_ids.push(active.id.clone());
        }
    }

    for model in models.iter().filter(|model| model.installed) {
        if !candidate_model_ids.iter().any(|id| id == &model.id) {
            candidate_model_ids.push(model.id.clone());
        }
    }

    Ok(candidate_model_ids)
}
