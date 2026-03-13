# PRD — Carla in a Tauri Setup

## 1. Vision

Carla is a privacy-first desktop app that records meetings locally, transcribes them on-device, and makes them searchable without sending audio to the cloud.

In a Tauri setup, Carla is framed as:

- a **system tray desktop app** with dedicated windows for meetings, transcripts, and settings
- a **web-based frontend** for all product UI
- a **local backend core** that orchestrates recording, transcription, playback, search, and persistence
- a set of **native macOS adapters** for platform-specific capabilities such as system audio capture and permissions

**One-liner:** _Your local meeting recorder and transcript workspace, packaged as a Tauri desktop app._

## 2. Product Goals

Carla should let a user:

1. Record microphone and system audio from a meeting running on their Mac
2. See live transcript updates while recording
3. Re-open any meeting later with transcript, playback, search, export, and delete actions
4. Keep all core functionality local to the device

## 3. Non-Goals

This PRD does not assume:

- cloud recording infrastructure
- cloud-first transcription
- browser-only operation without native desktop privileges
- a fully cross-platform implementation in the current product state

Although the UI shell is described through Tauri, the product remains **macOS-first** because audio capture, permissions, and local MLX inference depend on native macOS capabilities.

## 4. Target Users

| Persona | Need |
|---|---|
| Freelancers / Consultants | Record client calls privately without bots joining |
| Sales Teams | Keep searchable records of calls without sending audio to third parties |
| Managers | Review discussions, decisions, and follow-ups across many meetings |
| Legal / Compliance-sensitive teams | Prefer local-only storage and processing |
| Developers | Search past standups, planning calls, and technical discussions |

## 5. Product Principles

1. **Privacy by default** — audio and transcripts remain local unless the user explicitly opts into something else later
2. **Desktop-native capability** — the app must use native OS integrations where web APIs are insufficient
3. **Low-friction capture** — starting and stopping a recording should be immediate
4. **Local-first retrieval** — search, playback, transcript browsing, and export must work offline
5. **Clear backend/frontend boundaries** — the Tauri frontend renders state, while backend services own recording, storage, and AI execution

## 6. Core User Experience

### 6.1 App Shell

The product is presented as a Tauri desktop app with:

- a system tray entry point
- a tray menu with primary actions
- dedicated windows for:
  - Meetings
  - Transcript viewer
  - Settings

The tray is the always-available control surface. The larger windows are used for browsing and editing meeting data.

### 6.2 Recording Flow

1. User opens the tray menu
2. User starts recording
3. The backend requests or validates microphone and screen recording permissions
4. Native adapters begin capturing:
   - microphone input
   - system audio output
5. The backend persists a meeting record immediately for crash recovery
6. Live transcript updates stream into the frontend
7. User stops recording
8. Post-recording transcription finalization runs locally
9. The meeting appears in the Meetings window with transcript, duration, and playback data

### 6.3 Review Flow

1. User opens Meetings
2. User selects a meeting
3. Carla shows transcript segments, timestamps, and playback state
4. User can:
   - rename the meeting
   - search meetings and transcripts
   - jump to timestamps
   - copy transcript text
   - export to local files
   - delete the meeting and associated audio

## 7. Functional Scope

### 7.1 Implemented / Current-State Capabilities

- system tray application shell
- separate meetings, transcript, and settings windows
- microphone audio capture
- system audio capture on macOS
- local persistence of meetings and transcript data
- live transcription during recording
- post-recording polish/finalization
- local full-text transcript search
- audio playback with timestamp seeking
- meeting title editing
- model management and local model download UI
- export in Markdown, TXT, SRT, and JSON
- delete flow with confirmation

### 7.2 Future-Facing Capabilities

These are product directions, not current guarantees:

- local meeting summaries and action items
- calendar integration
- auto-recording
- richer speaker identification
- broader desktop portability beyond macOS

## 8. Tauri-Oriented System Architecture

### 8.1 Frontend Layer

The frontend is a Tauri-rendered web UI responsible for:

- tray-driven window navigation
- meetings list rendering
- transcript rendering
- playback controls
- settings forms
- model download progress display
- user-visible job and error states

The frontend should not directly own recording or persistence logic. It should consume state from backend commands and events.

### 8.2 Backend Core

The backend core is the local orchestration layer responsible for:

- application state coordination
- recording lifecycle
- transcription job lifecycle
- meeting retrieval and search
- playback state synchronization
- model selection and download workflows
- alert and error propagation

