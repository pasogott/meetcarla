# Carla

Carla is a local-first Tauri desktop app for recording meetings, streaming live transcripts, searching past calls offline, and exporting transcripts without sending audio to a cloud service.

This repository implements the PRD in a greenfield Tauri v2 stack:

- React + TypeScript frontend
- Rust backend services for persistence, search, exports, tray/windows, and event orchestration
- a thin Swift helper for macOS permissions and meeting capture
- a bundled Python runtime for local MLX and Whisper transcription

## Current implementation

The app currently ships a runnable end-to-end local workflow with:

- system tray entry and dedicated meetings, transcript, and settings windows
- local SQLite persistence for meetings, transcript segments, jobs, settings, model state, and permissions
- real macOS microphone and screen-recording permission prompts
- real microphone + system audio meeting capture on macOS
- real post-recording local transcription with MLX primary and Whisper fallback
- real model download and active-model selection in Settings
- offline full-text transcript search
- export support for Markdown, TXT, SRT, and JSON
- packaged `.app` bundle with helper, `ffmpeg`, transcription runner script, and Python environment

## Run locally

```bash
pnpm install
pnpm tauri dev
```

## Build

```bash
pnpm tauri build --debug --bundles app
```

Artifacts:

- app bundle: `src-tauri/target/debug/bundle/macos/Carla.app`

## Remaining gaps

- live transcript streaming while recording is not implemented yet; transcription currently runs immediately after stop
- in-app MP4 playback is still delegated to the system player because the embedded WebView path is not reliable for the current capture format
- the `.dmg` packaging step is still less reliable than the `.app` bundle because the app now carries a large local runtime
