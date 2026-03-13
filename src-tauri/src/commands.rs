use anyhow::{anyhow, Result};
use serde_json;
use tauri::{AppHandle, Emitter, State};

use crate::ask_ai;
use crate::exports;
use crate::recording;
use crate::state::AppState;
use crate::summarization;
use crate::types::{
    AppSettings, AskAiResponse, AudioDevice, CalendarEvent, ChatMessage, CopyContent,
    DetectionSettings, LlmSettings, MeetingDetail, MeetingSpeaker, MeetingSummary,
    NativeHelperStatus, PermissionStatus, PlaybackState, ProcessingStep, RecordingStatus,
    SearchResult, Speaker, SummaryTemplate, Tag, Task, TranscriptionModel, Webhook,
    WebhookDelivery,
};
use crate::webhooks;

fn map_error<T>(result: Result<T>) -> std::result::Result<T, String> {
    result.map_err(|error| error.to_string())
}

fn sync_permissions(state: &AppState, permissions: PermissionStatus) -> Result<PermissionStatus> {
    state
        .database
        .set_permission("microphone", permissions.microphone)?;
    state
        .database
        .set_permission("screen_recording", permissions.screen_recording)?;
    Ok(permissions)
}

fn sync_models(state: &AppState, models: &[TranscriptionModel]) -> Result<Vec<TranscriptionModel>> {
    let supported_model_ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    state
        .database
        .prune_unsupported_models(&supported_model_ids)?;
    for model in models {
        state.database.set_model_state(
            &model.id,
            model.installed,
            model.active,
            model.download_progress,
        )?;
    }
    state.database.list_models()
}

fn resolved_playback_media_path(
    state: &AppState,
    meeting_id: &str,
    stored_audio_path: Option<&str>,
) -> Result<std::path::PathBuf> {
    let extracted_path = state.paths.meetings_dir.join(format!("{meeting_id}.m4a"));
    if extracted_path.exists() {
        return Ok(extracted_path);
    }

    if let Some(audio_path) = stored_audio_path {
        let media_path = std::path::PathBuf::from(audio_path);
        if media_path.exists() {
            return Ok(media_path);
        }
    }

    let raw_capture_path = state.paths.meetings_dir.join(format!("{meeting_id}.mp4"));
    if raw_capture_path.exists() {
        return Ok(raw_capture_path);
    }

    Err(anyhow!("No recording file is available for this meeting."))
}

#[tauri::command]
pub async fn get_recording_state(
    state: State<'_, AppState>,
) -> std::result::Result<RecordingStatus, String> {
    Ok(recording::get_recording_status(state.inner()).await)
}

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<RecordingStatus, String> {
    map_error(recording::start_recording(state.inner().clone(), app).await)
}

#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<RecordingStatus, String> {
    map_error(recording::stop_recording(state.inner().clone(), app).await)
}

#[tauri::command]
pub fn check_permissions(
    state: State<'_, AppState>,
) -> std::result::Result<PermissionStatus, String> {
    map_error(
        state
            .helper
            .check_permissions()
            .and_then(|permissions| sync_permissions(state.inner(), permissions)),
    )
}

#[tauri::command]
pub fn request_microphone_permission(
    state: State<'_, AppState>,
) -> std::result::Result<PermissionStatus, String> {
    map_error(
        state
            .helper
            .request_microphone_permission()
            .and_then(|permissions| sync_permissions(state.inner(), permissions)),
    )
}

#[tauri::command]
pub fn request_screen_recording_permission(
    state: State<'_, AppState>,
) -> std::result::Result<PermissionStatus, String> {
    map_error(
        state
            .helper
            .request_screen_recording_permission()
            .and_then(|permissions| sync_permissions(state.inner(), permissions)),
    )
}

#[tauri::command]
pub fn open_system_settings(state: State<'_, AppState>) -> std::result::Result<String, String> {
    map_error(state.helper.open_system_settings())
}