In the current Swift implementation, this role is effectively split across `AppState` and `RecordingCoordinator`. In a Tauri setup, that responsibility moves behind Rust backend services and Tauri command handlers.

For the agreed v1 implementation:

- Rust is the source of truth for application state, persistence, search, playback, export, and command/event orchestration
- the Rust backend supervises native helper processes and translates native events into frontend-safe application events
- the frontend never talks directly to native macOS adapters

### 8.3 Native macOS Adapters

Some product features cannot be implemented as generic web functionality and remain native integrations:

- microphone capture
- system audio capture
- screen recording permission checks and prompts
- microphone permission checks and prompts
- tray-specific desktop behavior
- Apple Silicon-dependent local MLX execution

These adapters are product-critical and should be treated as first-class backend dependencies, not incidental implementation details.

For the agreed v1 implementation, these adapters are delivered through a thin Swift helper boundary rather than a Swift-owned app core. That helper is responsible only for macOS-critical capabilities that are impractical to ship as a Rust-first implementation in the first version.

### 8.4 Local AI and Processing Adapters

Carla uses local inference and processing runtimes behind backend adapters:

- realtime and post-recording Whisper-based transcription
- local model validation and download management
- future local summarization runtime

The frontend should only understand high-level model availability, progress, and error states.

For v1, Apple Silicon MLX is the primary transcription runtime. Broader runtime portability is deferred until after the core macOS-first build is working end to end.

## 9. Capability Mapping from Current App to Tauri Model

### 9.1 App Shell Mapping

Current app concepts:

- menu bar app shell
- meetings window
- transcript window
- settings window

Tauri framing:

- system tray menu
- tray-triggered windows
- frontend routes or distinct window entry points

### 9.2 State and Orchestration Mapping

Current app concepts:

- global app state
- recording state
- live transcript stream
- playback state
- model download state

Tauri framing:

- backend-owned application state
- command-based mutations
- event-driven frontend updates

### 9.3 Recording Mapping

Current app concepts:

- recording coordinator
- microphone and system audio capturers
- audio session and recorder
- realtime transcription jobs

Tauri framing:

- backend recording orchestration service
- native capture adapters
- streamed events for transcript and audio levels

### 9.4 Permissions Mapping

Current app concepts:

- microphone permission manager
- screen recording permission manager
- open system settings helpers

Tauri framing:

- native permission service
- frontend permission UI that invokes backend permission commands

### 9.5 Storage and Search Mapping

Current app concepts:

- SQLite via GRDB
- meeting repository
- FTS transcript search

Tauri framing:

- local SQLite persistence service
- repository-style backend data access
- command surface for listing meetings, fetching details, searching, exporting, and deleting

## 10. Backend Command Surface

The exact command names are implementation details, but the product requires these backend capability groups:

### 10.1 Recording Commands

- start recording
- stop recording
- query current recording state

### 10.2 Permission Commands

- check all required permissions
- request microphone permission
- request screen recording permission
- open relevant OS settings panes

### 10.3 Meeting Commands

- list meetings
- fetch meeting details
- rename a meeting
- delete a meeting
- export meeting data

### 10.4 Search Commands

- search transcript content
- filter meetings by local title match when full-text search is unavailable

### 10.5 Settings and Model Commands

- read and update app settings
- refresh installed models
- download or cancel model downloads
- select active transcription model

### 10.6 Playback Commands

- load meeting audio
- play / pause
- seek to timestamp
- expose current time, duration, and playback errors

## 11. Event Streams to Frontend

The frontend needs push-style updates for:

- recording state changes
- live transcript segment updates
- audio level updates
- meeting job status changes
- model download progress
- playback position and duration updates
- user-visible alerts and recoverable failures

Without these event streams, the frontend would be forced into inefficient polling for highly dynamic UI states.

## 12. Domain Model

### Meeting

- `id`
- `title`
- `startedAt`
- `duration`
- `audioFilePath`
- `platform`

### TranscriptSegment

- `id`
- `meetingID`
- `startTime`
- `endTime`
- `text`
- `speaker`
- `language`

### Speaker

- `id`
- `meetingID`
- `label`
- `isLocal`

### MeetingJob

- `id`
- `meetingID`
- `type`
- `status`
- `errorMessage`
- `timestamps`

### Settings / Model State

- selected input/output device
- selected transcription model
- primary language
- storage path
- launch or onboarding preferences
- model availability and download progress

