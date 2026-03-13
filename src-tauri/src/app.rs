use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, RunEvent, Size,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::commands;
use crate::state::AppState;
use crate::types::AlertEvent;

const NOTCH_LABEL: &str = "recording-notch";
const NOTCH_ROUTE: &str = "notch";
const NOTCH_WIDTH: f64 = 200.0;
const NOTCH_HEIGHT: f64 = 60.0;
const NOTCH_TOP_MARGIN: f64 = 32.0;

fn app_route(route: &str) -> String {
    if route.is_empty() {
        "/".into()
    } else {
        format!("/#/{route}")
    }
}

fn ensure_window(app: &AppHandle, label: &str, title: &str, route: &str) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(label) {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, label, WebviewUrl::App(app_route(route).into()))
        .title(title)
        .inner_size(1280.0, 860.0)
        .resizable(true)
        .build()?;
    Ok(())
}

fn notch_position(app: &AppHandle) -> tauri::Result<Option<Position>> {
    let monitor = app
        .get_webview_window("main")
        .and_then(|window| window.current_monitor().ok().flatten())
        .or_else(|| {
            app.available_monitors()
                .ok()
                .and_then(|mut monitors| monitors.drain(..).next())
        });

    let Some(monitor) = monitor else {
        return Ok(None);
    };

    let scale_factor = monitor.scale_factor();
    let width = (NOTCH_WIDTH * scale_factor).round() as i32;
    let right_margin = (12.0 * scale_factor).round() as i32;
    let x = monitor.position().x + (monitor.size().width as i32 - width - right_margin);
    let y = monitor.position().y + (NOTCH_TOP_MARGIN * scale_factor).round() as i32;

    Ok(Some(Position::Physical(PhysicalPosition::new(x, y))))
}

fn layout_notch_window(app: &AppHandle, window: &WebviewWindow) -> tauri::Result<()> {
    window.set_size(Size::Logical(LogicalSize::new(NOTCH_WIDTH, NOTCH_HEIGHT)))?;
    if let Some(position) = notch_position(app)? {
        window.set_position(position)?;
    }
    Ok(())
}

fn ensure_notch_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(NOTCH_LABEL) {
        layout_notch_window(app, &window)?;
        return Ok(window);
    }

    let window = WebviewWindowBuilder::new(
        app,
        NOTCH_LABEL,
        WebviewUrl::App(app_route(NOTCH_ROUTE).into()),
    )
    .title("Recording Notch")
    .inner_size(NOTCH_WIDTH, NOTCH_HEIGHT)
    .min_inner_size(NOTCH_WIDTH, NOTCH_HEIGHT)
    .max_inner_size(NOTCH_WIDTH, NOTCH_HEIGHT)
    .resizable(false)
    .minimizable(false)
    .maximizable(false)
    .closable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .focusable(false)
    .focused(false)
    .visible(false)
    .build()?;

    layout_notch_window(app, &window)?;
    Ok(window)
}

pub fn show_recording_notch(app: &AppHandle) -> tauri::Result<()> {
    let window = ensure_notch_window(app)?;
    layout_notch_window(app, &window)?;
    window.show()?;
    Ok(())
}