#[tauri::command]
pub fn list_meetings(
    state: State<'_, AppState>,
    query: Option<String>,
) -> std::result::Result<Vec<MeetingSummary>, String> {
    map_error(state.database.list_meetings(query.as_deref()))
}

#[tauri::command]
pub fn get_meeting_detail(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<MeetingDetail, String> {
    map_error(state.database.get_meeting_detail(&meeting_id))
}

#[tauri::command]
pub fn rename_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    title: String,
) -> std::result::Result<(), String> {
    map_error(state.database.rename_meeting(&meeting_id, &title))
}

#[tauri::command]
pub async fn delete_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<(), String> {
    // Dispatch webhook before deletion
    let webhook_payload = serde_json::json!({
        "event": "meeting.deleted",
        "meeting_id": meeting_id,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    let _ = webhooks::dispatch_webhook_event(&state.database, "meeting.deleted", &webhook_payload)
        .await;

    let audio_file = map_error(state.database.delete_meeting(&meeting_id))?;
    if let Some(audio_file) = audio_file {
        let _ = std::fs::remove_file(audio_file);
    }
    let mp4_capture = state.paths.meetings_dir.join(format!("{meeting_id}.mp4"));
    let m4a_playback = state.paths.meetings_dir.join(format!("{meeting_id}.m4a"));
    let chunk_dir = state
        .paths
        .meetings_dir
        .join(format!("{meeting_id}_chunks"));
    let _ = std::fs::remove_file(mp4_capture);
    let _ = std::fs::remove_file(m4a_playback);
    let _ = std::fs::remove_dir_all(chunk_dir);
    Ok(())
}

#[tauri::command]
pub fn delete_transcript(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.delete_transcript(&meeting_id))
}

#[tauri::command]
pub fn open_meeting_media(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<(), String> {
    let detail = map_error(state.database.get_meeting_detail(&meeting_id))?;
    let fallback_capture = state.paths.meetings_dir.join(format!("{meeting_id}.mp4"));
    let path = detail
        .summary
        .audio_file_path
        .map(std::path::PathBuf::from)
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("mp4"))
        .or_else(|| fallback_capture.exists().then_some(fallback_capture))
        .ok_or_else(|| "No recording file is available for this meeting.".to_string())?;
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn search_transcripts(
    state: State<'_, AppState>,
    query: String,
) -> std::result::Result<Vec<SearchResult>, String> {
    map_error(state.database.search_transcripts(&query))
}

#[tauri::command]
pub fn export_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    format: String,
) -> std::result::Result<String, String> {
    let detail = map_error(state.database.get_meeting_detail(&meeting_id))?;
    let output = map_error(exports::export_meeting(&detail, &format))?;
    let path = state
        .paths
        .exports_dir
        .join(format!("{}_export.{}", meeting_id, format));
    map_error(std::fs::write(&path, output).map_err(Into::into))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> std::result::Result<AppSettings, String> {
    map_error(state.database.get_settings())
}

#[tauri::command]
pub fn list_models(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<TranscriptionModel>, String> {
    let settings = map_error(state.database.get_settings())?;
    let models = map_error(state.transcription.list_models(
        &state.paths.models_dir,
        &settings.selected_transcription_model,
    ))?;
    map_error(sync_models(state.inner(), &models))
}

#[tauri::command]
pub async fn download_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> std::result::Result<Vec<TranscriptionModel>, String> {
    let selected_model = map_error(state.database.get_settings())?.selected_transcription_model;
    let supported_models = map_error(
        state
            .transcription
            .list_models(&state.paths.models_dir, &selected_model),
    )?;
    if !supported_models.iter().any(|model| model.id == model_id) {
        return Err(format!("Unsupported model id: {model_id}"));
    }
    map_error(state.database.set_model_state(
        &model_id,
        false,
        model_id == selected_model,
        Some(0.0),
    ))?;
    let _ = app.emit("models-changed", &model_id);

    let runtime = state.transcription.clone();
    let models_dir = state.paths.models_dir.clone();
    let model_id_for_download = model_id.clone();
    let selected_model_for_download = selected_model.clone();
    let download_result = tokio::task::spawn_blocking(move || {
        runtime.download_model(
            &models_dir,
            &model_id_for_download,
            &selected_model_for_download,
        )
    })
    .await
    .map_err(|error| format!("download task join error: {error}"))?;
    map_error(download_result)?;

    let models = map_error(
        state
            .transcription
            .list_models(&state.paths.models_dir, &selected_model),
    )?;
    let synced = map_error(sync_models(state.inner(), &models))?;
    let _ = app.emit("models-changed", &model_id);
    Ok(synced)
}

#[tauri::command]
pub fn select_model(
    app: AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> std::result::Result<Vec<TranscriptionModel>, String> {
    let models = map_error(state.database.list_models())?;
    let model = models
        .iter()
        .find(|candidate| candidate.id == model_id)
        .ok_or_else(|| "Model not found.".to_string())?;
    if !model.installed {
        return Err("Install the model before selecting it.".into());
    }
    map_error(state.database.set_active_model(&model_id))?;
    let next_models = map_error(state.database.list_models())?;
    let _ = app.emit("models-changed", &model_id);
    Ok(next_models)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> std::result::Result<AppSettings, String> {
    map_error(state.database.update_settings(&settings))
}

#[tauri::command]
pub async fn load_playback(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<PlaybackState, String> {
    let detail = map_error(state.database.get_meeting_detail(&meeting_id))?;
    let media_path = map_error(resolved_playback_media_path(
        state.inner(),
        &meeting_id,
        detail.summary.audio_file_path.as_deref(),
    ))?;
    let resolved_media_path =
        if media_path.extension().and_then(|value| value.to_str()) == Some("mp4") {
            let playback_path = state.paths.meetings_dir.join(format!("{meeting_id}.m4a"));
            if !playback_path.exists() {
                let runtime = state.transcription.clone();
                let source = media_path.clone();
                let target = playback_path.clone();
                let extraction = tokio::task::spawn_blocking(move || {
                    runtime.extract_playback_audio(&source, &target)
                })
                .await
                .map_err(|error| format!("playback extraction join error: {error}"))?;
                map_error(extraction)?;
            }
            playback_path
        } else {
            media_path
        };
    if detail.summary.audio_file_path.as_deref()
        != Some(resolved_media_path.to_string_lossy().as_ref())
    {
        map_error(state.database.set_meeting_audio_file_path(
            &meeting_id,
            resolved_media_path.to_string_lossy().as_ref(),
        ))?;
    }
    let playback = PlaybackState {
        meeting_id: Some(meeting_id),
        media_path: Some(resolved_media_path.to_string_lossy().to_string()),
        position_seconds: 0.0,
        duration_seconds: detail.summary.duration_seconds as f64,
        is_playing: false,
        error: None,
    };
    *state.playback.lock().await = playback.clone();
    let _ = app.emit("playback-state-changed", &playback);
    Ok(playback)
}

#[tauri::command]
pub async fn play_playback(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<PlaybackState, String> {
    let mut playback = state.playback.lock().await;
    playback.is_playing = true;
    let next = playback.clone();
    let _ = app.emit("playback-state-changed", &next);
    Ok(next)
}

#[tauri::command]
pub async fn pause_playback(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<PlaybackState, String> {
    let mut playback = state.playback.lock().await;
    playback.is_playing = false;
    let next = playback.clone();
    let _ = app.emit("playback-state-changed", &next);
    Ok(next)
}

#[tauri::command]
pub async fn seek_playback(
    app: AppHandle,
    state: State<'_, AppState>,
    position_seconds: f64,
) -> std::result::Result<PlaybackState, String> {
    let mut playback = state.playback.lock().await;
    playback.position_seconds = position_seconds;
    let next = playback.clone();
    let _ = app.emit("playback-state-changed", &next);
    Ok(next)
}

#[tauri::command]
pub async fn get_playback_state(
    state: State<'_, AppState>,
) -> std::result::Result<PlaybackState, String> {
    Ok(state.playback.lock().await.clone())
}

#[tauri::command]
pub fn get_native_helper_status(
    state: State<'_, AppState>,
) -> std::result::Result<NativeHelperStatus, String> {
    Ok(state.helper.status())
}

#[tauri::command]
pub fn list_audio_devices(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<AudioDevice>, String> {
    map_error(state.helper.list_audio_devices())
}

#[tauri::command]
pub async fn summarize_meeting(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
    template_id: Option<String>,
) -> std::result::Result<MeetingDetail, String> {
    let detail = map_error(state.database.get_meeting_detail(&meeting_id))?;
    if detail.transcript_segments.is_empty() {
        return Err("No transcript available to summarize.".into());
    }

    let transcript_text = detail
        .transcript_segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let llm = map_error(state.database.get_llm_settings())?;

    let template = match template_id {
        Some(ref id) => map_error(state.database.get_template_by_id(id))?,
        None => map_error(state.database.get_default_template())?,
    };

    let _ = app.emit(
        "processing-step-changed",
        &ProcessingStep {
            step: "summarizing".into(),
            progress: 0.1,
        },
    );

    let result = summarization::generate_summary(
        &transcript_text,
        &template.prompt_template,
        &llm.api_key,
        &llm.provider,
        &llm.model,
    )
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "processing-step-changed",
        &ProcessingStep {
            step: "extracting_tasks".into(),
            progress: 0.8,
        },
    );

    map_error(state.database.save_summary(&meeting_id, &result.summary))?;

    map_error(
        state
            .database
            .save_extracted_tasks(&meeting_id, &result.tasks),
    )?;

    let _ = app.emit(
        "processing-step-changed",
        &ProcessingStep {
            step: "completed".into(),
            progress: 1.0,
        },
    );
    let _ = app.emit("meeting-updated", meeting_id.clone());

    // Dispatch webhook non-blocking
    let webhook_db = state.database.clone();
    let webhook_meeting_id = meeting_id.clone();
    tauri::async_runtime::spawn(async move {
        let payload = serde_json::json!({
            "event": "meeting.summarized",
            "meeting_id": webhook_meeting_id,
            "timestamp": chrono::Utc::now().to_rfc3339()
        });
        let _ = webhooks::dispatch_webhook_event(&webhook_db, "meeting.summarized", &payload).await;
    });

    map_error(state.database.get_meeting_detail(&meeting_id))
}

#[tauri::command]
pub fn list_tasks(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<Vec<Task>, String> {
    map_error(state.database.list_tasks(&meeting_id))
}

#[tauri::command]
pub fn create_task(
    state: State<'_, AppState>,
    meeting_id: String,
    text: String,
    assignee: Option<String>,
) -> std::result::Result<Task, String> {
    map_error(
        state
            .database
            .create_task(&meeting_id, &text, assignee.as_deref()),
    )
}

#[tauri::command]
pub fn update_task(
    state: State<'_, AppState>,
    task_id: String,
    text: String,
    assignee: Option<String>,
    completed: bool,
) -> std::result::Result<(), String> {
    map_error(
        state
            .database
            .update_task(&task_id, &text, assignee.as_deref(), completed),
    )
}

#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, task_id: String) -> std::result::Result<(), String> {
    map_error(state.database.delete_task(&task_id))
}

#[tauri::command]
pub fn toggle_task(state: State<'_, AppState>, task_id: String) -> std::result::Result<(), String> {
    map_error(state.database.toggle_task(&task_id))
}

#[tauri::command]
pub fn update_scratchpad(
    state: State<'_, AppState>,
    meeting_id: String,
    content: String,
) -> std::result::Result<(), String> {
    map_error(state.database.save_scratchpad(&meeting_id, &content))
}

#[tauri::command]
pub fn get_detection_settings(
    state: State<'_, AppState>,
) -> std::result::Result<DetectionSettings, String> {
    map_error(state.database.get_detection_settings())
}

#[tauri::command]
pub fn update_detection_settings(
    state: State<'_, AppState>,
    enabled: bool,
    disabled_apps: Vec<String>,
) -> std::result::Result<DetectionSettings, String> {
    let settings = DetectionSettings {
        enabled,
        disabled_apps,
    };
    map_error(state.database.update_detection_settings(&settings))
}

#[tauri::command]
pub fn list_templates(
    state: State<'_, AppState>,
) -> std::result::Result<Vec<SummaryTemplate>, String> {
    map_error(state.database.list_templates())
}

#[tauri::command]
pub fn create_template(
    state: State<'_, AppState>,
    name: String,
    prompt_template: String,
) -> std::result::Result<SummaryTemplate, String> {
    map_error(state.database.create_template(&name, &prompt_template))
}

#[tauri::command]
pub fn update_template(
    state: State<'_, AppState>,
    template_id: String,
    name: String,
    prompt_template: String,
) -> std::result::Result<(), String> {
    map_error(
        state
            .database
            .update_template(&template_id, &name, &prompt_template),
    )
}

#[tauri::command]
pub fn delete_template(
    state: State<'_, AppState>,
    template_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.delete_template(&template_id))
}

#[tauri::command]
pub fn set_default_template(
    state: State<'_, AppState>,
    template_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.set_default_template(&template_id))
}

#[tauri::command]
pub fn list_speakers(state: State<'_, AppState>) -> std::result::Result<Vec<Speaker>, String> {
    map_error(state.database.list_speakers())
}

#[tauri::command]
pub fn create_speaker(
    state: State<'_, AppState>,
    name: String,
) -> std::result::Result<Speaker, String> {
    map_error(state.database.create_speaker(&name))
}

#[tauri::command]
pub fn rename_speaker(
    state: State<'_, AppState>,
    speaker_id: String,
    name: String,
) -> std::result::Result<(), String> {
    map_error(state.database.rename_speaker(&speaker_id, &name))
}

#[tauri::command]
pub fn delete_speaker(
    state: State<'_, AppState>,
    speaker_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.delete_speaker(&speaker_id))
}

#[tauri::command]
pub fn list_meeting_speakers(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<Vec<MeetingSpeaker>, String> {
    map_error(state.database.list_meeting_speakers(&meeting_id))
}

#[tauri::command]
pub fn assign_speaker_to_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    speaker_label: String,
    speaker_id: String,
) -> std::result::Result<(), String> {
    map_error(
        state
            .database
            .assign_speaker(&meeting_id, &speaker_label, &speaker_id),
    )
}

#[tauri::command]
pub async fn extract_speaker_clips(
    app: AppHandle,
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<(), String> {
    let detail = map_error(state.database.get_meeting_detail(&meeting_id))?;
    let audio_path = map_error(
        detail
            .summary
            .audio_file_path
            .as_deref()
            .map(std::path::PathBuf::from)
            .filter(|path| path.exists())
            .ok_or_else(|| anyhow!("No audio file available for this meeting.")),
    )?;
    let transcript_path = state
        .paths
        .meetings_dir
        .join(format!("{meeting_id}.transcript.json"));
    if !transcript_path.exists() {
        return Err("No transcript file available for speaker extraction.".into());
    }
    let clips_dir = state
        .paths
        .meetings_dir
        .join(format!("{meeting_id}_speakers"));
    let runtime = state.transcription.clone();
    let clips = tokio::task::spawn_blocking(move || {
        runtime.extract_speaker_clips(&audio_path, &transcript_path, &clips_dir)
    })
    .await
    .map_err(|e| format!("speaker clip task join error: {e}"))?
    .map_err(|e| e.to_string())?;
    let speaker_entries: Vec<(String, Option<String>)> = clips
        .iter()
        .map(|clip| {
            let clip_file_path = state
                .paths
                .meetings_dir
                .join(format!("{meeting_id}_speakers"))
                .join(&clip.file)
                .to_string_lossy()
                .to_string();
            (clip.speaker.clone(), Some(clip_file_path))
        })
        .collect();
    map_error(
        state
            .database
            .save_meeting_speakers(&meeting_id, speaker_entries),
    )?;
    let _ = app.emit("speakers-extracted", meeting_id.clone());
    let _ = app.emit("meeting-updated", meeting_id);
    Ok(())
}

#[tauri::command]
pub async fn ask_ai(
    state: State<'_, AppState>,
    question: String,
    meeting_id: Option<String>,
) -> std::result::Result<AskAiResponse, String> {
    let llm = map_error(state.database.get_llm_settings())?;

    map_error(state.database.save_chat_message("user", &question, None))?;

    let response = ask_ai::ask_ai(
        &question,
        meeting_id.as_deref(),
        &state.database,
        &llm.api_key,
        &llm.provider,
        &llm.model,
    )
    .await
    .map_err(|e| e.to_string())?;

    let meeting_ref_ids: Vec<String> = response
        .meeting_references
        .iter()
        .map(|r| r.meeting_id.clone())
        .collect();
    let refs_json = serde_json::to_string(&meeting_ref_ids).unwrap_or_default();

    map_error(
        state
            .database
            .save_chat_message("assistant", &response.answer, Some(&refs_json)),
    )?;

    Ok(response)
}

#[tauri::command]
pub fn list_chat_messages(
    state: State<'_, AppState>,
    limit: u32,
    offset: u32,
) -> std::result::Result<Vec<ChatMessage>, String> {
    map_error(state.database.list_chat_messages(limit, offset))
}

#[tauri::command]
pub fn clear_chat_history(state: State<'_, AppState>) -> std::result::Result<(), String> {
    map_error(state.database.clear_chat_history())
}

#[tauri::command]
pub async fn force_quit(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<(), String> {
    // Stop recording if active, then exit immediately
    {
        let recording = state.recording.lock().await;
        if recording.active.is_some() {
            drop(recording);
            let _ = recording::stop_recording(state.inner().clone(), app.clone()).await;
        }
    }
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn stop_and_quit(
    app: AppHandle,
    state: State<'_, AppState>,
) -> std::result::Result<(), String> {
    let is_recording = {
        let recording = state.recording.lock().await;
        recording.active.is_some()
    };
    if is_recording {
        map_error(recording::stop_recording(state.inner().clone(), app.clone()).await)?;
    }
    app.exit(0);
    Ok(())
}

// ---- Tags ----

#[tauri::command]
pub fn list_tags(state: State<'_, AppState>) -> std::result::Result<Vec<Tag>, String> {
    map_error(state.database.list_tags())
}

#[tauri::command]
pub fn create_tag(
    state: State<'_, AppState>,
    name: String,
    color: String,
) -> std::result::Result<Tag, String> {
    map_error(state.database.create_tag(&name, &color))
}

#[tauri::command]
pub fn update_tag(
    state: State<'_, AppState>,
    tag_id: String,
    name: String,
    color: String,
) -> std::result::Result<(), String> {
    map_error(state.database.update_tag(&tag_id, &name, &color))
}

#[tauri::command]
pub fn delete_tag(state: State<'_, AppState>, tag_id: String) -> std::result::Result<(), String> {
    map_error(state.database.delete_tag(&tag_id))
}

#[tauri::command]
pub fn add_tag_to_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    tag_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.add_tag_to_meeting(&meeting_id, &tag_id))
}

#[tauri::command]
pub fn remove_tag_from_meeting(
    state: State<'_, AppState>,
    meeting_id: String,
    tag_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.remove_tag_from_meeting(&meeting_id, &tag_id))
}

#[tauri::command]
pub fn list_meetings_by_tag(
    state: State<'_, AppState>,
    tag_id: String,
) -> std::result::Result<Vec<MeetingSummary>, String> {
    map_error(state.database.list_meetings_by_tag(&tag_id))
}

// ---- Copy with formatting ----

#[tauri::command]
pub fn copy_summary(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<CopyContent, String> {
    map_error(state.database.copy_summary(&meeting_id))
}

#[tauri::command]
pub fn copy_transcript(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<CopyContent, String> {
    map_error(state.database.copy_transcript(&meeting_id))
}

#[tauri::command]
pub fn copy_tasks(
    state: State<'_, AppState>,
    meeting_id: String,
) -> std::result::Result<CopyContent, String> {
    map_error(state.database.copy_tasks(&meeting_id))
}

// ---- Calendar ----

#[tauri::command]
pub fn list_calendar_events(date: String) -> std::result::Result<Vec<CalendarEvent>, String> {
    map_error(crate::database::fetch_calendar_events(&date))
}

#[tauri::command]
pub fn link_meeting_to_calendar(
    state: State<'_, AppState>,
    meeting_id: String,
    calendar_event_title: String,
) -> std::result::Result<(), String> {
    map_error(
        state
            .database
            .link_meeting_to_calendar(&meeting_id, &calendar_event_title),
    )
}

// ---- Edit commands ----

#[tauri::command]
pub fn update_summary(
    state: State<'_, AppState>,
    meeting_id: String,
    summary: String,
) -> std::result::Result<(), String> {
    map_error(state.database.update_summary(&meeting_id, &summary))
}

#[tauri::command]
pub fn update_transcript_segment(
    state: State<'_, AppState>,
    segment_id: String,
    text: String,
) -> std::result::Result<(), String> {
    map_error(state.database.update_transcript_segment(&segment_id, &text))
}

// ---- Webhooks ----

#[tauri::command]
pub fn list_webhooks(state: State<'_, AppState>) -> std::result::Result<Vec<Webhook>, String> {
    map_error(state.database.list_webhooks())
}

#[tauri::command]
pub fn create_webhook(
    state: State<'_, AppState>,
    name: String,
    url: String,
    events: Vec<String>,
    secret: Option<String>,
) -> std::result::Result<Webhook, String> {
    map_error(
        state
            .database
            .create_webhook(&name, &url, &events, secret.as_deref()),
    )
}

#[tauri::command]
pub fn update_webhook(
    state: State<'_, AppState>,
    webhook_id: String,
    name: String,
    url: String,
    events: Vec<String>,
    secret: Option<String>,
    enabled: bool,
) -> std::result::Result<(), String> {
    map_error(state.database.update_webhook(
        &webhook_id,
        &name,
        &url,
        &events,
        secret.as_deref(),
        enabled,
    ))
}

#[tauri::command]
pub fn delete_webhook(
    state: State<'_, AppState>,
    webhook_id: String,
) -> std::result::Result<(), String> {
    map_error(state.database.delete_webhook(&webhook_id))
}

#[tauri::command]
pub fn list_webhook_deliveries(
    state: State<'_, AppState>,
    webhook_id: String,
    limit: u32,
) -> std::result::Result<Vec<WebhookDelivery>, String> {
    map_error(state.database.list_webhook_deliveries(&webhook_id, limit))
}

#[tauri::command]
pub async fn test_webhook(
    state: State<'_, AppState>,
    webhook_id: String,
) -> std::result::Result<(), String> {
    let webhook = map_error(state.database.get_webhook(&webhook_id))?;
    let payload = serde_json::json!({
        "event": "webhook.test",
        "webhook_id": webhook.id,
        "timestamp": chrono::Utc::now().to_rfc3339()
    });
    // Dispatch directly to this specific webhook regardless of subscribed events
    map_error(webhooks::dispatch_to_webhook(&webhook, &payload).await)
}

#[tauri::command]
pub async fn get_llm_settings(
    state: State<'_, AppState>,
) -> std::result::Result<LlmSettings, String> {
    map_error(state.database.get_llm_settings())
}

#[tauri::command]
pub async fn update_llm_settings(
    state: State<'_, AppState>,
    settings: LlmSettings,
) -> std::result::Result<LlmSettings, String> {
    map_error(state.database.update_llm_settings(&settings))
}