## 13. Platform Constraints

### 13.1 OS Constraints

Current practical target:

- macOS 14+

The Tauri packaging layer does not remove the need for:

- native microphone access
- native system audio capture
- native privacy permission flows

### 13.2 Hardware Constraints

Local MLX transcription depends on Apple Silicon support. The product should clearly surface unsupported hardware states and recovery guidance.

### 13.3 Privacy Constraints

Core product behavior must remain local-first:

- recordings stored locally
- transcripts stored locally
- search performed locally
- model execution performed locally where supported

## 14. UX and Reliability Requirements

- The tray must always expose the current high-level recording status
- Users must be told when permissions block recording
- Recording failures and post-processing failures must surface clearly in the UI
- Meetings must remain browseable after app restarts
- Search should remain available offline
- Renaming a meeting must feel instant in the UI and persist safely
- Deleting a meeting must require confirmation and keep the app in a valid selection state

## 15. Success Criteria

Carla is successful in a Tauri setup when:

1. A user can control the full core workflow from tray to transcript without leaving the desktop app
2. The frontend remains a thin presentation layer over local backend capabilities
3. Native macOS requirements are isolated behind backend adapters instead of leaking into frontend logic
4. Local-first recording, transcription, storage, and search remain the default product behavior

## 16. Open Product Directions

- local summarization and structured action items
- calendar-linked meeting context
- smarter automatic meeting detection
- richer multi-speaker workflows
- longer-term portability beyond a macOS-only deployment target

These should be designed as extensions of the same Tauri pattern:

- web UI for presentation
- backend commands and events for coordination
- native adapters only where platform access is required

## 17. Agreed Implementation Plan

### 17.1 Delivery Summary

The first delivery is a greenfield Tauri v2 desktop application with:

- a React + TypeScript + Vite frontend
- a Rust backend core inside Tauri
- a thin Swift helper for system audio capture, permission helpers, and MLX-backed transcription

The implementation target includes the full current PRD scope, but work should land in dependency order so recording, transcription, and persistence are stable before secondary UX layers.

### 17.2 Locked Architecture Decisions

- Tauri v2 is the desktop shell and window/tray host
- Rust owns backend services, domain state, repositories, SQLite access, playback, exports, and frontend-facing command/event contracts
- Swift is allowed only as a thin native helper for macOS-critical features that are high-risk in a Rust-only v1
- the primary target is macOS 14+ on Apple Silicon
- MLX is the default local transcription runtime for v1

### 17.3 Implementation Order

1. Bootstrap the Tauri app shell, tray, and three-window structure for meetings, transcript, and settings
2. Implement the Rust service layer, shared domain types, SQLite persistence, and Tauri command/event plumbing
3. Add the Swift helper process and define a narrow IPC contract for permissions, system audio capture, and transcription jobs
4. Deliver the recording flow end to end: permission checks, mic + system audio capture, live transcript events, local audio persistence, and post-recording finalization
5. Deliver meetings and transcript review flows: list/detail fetch, transcript rendering, rename, delete, and valid selection recovery
6. Deliver playback, search, export, and model management on top of the persisted meeting data model
7. Harden restart recovery, alert/error surfaces, unsupported hardware handling, and offline behavior

### 17.4 Backend Interface Contract

The backend must expose command groups for:

- recording: start, stop, get current recording state
- permissions: check, request microphone, request screen recording, open system settings
- meetings: list, fetch detail, rename, delete, export
- search: transcript search and meeting filtering
- settings and models: read/update settings, refresh models, download/cancel downloads, select active model
- playback: load audio, play, pause, seek, and report position/duration/errors

The backend must emit events for:

- recording state changes
- live transcript segment updates
- audio level updates
- meeting job status updates
- model download progress
- playback state updates
- user-visible alerts and recoverable failures

### 17.5 Test and Acceptance Plan

Rust unit and integration coverage should include:

- repository behavior and SQLite FTS queries
- recording and playback state transitions
- export formatting for Markdown, TXT, SRT, and JSON
- command handler behavior and event emission
- Swift-helper IPC parsing, supervision, and failure handling

Manual acceptance on Apple Silicon macOS should confirm:

- tray-driven start/stop recording works
- permission denial states surface recovery actions
- live transcript updates stream during recording
- meetings remain available after restart
- playback seeking, transcript search, rename, export, and delete all work offline
- model downloads and selection survive relaunch and fail clearly when unavailable