pub fn hide_recording_notch(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(NOTCH_LABEL) {
        window.hide()?;
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(AppState::new(app.handle()).expect("failed to initialize app state"));
            let open_meetings =
                MenuItem::with_id(app, "open-meetings", "Meetings", true, None::<&str>)?;
            let open_transcript =
                MenuItem::with_id(app, "open-transcript", "Transcript", true, None::<&str>)?;
            let open_settings =
                MenuItem::with_id(app, "open-settings", "Settings", true, None::<&str>)?;
            let start_recording = MenuItem::with_id(
                app,
                "start-recording",
                "Start recording",
                true,
                None::<&str>,
            )?;
            let stop_recording =
                MenuItem::with_id(app, "stop-recording", "Stop recording", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &open_meetings,
                    &open_transcript,
                    &open_settings,
                    &start_recording,
                    &stop_recording,
                    &quit,
                ],
            )?;
            #[cfg(target_os = "macos")]
            let tray_icon = tauri::include_image!("icons/tray-icon-template.png");
            #[cfg(not(target_os = "macos"))]
            let tray_icon = tauri::include_image!("icons/icon.png");

            let mut tray_builder = TrayIconBuilder::with_id("carla-tray")
                .icon(tray_icon)
                .menu(&menu)
                .tooltip("Carla")
                .show_menu_on_left_click(true);
            #[cfg(target_os = "macos")]
            {
                tray_builder = tray_builder.icon_as_template(true);
            }

            tray_builder
                .on_menu_event(|app: &AppHandle, event: MenuEvent| {
                    let state = app.state::<AppState>().inner().clone();
                    match event.id().as_ref() {
                        "open-meetings" => {
                            let _ = ensure_window(app, "main", "Carla", "");
                        }
                        "open-transcript" => {
                            let _ = ensure_window(app, "transcript", "Transcript", "transcript");
                        }
                        "open-settings" => {
                            let _ = ensure_window(app, "settings", "Settings", "settings");
                        }
                        "start-recording" => {
                            let app_handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(error) =
                                    crate::recording::start_recording(state, app_handle.clone())
                                        .await
                                {
                                    let _ = app_handle.emit(
                                        "user-alert",
                                        AlertEvent {
                                            level: "error".into(),
                                            title: "Recording failed".into(),
                                            message: error.to_string(),
                                        },
                                    );
                                }
                            });
                        }
                        "stop-recording" => {
                            let app_handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(error) =
                                    crate::recording::stop_recording(state, app_handle.clone())
                                        .await
                                {
                                    let _ = app_handle.emit(
                                        "user-alert",
                                        AlertEvent {
                                            level: "error".into(),
                                            title: "Stop failed".into(),
                                            message: error.to_string(),
                                        },
                                    );
                                }
                            });
                        }
                        "quit" => {
                            let app_handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                // Stop any active recording before quitting
                                let state = app_handle.state::<AppState>().inner().clone();
                                let has_recording = {
                                    let recording = state.recording.lock().await;
                                    recording.active.is_some()
                                };
                                if has_recording {
                                    let _ = crate::recording::stop_recording(
                                        state,
                                        app_handle.clone(),
                                    )
                                    .await;
                                }
                                app_handle.exit(0);
                            });
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            ensure_window(app.handle(), "transcript", "Transcript", "transcript")?;
            ensure_window(app.handle(), "settings", "Settings", "settings")?;
            ensure_notch_window(app.handle())?;
            if let Some(window) = app.get_webview_window("transcript") {
                window.hide()?;
            }
            if let Some(window) = app.get_webview_window("settings") {
                window.hide()?;
            }
            hide_recording_notch(app.handle())?;

            // Spawn background video call detection loop
            let detection_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = detection_app.state::<AppState>().inner().clone();
                let mut detector = crate::detection::VideoCallDetector::new();
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
                loop {
                    interval.tick().await;

                    // Skip if recording is already active
                    {
                        let recording = state.recording.lock().await;
                        if recording.active.is_some() {
                            continue;
                        }
                    }

                    // Skip if detection is disabled
                    let detection_settings = match state.database.get_detection_settings() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    if !detection_settings.enabled {
                        continue;
                    }

                    // Check for newly launched video call apps
                    if let Some(app_name) = detector.check_for_new_calls() {
                        // Skip if this app has been disabled by the user
                        if detection_settings
                            .disabled_apps
                            .iter()
                            .any(|d| d == &app_name)
                        {
                            continue;
                        }
                        let _ = detection_app.emit("video-call-detected", &app_name);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_recording_state,
            commands::start_recording,
            commands::stop_recording,
            commands::check_permissions,
            commands::request_microphone_permission,
            commands::request_screen_recording_permission,
            commands::open_system_settings,
            commands::list_meetings,
            commands::get_meeting_detail,
            commands::rename_meeting,
            commands::delete_meeting,
            commands::delete_transcript,
            commands::open_meeting_media,
            commands::search_transcripts,
            commands::export_meeting,
            commands::get_settings,
            commands::list_models,
            commands::download_model,
            commands::select_model,
            commands::update_settings,
            commands::load_playback,
            commands::play_playback,
            commands::pause_playback,
            commands::seek_playback,
            commands::get_playback_state,
            commands::get_native_helper_status,
            commands::list_audio_devices,
            commands::summarize_meeting,
            commands::list_tasks,
            commands::create_task,
            commands::update_task,
            commands::delete_task,
            commands::toggle_task,
            commands::update_scratchpad,
            commands::get_detection_settings,
            commands::update_detection_settings,
            commands::force_quit,
            commands::stop_and_quit,
            commands::list_templates,
            commands::create_template,
            commands::update_template,
            commands::delete_template,
            commands::set_default_template,
            commands::list_speakers,
            commands::create_speaker,
            commands::rename_speaker,
            commands::delete_speaker,
            commands::list_meeting_speakers,
            commands::assign_speaker_to_meeting,
            commands::extract_speaker_clips,
            commands::ask_ai,
            commands::list_chat_messages,
            commands::clear_chat_history,
            commands::list_tags,
            commands::create_tag,
            commands::update_tag,
            commands::delete_tag,
            commands::add_tag_to_meeting,
            commands::remove_tag_from_meeting,
            commands::list_meetings_by_tag,
            commands::copy_summary,
            commands::copy_transcript,
            commands::copy_tasks,
            commands::list_calendar_events,
            commands::link_meeting_to_calendar,
            commands::update_summary,
            commands::update_transcript_segment,
            commands::list_webhooks,
            commands::create_webhook,
            commands::update_webhook,
            commands::delete_webhook,
            commands::list_webhook_deliveries,
            commands::test_webhook,
            commands::get_llm_settings,
            commands::update_llm_settings
        ])
        .on_window_event(|window, event| {
            // Hide windows on close instead of destroying them (except notch)
            if let WindowEvent::CloseRequested { api, .. } = event {
                let label = window.label();
                if label != NOTCH_LABEL {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Keep the app alive when all windows are closed (tray stays active)
            if let RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
