import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type {
  AlertEvent,
  AppSettings,
  AudioDevice,
  AskAiResponse,
  CalendarEvent,
  ChatMessage,
  CopyContent,
  LlmSettings,
  MeetingDetail,
  MeetingSpeaker,
  MeetingSummary,
  NativeHelperStatus,
  PermissionStatus,
  PlaybackState,
  RecordingStatus,
  SearchResult,
  Speaker,
  SummaryTemplate,
  Tag,
  Task,
  TranscriptionModel,
  Webhook,
  WebhookDelivery,
} from "./types";

export const commands = {
  getRecordingState: () => invoke<RecordingStatus>("get_recording_state"),
  startRecording: () => invoke<RecordingStatus>("start_recording"),
  stopRecording: () => invoke<RecordingStatus>("stop_recording"),
  checkPermissions: () => invoke<PermissionStatus>("check_permissions"),
  requestMicrophonePermission: () =>
    invoke<PermissionStatus>("request_microphone_permission"),
  requestScreenRecordingPermission: () =>
    invoke<PermissionStatus>("request_screen_recording_permission"),
  openSystemSettings: () => invoke<string>("open_system_settings"),
  listMeetings: (query = "") =>
    invoke<MeetingSummary[]>("list_meetings", { query: query || null }),
  getMeetingDetail: (meetingId: string) =>
    invoke<MeetingDetail>("get_meeting_detail", { meetingId }),
  renameMeeting: (meetingId: string, title: string) =>
    invoke<void>("rename_meeting", { meetingId, title }),
  deleteMeeting: (meetingId: string) =>
    invoke<void>("delete_meeting", { meetingId }),
  deleteTranscript: (meetingId: string) =>
    invoke<void>("delete_transcript", { meetingId }),
  openMeetingMedia: (meetingId: string) =>
    invoke<void>("open_meeting_media", { meetingId }),
  searchTranscripts: (query: string) =>
    invoke<SearchResult[]>("search_transcripts", { query }),
  exportMeeting: (meetingId: string, format: string) =>
    invoke<string>("export_meeting", { meetingId, format }),
  getSettings: () => invoke<AppSettings>("get_settings"),
  listModels: () => invoke<TranscriptionModel[]>("list_models"),
  downloadModel: (modelId: string) =>
    invoke<TranscriptionModel[]>("download_model", { modelId }),
  selectModel: (modelId: string) =>
    invoke<TranscriptionModel[]>("select_model", { modelId }),
  updateSettings: (settings: AppSettings) =>
    invoke<AppSettings>("update_settings", { settings }),
  loadPlayback: (meetingId: string) =>
    invoke<PlaybackState>("load_playback", { meetingId }),
  playPlayback: () => invoke<PlaybackState>("play_playback"),
  pausePlayback: () => invoke<PlaybackState>("pause_playback"),
  seekPlayback: (positionSeconds: number) =>
    invoke<PlaybackState>("seek_playback", { positionSeconds }),
  getPlaybackState: () => invoke<PlaybackState>("get_playback_state"),
  getNativeHelperStatus: () =>
    invoke<NativeHelperStatus>("get_native_helper_status"),
  listAudioDevices: () => invoke<AudioDevice[]>("list_audio_devices"),
  summarizeMeeting: (meetingId: string) =>
    invoke<void>("summarize_meeting", { meetingId }),
  listTasks: (meetingId: string) =>
    invoke<Task[]>("list_tasks", { meetingId }),
  createTask: (meetingId: string, text: string, assignee?: string) =>
    invoke<Task>("create_task", { meetingId, text, assignee: assignee ?? null }),
  updateTask: (taskId: string, text: string, assignee: string | null, completed: boolean) =>
    invoke<void>("update_task", { taskId, text, assignee, completed }),
  deleteTask: (taskId: string) =>
    invoke<void>("delete_task", { taskId }),
  toggleTask: (taskId: string) =>
    invoke<void>("toggle_task", { taskId }),
  updateScratchpad: (meetingId: string, content: string) =>
    invoke<void>("update_scratchpad", { meetingId, content }),
  listSpeakers: () => invoke<Speaker[]>("list_speakers"),
  createSpeaker: (name: string) => invoke<Speaker>("create_speaker", { name }),
  renameSpeaker: (speakerId: string, name: string) =>
    invoke<void>("rename_speaker", { speakerId, name }),
  deleteSpeaker: (speakerId: string) => invoke<void>("delete_speaker", { speakerId }),
  listMeetingSpeakers: (meetingId: string) =>
    invoke<MeetingSpeaker[]>("list_meeting_speakers", { meetingId }),
  assignSpeakerToMeeting: (meetingId: string, speakerLabel: string, speakerId: string) =>
    invoke<void>("assign_speaker_to_meeting", { meetingId, speakerLabel, speakerId }),
  extractSpeakerClips: (meetingId: string) =>
    invoke<void>("extract_speaker_clips", { meetingId }),
  listTemplates: () => invoke<SummaryTemplate[]>("list_templates"),
  createTemplate: (name: string, promptTemplate: string) =>
    invoke<SummaryTemplate>("create_template", { name, promptTemplate }),
  updateTemplate: (templateId: string, name: string, promptTemplate: string) =>
    invoke<void>("update_template", { templateId, name, promptTemplate }),
  deleteTemplate: (templateId: string) => invoke<void>("delete_template", { templateId }),
  setDefaultTemplate: (templateId: string) =>
    invoke<void>("set_default_template", { templateId }),
  summarizeMeetingWithTemplate: (meetingId: string, templateId: string) =>
    invoke<void>("summarize_meeting_with_template", { meetingId, templateId }),

  // Tags
  listTags: () => invoke<Tag[]>("list_tags"),
  createTag: (name: string, color: string) =>
    invoke<Tag>("create_tag", { name, color }),
  updateTag: (tagId: string, name: string, color: string) =>
    invoke<void>("update_tag", { tagId, name, color }),
  deleteTag: (tagId: string) => invoke<void>("delete_tag", { tagId }),
  addTagToMeeting: (meetingId: string, tagId: string) =>
    invoke<void>("add_tag_to_meeting", { meetingId, tagId }),
  removeTagFromMeeting: (meetingId: string, tagId: string) =>
    invoke<void>("remove_tag_from_meeting", { meetingId, tagId }),
  listMeetingsByTag: (tagId: string) =>
    invoke<MeetingSummary[]>("list_meetings_by_tag", { tagId }),

  // Ask AI
  askAi: (question: string, meetingId?: string) =>
    invoke<AskAiResponse>("ask_ai", { question, meetingId: meetingId ?? null }),
  listChatMessages: (limit: number, offset: number) =>
    invoke<ChatMessage[]>("list_chat_messages", { limit, offset }),
  clearChatHistory: () => invoke<void>("clear_chat_history"),

  // Copy with formatting
  copySummary: (meetingId: string) =>
    invoke<CopyContent>("copy_summary", { meetingId }),
  copyTranscript: (meetingId: string) =>
    invoke<CopyContent>("copy_transcript", { meetingId }),
  copyTasks: (meetingId: string) =>
    invoke<CopyContent>("copy_tasks", { meetingId }),

  // Calendar
  listCalendarEvents: (date: string) =>
    invoke<CalendarEvent[]>("list_calendar_events", { date }),
  linkMeetingToCalendar: (meetingId: string, calendarEventTitle: string) =>
    invoke<void>("link_meeting_to_calendar", { meetingId, calendarEventTitle }),

  // Editing
  updateSummary: (meetingId: string, summary: string) =>
    invoke<void>("update_summary", { meetingId, summary }),
  updateTranscriptSegment: (segmentId: string, text: string) =>
    invoke<void>("update_transcript_segment", { segmentId, text }),

  // Webhooks
  listWebhooks: () => invoke<Webhook[]>("list_webhooks"),
  createWebhook: (name: string, url: string, events: string[], secret?: string) =>
    invoke<Webhook>("create_webhook", { name, url, events, secret: secret ?? null }),
  updateWebhook: (webhookId: string, name: string, url: string, events: string[], secret: string | null, enabled: boolean) =>
    invoke<void>("update_webhook", { webhookId, name, url, events, secret, enabled }),
  deleteWebhook: (webhookId: string) => invoke<void>("delete_webhook", { webhookId }),
  listWebhookDeliveries: (webhookId: string, limit: number) =>
    invoke<WebhookDelivery[]>("list_webhook_deliveries", { webhookId, limit }),
  testWebhook: (webhookId: string) => invoke<void>("test_webhook", { webhookId }),

  // LLM Settings
  getLlmSettings: () => invoke<LlmSettings>("get_llm_settings"),
  updateLlmSettings: (settings: LlmSettings) =>
    invoke<LlmSettings>("update_llm_settings", { settings }),
};

export const audioSourceForPath = (filePath: string | null) =>
  filePath ? convertFileSrc(filePath) : null;

export type ExportFormat = "md" | "txt" | "srt" | "json" | "html";
export type AlertItem = AlertEvent;
