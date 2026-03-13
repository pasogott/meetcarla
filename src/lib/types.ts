export type RecordingState = "idle" | "recording" | "finalizing";

export type MeetingStatus = "recording" | "processing" | "completed" | "failed";

export interface RecordingStatus {
  state: RecordingState;
  meetingId: string | null;
  startedAt: string | null;
  durationSeconds: number;
}

export interface PermissionStatus {
  microphone: boolean;
  screenRecording: boolean;
}

export interface MeetingSummary {
  id: string;
  title: string;
  startedAt: string;
  durationSeconds: number;
  platform: string;
  status: MeetingStatus;
  segmentCount: number;
  tags: MeetingTag[];
  calendarEventTitle: string | null;
}

export interface TranscriptSegment {
  id: string;
  meetingId: string;
  startTime: number;
  endTime: number;
  text: string;
  speaker: string | null;
  language: string;
}

export interface MeetingJob {
  id: string;
  meetingId: string;
  kind: string;
  status: string;
  errorMessage: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface TranscriptionModel {
  id: string;
  name: string;
  family: "mlx" | "whisper";
  sizeMb: number;
  installed: boolean;
  active: boolean;
  downloadProgress: number | null;
}

export interface Task {
  id: string;
  meetingId: string;
  text: string;
  assignee: string | null;
  completed: boolean;
  position: number;
  createdAt: string;
  updatedAt: string;
}

export interface SummaryResult {
  summary: string;
  tasks: TaskExtraction[];
}

export interface TaskExtraction {
  text: string;
  assignee: string | null;
}

export interface ProcessingStep {
  step: "transcribing" | "summarizing" | "extracting_tasks" | "completed";
  progress: number;
}

export interface MeetingDetail extends MeetingSummary {
  audioFilePath: string | null;
  transcriptSegments: TranscriptSegment[];
  jobs: MeetingJob[];
  summaryText: string | null;
  scratchpad: string | null;
  tasks: Task[];
  speakers: MeetingSpeaker[];
}

export interface SearchResult {
  meetingId: string;
  meetingTitle: string;
  segmentId: string;
  startTime: number;
  endTime: number;
  snippet: string;
}

export interface AppSettings {
  selectedInputDevice: string;
  selectedOutputDevice: string;
  selectedTranscriptionModel: string;
  primaryLanguage: string;
  storagePath: string;
  launchAtLogin: boolean;
}

export interface LlmSettings {
  apiKey: string;
  provider: string;
  model: string;
  detailLevel: string;
}

export interface PlaybackState {
  meetingId: string | null;
  mediaPath: string | null;
  positionSeconds: number;
  durationSeconds: number;
  isPlaying: boolean;
  error: string | null;
}

export interface AlertEvent {
  level: "info" | "warning" | "error" | "success";
  title: string;
  message: string;
}

export interface NativeHelperStatus {
  mode: "stub" | "connected";
  executablePath: string | null;
  lastError: string | null;
}

export interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
  isInput: boolean;
}

export interface Speaker {
  id: string;
  name: string;
  createdAt: string;
  updatedAt: string;
}

export interface MeetingSpeaker {
  meetingId: string;
  speakerLabel: string;
  speakerId: string | null;
  speakerName: string | null;
  clipPath: string | null;
}

export interface SummaryTemplate {
  id: string;
  name: string;
  promptTemplate: string;
  isDefault: boolean;
  createdAt: string;
}

export interface Tag {
  id: string;
  name: string;
  color: string;
  createdAt: string;
}

export interface MeetingTag {
  meetingId: string;
  tagId: string;
  tagName: string;
  tagColor: string;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  meetingReferences: string[] | null;
  createdAt: string;
}

export interface MeetingReference {
  meetingId: string;
  meetingTitle: string;
  relevantExcerpt: string;
}

export interface AskAiResponse {
  answer: string;
  meetingReferences: MeetingReference[];
}

export interface CopyContent {
  plainText: string;
  html: string;
}

export interface CalendarEvent {
  title: string;
  startTime: string;
  endTime: string;
  isMeeting: boolean;
}

export interface Webhook {
  id: string;
  name: string;
  url: string;
  events: string[];
  secret: string | null;
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface WebhookDelivery {
  id: string;
  webhookId: string;
  eventType: string;
  payload: string;
  responseStatus: number | null;
  responseBody: string | null;
  success: boolean;
  createdAt: string;
}

