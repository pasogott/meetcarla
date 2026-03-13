import {
  startTransition,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Link, Route, Routes, useLocation, useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import clsx from "clsx";
import { CarlaLogo } from "./components/CarlaLogo";
import { commands, audioSourceForPath } from "./lib/api";
import type {
  AlertEvent,
  AppSettings,
  AskAiResponse,
  AudioDevice,
  ChatMessage,
  CopyContent,
  LlmSettings,
  MeetingDetail,
  MeetingSpeaker,
  MeetingSummary,
  MeetingTag,
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
} from "./lib/types";

const exportFormats = ["md", "txt", "srt", "json", "html"] as const;

type DetailTab = "summary" | "transcript" | "tasks" | "scratchpad";

// ─── Utility helpers ───────────────────────────────────────────────────────

function formatDuration(seconds: number) {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;
  return [hours, minutes, secs]
    .map((value, index) => (index === 0 ? String(value) : String(value).padStart(2, "0")))
    .join(":");
}

function formatClock(timestamp: string) {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(timestamp));
}

/**
 * Minimal markdown renderer - handles headers, bold, lists, paragraphs.
 * No external dependencies; pure string parsing.
 */
function renderMarkdown(text: string): React.ReactNode[] {
  const lines = text.split("\n");
  const nodes: React.ReactNode[] = [];
  let listItems: string[] = [];
  let keyCounter = 0;

  const flushList = () => {
    if (listItems.length === 0) return;
    nodes.push(
      <ul key={`ul-${keyCounter++}`} className="md-list">
        {listItems.map((item, i) => (
          <li key={i}>{renderInline(item)}</li>
        ))}
      </ul>,
    );
    listItems = [];
  };

  const renderInline = (line: string): React.ReactNode => {
    // Bold: **text**
    const parts = line.split(/(\*\*[^*]+\*\*)/g);
    if (parts.length === 1) return line;
    return parts.map((part, i) => {
      if (part.startsWith("**") && part.endsWith("**")) {
        return <strong key={i}>{part.slice(2, -2)}</strong>;
      }
      return part;
    });
  };

  for (const line of lines) {
    const trimmed = line.trim();

    if (trimmed === "") {
      flushList();
      continue;
    }

    const h1Match = /^# (.+)/.exec(trimmed);
    const h2Match = /^## (.+)/.exec(trimmed);
    const h3Match = /^### (.+)/.exec(trimmed);
    const listMatch = /^[-*] (.+)/.exec(trimmed);

    if (h1Match) {
      flushList();
      nodes.push(<h1 key={keyCounter++} className="md-h1">{h1Match[1]}</h1>);
    } else if (h2Match) {
      flushList();
      nodes.push(<h2 key={keyCounter++} className="md-h2">{h2Match[1]}</h2>);
    } else if (h3Match) {
      flushList();
      nodes.push(<h3 key={keyCounter++} className="md-h3">{h3Match[1]}</h3>);
    } else if (listMatch) {
      listItems.push(listMatch[1]);
    } else {
      flushList();
      nodes.push(<p key={keyCounter++} className="md-p">{renderInline(trimmed)}</p>);
    }
  }

  flushList();
  return nodes;
}

// ─── Clipboard helper ────────────────────────────────────────────────────────

async function copyToClipboard(content: CopyContent) {
  const blob = new Blob([content.html], { type: "text/html" });
  const textBlob = new Blob([content.plainText], { type: "text/plain" });
  await navigator.clipboard.write([
    new ClipboardItem({
      "text/html": blob,
      "text/plain": textBlob,
    }),
  ]);
}

function CopyButton({ onCopy }: { onCopy: () => Promise<void> }) {
  const [state, setState] = useState<"idle" | "copied" | "error">("idle");
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const handleClick = async () => {
    try {
      await onCopy();
      setState("copied");
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setState("idle"), 2000);
    } catch {
      setState("error");
      if (timerRef.current) clearTimeout(timerRef.current);
      timerRef.current = setTimeout(() => setState("idle"), 2000);
    }
  };

  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  return (
    <button
      className={clsx("copy-button", state === "copied" && "copy-button--copied", state === "error" && "copy-button--error")}
      onClick={() => void handleClick()}
      aria-label="Copy to clipboard"
      title={state === "copied" ? "Copied!" : state === "error" ? "Copy failed" : "Copy"}
    >
      {state === "copied" ? (
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
          <path d="M2 7L5.5 10.5L12 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      ) : (
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
          <rect x="5" y="1" width="8" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.25" />
          <path d="M9 10v2a1.5 1.5 0 0 1-1.5 1.5H2A1.5 1.5 0 0 1 .5 10.5V5A1.5 1.5 0 0 1 2 3.5h2" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
        </svg>
      )}
      <span className="copy-button-label">{state === "copied" ? "Copied!" : state === "error" ? "Failed" : "Copy"}</span>
    </button>
  );
}

// ─── App shell ─────────────────────────────────────────────────────────────

function AppShell({
  recording,
  alerts,
  onDismissAlert,
  onStart,
  onStop,
  children,
}: {
  recording: RecordingStatus;
  alerts: AlertEvent[];
  onDismissAlert: (index: number) => void;
  onStart: () => Promise<void>;
  onStop: () => Promise<void>;
  children: React.ReactNode;
}) {
  const location = useLocation();

  return (
    <div className="app-shell">
      <a className="skip-link" href="#workspace-content">
        Skip to content
      </a>

      {/* Left nav sidebar */}
      <aside className="app-sidebar">
        <div className="sidebar-titlebar" data-tauri-drag-region="">
          <Link className="brand-link" to="/">
            <CarlaLogo className="brand-logo" compact />
            <span>Carla</span>
          </Link>
        </div>
        <nav className="sidebar-nav" aria-label="Primary">
          <Link
            aria-current={location.pathname === "/" ? "page" : undefined}
            className={clsx("sidebar-nav-link", location.pathname === "/" && "active")}
            to="/"
          >
            Meetings
          </Link>
          <Link
            aria-current={location.pathname === "/transcript" ? "page" : undefined}
            className={clsx("sidebar-nav-link", location.pathname === "/transcript" && "active")}
            to="/transcript"
          >
            Transcript
          </Link>
          <Link
            aria-current={location.pathname === "/ask-ai" ? "page" : undefined}
            className={clsx("sidebar-nav-link", location.pathname === "/ask-ai" && "active")}
            to="/ask-ai"
          >
            Ask AI
          </Link>
          <Link
            aria-current={location.pathname === "/settings" ? "page" : undefined}
            className={clsx("sidebar-nav-link", location.pathname === "/settings" && "active")}
            to="/settings"
          >
            Settings
          </Link>
        </nav>

        <div className="sidebar-footer">
          {recording.state !== "idle" ? (
            <div className="recording-indicator">
              <span className="recording-dot" aria-hidden="true" />
              <span className="recording-timer">
                {recording.state === "recording"
                  ? formatDuration(recording.durationSeconds)
                  : "Finalizing..."}
              </span>
            </div>
          ) : null}
          <button
            className={clsx(
              "record-button",
              recording.state !== "idle" && "recording-active",
            )}
            onClick={() => void (recording.state === "idle" ? onStart() : onStop())}
          >
            <span className="record-button-dot" aria-hidden="true" />
            {recording.state === "idle" ? "Start recording" : "Stop recording"}
          </button>
        </div>
      </aside>

      {/* Main content */}
      <main className="workspace" id="workspace-content" tabIndex={-1}>
        <div className="workspace-titlebar" data-tauri-drag-region="" />
        {children}
      </main>

      {/* Toasts */}
      {alerts.length > 0 ? (
        <div aria-live="polite" className="alert-stack">
          {alerts.slice(0, 3).map((alert, index) => (
            <div
              key={`${alert.title}-${index}`}
              className={clsx("alert-card", alert.level)}
              onAnimationEnd={(event) => {
                if (event.animationName === "toast-out") {
                  onDismissAlert(index);
                }
              }}
            >
              <div className="alert-content">
                <strong>{alert.title}</strong>
                <p>{alert.message}</p>
              </div>
              <button
                className="alert-dismiss"
                aria-label="Dismiss"
                onClick={() => onDismissAlert(index)}
              >
                <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                  <path d="M3 3L11 11M11 3L3 11" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                </svg>
              </button>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

// ─── Processing progress ────────────────────────────────────────────────────

const PROCESSING_STEPS = [
  { key: "transcribing", label: "Transcribing audio" },
  { key: "summarizing", label: "Generating summary" },
  { key: "extracting_tasks", label: "Extracting tasks" },
] as const;

function ProcessingProgress({ detail }: { detail: MeetingDetail }) {
  const [elapsed, setElapsed] = useState(0);
  const startRef = useRef(Date.now());

  useEffect(() => {
    const id = setInterval(() => {
      setElapsed(Math.floor((Date.now() - startRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, []);

  const runningJob = detail.jobs.find((job) => job.status === "running");
  const currentStep =
    runningJob?.kind === "transcription"
      ? "transcribing"
      : runningJob?.kind === "summarization"
        ? "summarizing"
        : runningJob?.kind === "task_extraction"
          ? "extracting_tasks"
          : null;

  const completedKinds = new Set(
    detail.jobs.filter((job) => job.status === "completed").map((job) => job.kind),
  );

  const stepCompleted = (key: string) => {
    if (key === "transcribing") return completedKinds.has("transcription");
    if (key === "summarizing") return completedKinds.has("summarization");
    if (key === "extracting_tasks") return completedKinds.has("task_extraction");
    return false;
  };

  return (
    <div className="processing-view">
      <div className="processing-header">
        <strong>Processing meeting</strong>
        <span className="processing-elapsed">{formatDuration(elapsed)}</span>
      </div>
      <ol className="processing-steps" aria-label="Processing steps">
        {PROCESSING_STEPS.map((step, index) => {
          const isActive = currentStep === step.key;
          const isDone = stepCompleted(step.key);
          return (
            <li
              key={step.key}
              className={clsx(
                "processing-step",
                isActive && "active",
                isDone && "done",
              )}
              aria-current={isActive ? "step" : undefined}
            >
              <span className="processing-step-indicator" aria-hidden="true">
                {isDone ? (
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
                    <path d="M2 6L5 9L10 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                ) : (
                  <span>{index + 1}</span>
                )}
              </span>
              <span className="processing-step-label">{step.label}</span>
              {isActive ? <span className="processing-spinner" aria-hidden="true" /> : null}
            </li>
          );
        })}
      </ol>
    </div>
  );
}

// ─── Summary tab ────────────────────────────────────────────────────────────

// ─── Markdown toolbar ────────────────────────────────────────────────────────

function TextToolbar({ textareaRef, onUpdate }: {
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  onUpdate: (value: string) => void;
}) {
  const wrapSelection = (prefix: string, suffix: string) => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const value = textarea.value;
    const selected = value.substring(start, end);
    const replacement = prefix + selected + suffix;
    const next = value.substring(0, start) + replacement + value.substring(end);
    onUpdate(next);
    // Restore cursor after React re-render
    requestAnimationFrame(() => {
      textarea.focus();
      textarea.selectionStart = start + prefix.length;
      textarea.selectionEnd = start + prefix.length + selected.length;
    });
  };

  const prefixLines = (prefix: string) => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const value = textarea.value;
    // Find start of first line
    const lineStart = value.lastIndexOf("\n", start - 1) + 1;
    // Find end of last line
    const lineEnd = value.indexOf("\n", end);
    const blockEnd = lineEnd === -1 ? value.length : lineEnd;
    const block = value.substring(lineStart, blockEnd);
    const prefixed = block
      .split("\n")
      .map((line) => (line.startsWith(prefix) ? line.slice(prefix.length) : prefix + line))
      .join("\n");
    const next = value.substring(0, lineStart) + prefixed + value.substring(blockEnd);
    onUpdate(next);
    requestAnimationFrame(() => {
      textarea.focus();
    });
  };

  const cycleHeading = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const start = textarea.selectionStart;
    const value = textarea.value;
    const lineStart = value.lastIndexOf("\n", start - 1) + 1;
    const lineEnd = value.indexOf("\n", start);
    const blockEnd = lineEnd === -1 ? value.length : lineEnd;
    const line = value.substring(lineStart, blockEnd);
    let next: string;
    if (line.startsWith("### ")) {
      next = line.slice(4);
    } else if (line.startsWith("## ")) {
      next = "### " + line.slice(3);
    } else {
      next = "## " + line.replace(/^#+\s*/, "");
    }
    const updated = value.substring(0, lineStart) + next + value.substring(blockEnd);
    onUpdate(updated);
    requestAnimationFrame(() => {
      textarea.focus();
    });
  };

  return (
    <div className="text-toolbar" role="toolbar" aria-label="Text formatting">
      <button
        className="text-toolbar-btn"
        title="Bold (Ctrl+B)"
        aria-label="Bold"
        onMouseDown={(e) => { e.preventDefault(); wrapSelection("**", "**"); }}
      >
        <strong>B</strong>
      </button>
      <button
        className="text-toolbar-btn"
        title="Italic (Ctrl+I)"
        aria-label="Italic"
        onMouseDown={(e) => { e.preventDefault(); wrapSelection("*", "*"); }}
      >
        <em>I</em>
      </button>
      <div className="text-toolbar-sep" aria-hidden="true" />
      <button
        className="text-toolbar-btn"
        title="Heading (cycles H2 / H3 / plain)"
        aria-label="Heading"
        onMouseDown={(e) => { e.preventDefault(); cycleHeading(); }}
      >
        H
      </button>
      <div className="text-toolbar-sep" aria-hidden="true" />
      <button
        className="text-toolbar-btn"
        title="Bullet list"
        aria-label="Bullet list"
        onMouseDown={(e) => { e.preventDefault(); prefixLines("- "); }}
      >
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
          <circle cx="2" cy="4" r="1.2" fill="currentColor" />
          <circle cx="2" cy="7" r="1.2" fill="currentColor" />
          <circle cx="2" cy="10" r="1.2" fill="currentColor" />
          <line x1="5" y1="4" x2="13" y2="4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <line x1="5" y1="7" x2="13" y2="7" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <line x1="5" y1="10" x2="13" y2="10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      </button>
      <button
        className="text-toolbar-btn"
        title="Numbered list"
        aria-label="Numbered list"
        onMouseDown={(e) => { e.preventDefault(); prefixLines("1. "); }}
      >
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true">
          <text x="0" y="5.5" fontSize="5" fill="currentColor" fontFamily="monospace">1.</text>
          <text x="0" y="9" fontSize="5" fill="currentColor" fontFamily="monospace">2.</text>
          <text x="0" y="12.5" fontSize="5" fill="currentColor" fontFamily="monospace">3.</text>
          <line x1="5" y1="4" x2="13" y2="4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <line x1="5" y1="7.5" x2="13" y2="7.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          <line x1="5" y1="11" x2="13" y2="11" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}

// ─── Summary tab ─────────────────────────────────────────────────────────────

function SummaryTab({
  detail,
  onSummarize,
  isSummarizing,
  templates,
  selectedTemplateId,
  onSelectTemplate,
  onCopy,
  onUpdateSummary,
}: {
  detail: MeetingDetail;
  onSummarize: (templateId?: string) => Promise<void>;
  isSummarizing: boolean;
  templates: SummaryTemplate[];
  selectedTemplateId: string | null;
  onSelectTemplate: (templateId: string) => void;
  onCopy?: () => Promise<void>;
  onUpdateSummary?: (summary: string) => Promise<void>;
}) {
  const [editMode, setEditMode] = useState(false);
  const [editValue, setEditValue] = useState(detail.summaryText ?? "");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Keep editValue in sync when the detail changes (e.g. after regenerate)
  useEffect(() => {
    if (!editMode) {
      setEditValue(detail.summaryText ?? "");
      setDirty(false);
    }
  }, [detail.summaryText, editMode]);

  // Cleanup debounce on unmount
  useEffect(() => {
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, []);

  const handleTextChange = (value: string) => {
    setEditValue(value);
    setDirty(true);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      void save(value);
    }, 1000);
  };

  const save = async (value: string) => {
    if (!onUpdateSummary) return;
    setSaving(true);
    try {
      await onUpdateSummary(value);
      setDirty(false);
    } finally {
      setSaving(false);
    }
  };

  const handleBlur = () => {
    if (!dirty || !onUpdateSummary) return;
    if (debounceRef.current) clearTimeout(debounceRef.current);
    void save(editValue);
  };

  const handleExitEdit = async () => {
    if (dirty && onUpdateSummary) {
      if (debounceRef.current) clearTimeout(debounceRef.current);
      await save(editValue);
    }
    setEditMode(false);
  };

  const activeTemplate = selectedTemplateId
    ? templates.find((t) => t.id === selectedTemplateId) ?? null
    : templates.find((t) => t.isDefault) ?? null;

  if (detail.summaryText) {
    return (
      <div className="summary-content">
        <div className="summary-regenerate-bar">
          {!editMode && templates.length > 1 ? (
            <select
              className="template-select"
              value={activeTemplate?.id ?? ""}
              onChange={(e) => onSelectTemplate(e.target.value)}
              aria-label="Summary template"
            >
              {templates.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}{t.isDefault ? " (default)" : ""}
                </option>
              ))}
            </select>
          ) : null}
          {!editMode ? (
            <button
              className="secondary-button"
              disabled={isSummarizing}
              onClick={() => void onSummarize(activeTemplate?.id)}
            >
              {isSummarizing ? "Regenerating..." : "Regenerate"}
            </button>
          ) : null}
          {onUpdateSummary ? (
            <button
              className="secondary-button"
              onClick={() => {
                if (editMode) {
                  void handleExitEdit();
                } else {
                  setEditValue(detail.summaryText ?? "");
                  setDirty(false);
                  setEditMode(true);
                }
              }}
            >
              {editMode ? "Done" : "Edit"}
            </button>
          ) : null}
          {editMode && dirty ? (
            <span className="summary-unsaved-indicator" aria-live="polite">
              {saving ? "Saving..." : "Unsaved changes"}
            </span>
          ) : null}
          {!editMode && onCopy ? <CopyButton onCopy={onCopy} /> : null}
        </div>

        {editMode ? (
          <div className="summary-edit-area">
            <TextToolbar
              textareaRef={textareaRef}
              onUpdate={(value) => {
                setEditValue(value);
                setDirty(true);
                if (debounceRef.current) clearTimeout(debounceRef.current);
                debounceRef.current = setTimeout(() => {
                  void save(value);
                }, 1000);
              }}
            />
            <textarea
              ref={textareaRef}
              className="summary-edit-textarea"
              value={editValue}
              onChange={(e) => handleTextChange(e.target.value)}
              onBlur={handleBlur}
              spellCheck
              aria-label="Edit summary markdown"
            />
          </div>
        ) : (
          <div className="md-body">{renderMarkdown(detail.summaryText)}</div>
        )}
      </div>
    );
  }

  return (
    <div className="tab-empty-state">
      <strong>No summary yet</strong>
      <p>Generate an AI summary of this meeting transcript.</p>
      {detail.transcriptSegments.length === 0 ? (
        <p className="tab-empty-hint">Transcript required before summarizing.</p>
      ) : (
        <div className="summary-generate-row">
          {templates.length > 1 ? (
            <select
              className="template-select"
              value={activeTemplate?.id ?? ""}
              onChange={(e) => onSelectTemplate(e.target.value)}
              aria-label="Summary template"
            >
              {templates.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}{t.isDefault ? " (default)" : ""}
                </option>
              ))}
            </select>
          ) : null}
          <button
            className="primary-button"
            disabled={isSummarizing}
            onClick={() => void onSummarize(activeTemplate?.id)}
          >
            {isSummarizing ? "Generating..." : "Generate summary"}
          </button>
        </div>
      )}
    </div>
  );
}

// ─── Tag color palette ───────────────────────────────────────────────────────

const TAG_COLORS = [
  { name: "Gray", value: "#6B7280" },
  { name: "Red", value: "#EF4444" },
  { name: "Orange", value: "#F97316" },
  { name: "Yellow", value: "#EAB308" },
  { name: "Green", value: "#22C55E" },
  { name: "Blue", value: "#3B82F6" },
  { name: "Purple", value: "#8B5CF6" },
  { name: "Pink", value: "#EC4899" },
];

// ─── Speaker color palette ───────────────────────────────────────────────────

const SPEAKER_COLORS = [
  "#4A9EFF",
  "#FF6B6B",
  "#51CF66",
  "#FFD43B",
  "#CC5DE8",
  "#FF922B",
];

function getSpeakerColorIndex(label: string, allLabels: string[]): number {
  const index = allLabels.indexOf(label);
  return index >= 0 ? index % SPEAKER_COLORS.length : 0;
}

// ─── Transcript tab ─────────────────────────────────────────────────────────

function TranscriptTab({
  detail,
  onSeekPlayback,
  onCopy,
  onUpdateSegment,
}: {
  detail: MeetingDetail;
  onSeekPlayback: (seconds: number) => Promise<void>;
  onCopy?: () => Promise<void>;
  onUpdateSegment?: (segmentId: string, text: string) => Promise<void>;
}) {
  const segments = detail.transcriptSegments;
  const [editingSegmentId, setEditingSegmentId] = useState<string | null>(null);
  const [editSegmentText, setEditSegmentText] = useState("");

  // Build a map of speakerLabel -> speakerName from meeting speakers
  const speakerNameMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const ms of (detail.speakers ?? [])) {
      if (ms.speakerName) {
        map.set(ms.speakerLabel, ms.speakerName);
      }
    }
    return map;
  }, [detail.speakers]);

  // Collect ordered unique labels for color assignment
  const uniqueLabels = useMemo(() => {
    const seen = new Set<string>();
    const labels: string[] = [];
    for (const seg of segments) {
      const label = seg.speaker ?? "Speaker";
      if (!seen.has(label)) {
        seen.add(label);
        labels.push(label);
      }
    }
    return labels;
  }, [segments]);

  const startEditSegment = (segmentId: string, text: string) => {
    setEditingSegmentId(segmentId);
    setEditSegmentText(text);
  };

  const commitSegmentEdit = async () => {
    if (!editingSegmentId || !onUpdateSegment) return;
    const trimmed = editSegmentText.trim();
    if (trimmed) {
      await onUpdateSegment(editingSegmentId, trimmed);
    }
    setEditingSegmentId(null);
  };

  const cancelSegmentEdit = () => {
    setEditingSegmentId(null);
  };

  if (segments.length === 0) {
    return (
      <div className="tab-empty-state">
        <strong>No transcript yet</strong>
        <p>
          {detail.status === "processing"
            ? "Transcription is still running."
            : "Install a transcription model in Settings to generate local transcripts."}
        </p>
      </div>
    );
  }

  return (
    <div className="transcript-tab-list">
      {onCopy ? (
        <div className="tab-copy-bar">
          <CopyButton onCopy={onCopy} />
        </div>
      ) : null}
      {segments.map((segment) => {
        const label = segment.speaker ?? "Speaker";
        const displayName = speakerNameMap.get(label) ?? label;
        const colorIndex = getSpeakerColorIndex(label, uniqueLabels);
        const color = SPEAKER_COLORS[colorIndex];
        const isEditing = editingSegmentId === segment.id;

        if (isEditing) {
          return (
            <div key={segment.id} className="segment-card segment-card--editing">
              <span>{formatDuration(Math.floor(segment.startTime))}</span>
              <div>
                <span
                  className="speaker-badge"
                  style={{ color }}
                  aria-label={`Speaker: ${displayName}`}
                >
                  {displayName}
                </span>
                <textarea
                  className="segment-edit-textarea"
                  value={editSegmentText}
                  autoFocus
                  onChange={(e) => setEditSegmentText(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      void commitSegmentEdit();
                    }
                    if (e.key === "Escape") {
                      cancelSegmentEdit();
                    }
                  }}
                  aria-label="Edit transcript segment"
                />
                <div className="segment-edit-actions">
                  <button className="ghost-button" onClick={() => void commitSegmentEdit()}>
                    Save
                  </button>
                  <button className="ghost-button" onClick={cancelSegmentEdit}>
                    Cancel
                  </button>
                </div>
              </div>
            </div>
          );
        }

        return (
          <div key={segment.id} className="segment-card-wrapper">
            <button
              className="segment-card"
              onClick={() => void onSeekPlayback(segment.startTime)}
            >
              <span>{formatDuration(Math.floor(segment.startTime))}</span>
              <div>
                <span
                  className="speaker-badge"
                  style={{ color }}
                  aria-label={`Speaker: ${displayName}`}
                >
                  {displayName}
                </span>
                <p>{segment.text}</p>
              </div>
            </button>
            {onUpdateSegment ? (
              <button
                className="segment-edit-btn"
                title="Edit segment"
                aria-label={`Edit segment at ${formatDuration(Math.floor(segment.startTime))}`}
                onClick={() => startEditSegment(segment.id, segment.text)}
              >
                <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                  <path d="M8.5 1.5a1.207 1.207 0 0 1 1.707 1.707L3.5 9.914 1 10.5l.586-2.5L8.5 1.5Z" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
              </button>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

// ─── Tasks tab ──────────────────────────────────────────────────────────────

function TasksTab({
  tasks,
  onToggleTask,
  onDeleteTask,
  onUpdateTask,
  onCreateTask,
  onCopy,
}: {
  tasks: Task[];
  onToggleTask: (taskId: string) => Promise<void>;
  onDeleteTask: (taskId: string) => Promise<void>;
  onUpdateTask: (taskId: string, text: string, assignee: string | null, completed: boolean) => Promise<void>;
  onCreateTask: (text: string) => Promise<void>;
  onCopy?: () => Promise<void>;
}) {
  const [newTaskText, setNewTaskText] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editText, setEditText] = useState("");
  const [editAssignee, setEditAssignee] = useState<string>("");
  const newTaskRef = useRef<HTMLInputElement>(null);

  const startEdit = (task: Task) => {
    setEditingId(task.id);
    setEditText(task.text);
    setEditAssignee(task.assignee ?? "");
  };

  const commitEdit = async (task: Task) => {
    const trimmedText = editText.trim();
    if (!trimmedText) return;
    await onUpdateTask(task.id, trimmedText, editAssignee.trim() || null, task.completed);
    setEditingId(null);
  };

  const handleNewTaskSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    const text = newTaskText.trim();
    if (!text) return;
    setNewTaskText("");
    await onCreateTask(text);
  };

  return (
    <div className="tasks-tab">
      {onCopy && tasks.length > 0 ? (
        <div className="tab-copy-bar">
          <CopyButton onCopy={onCopy} />
        </div>
      ) : null}
      {tasks.length === 0 ? (
        <p className="tasks-empty">No tasks yet. Add one below.</p>
      ) : (
        <ul className="task-list" aria-label="Tasks">
          {tasks.map((task) => (
            <li
              key={task.id}
              className={clsx("task-item", task.completed && "task-completed")}
            >
              <button
                className="task-checkbox"
                role="checkbox"
                aria-checked={task.completed}
                aria-label={task.completed ? "Mark incomplete" : "Mark complete"}
                onClick={() => void onToggleTask(task.id)}
              >
                {task.completed ? (
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M2 6L5 9L10 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                ) : null}
              </button>

              {editingId === task.id ? (
                <div className="task-edit-row">
                  <input
                    autoFocus
                    className="task-edit-input"
                    value={editText}
                    onChange={(e) => setEditText(e.target.value)}
                    onKeyDown={async (e) => {
                      if (e.key === "Enter") await commitEdit(task);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    aria-label="Edit task text"
                  />
                  <input
                    className="task-edit-assignee"
                    value={editAssignee}
                    onChange={(e) => setEditAssignee(e.target.value)}
                    onKeyDown={async (e) => {
                      if (e.key === "Enter") await commitEdit(task);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    placeholder="Assignee"
                    aria-label="Edit assignee"
                  />
                  <button
                    className="ghost-button"
                    onClick={() => void commitEdit(task)}
                  >
                    Save
                  </button>
                  <button className="ghost-button" onClick={() => setEditingId(null)}>
                    Cancel
                  </button>
                </div>
              ) : (
                <>
                  <button
                    className="task-text-button"
                    onClick={() => startEdit(task)}
                    aria-label={`Edit: ${task.text}`}
                  >
                    {task.text}
                  </button>
                  {task.assignee ? (
                    <button
                      className="task-assignee-badge"
                      onClick={() => startEdit(task)}
                      aria-label={`Assignee: ${task.assignee}. Click to edit.`}
                    >
                      {task.assignee}
                    </button>
                  ) : null}
                  <button
                    className="task-delete"
                    aria-label={`Delete task: ${task.text}`}
                    onClick={() => void onDeleteTask(task.id)}
                  >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                      <path d="M2 2L10 10M10 2L2 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                    </svg>
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}

      <form className="task-add-form" onSubmit={(e) => void handleNewTaskSubmit(e)}>
        <input
          ref={newTaskRef}
          className="task-add-input"
          value={newTaskText}
          onChange={(e) => setNewTaskText(e.target.value)}
          placeholder="Add a task..."
          aria-label="New task text"
        />
        <button
          type="submit"
          className="secondary-button"
          disabled={!newTaskText.trim()}
        >
          Add
        </button>
      </form>
    </div>
  );
}

// ─── Scratchpad tab ─────────────────────────────────────────────────────────

function ScratchpadTab({
  detail,
  onUpdateScratchpad,
}: {
  detail: MeetingDetail;
  onUpdateScratchpad: (content: string) => Promise<void>;
}) {
  const [value, setValue] = useState(detail.scratchpad ?? "");
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setValue(detail.scratchpad ?? "");
  }, [detail.id, detail.scratchpad]);

  const handleChange = (text: string) => {
    setValue(text);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      void onUpdateScratchpad(text);
    }, 1000);
  };

  const handleBlur = () => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    void onUpdateScratchpad(value);
  };

  useEffect(() => {
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current);
    };
  }, []);

  return (
    <div className="scratchpad-tab">
      <p className="scratchpad-note">Private - not included in shared summaries.</p>
      <textarea
        className="scratchpad-textarea"
        value={value}
        onChange={(e) => handleChange(e.target.value)}
        onBlur={handleBlur}
        placeholder="Notes, ideas, follow-ups..."
        aria-label="Meeting scratchpad"
      />
    </div>
  );
}

// ─── Speaker identification panel ───────────────────────────────────────────

function SpeakerIdentificationPanel({
  meetingId,
  meetingSpeakers,
  knownSpeakers,
  onApply,
  onDismiss,
}: {
  meetingId: string;
  meetingSpeakers: MeetingSpeaker[];
  knownSpeakers: Speaker[];
  onApply: (assignments: Array<{ speakerLabel: string; speakerId: string }>) => Promise<void>;
  onDismiss: () => void;
}) {
  const [assignments, setAssignments] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    for (const ms of meetingSpeakers) {
      if (ms.speakerId) init[ms.speakerLabel] = ms.speakerId;
    }
    return init;
  });
  const [customNames, setCustomNames] = useState<Record<string, string>>(() => {
    const init: Record<string, string> = {};
    meetingSpeakers.forEach((ms, i) => {
      init[ms.speakerLabel] = ms.speakerName ?? `Speaker ${i + 1}`;
    });
    return init;
  });
  const [applying, setApplying] = useState(false);
  const [newSpeakerLabels, setNewSpeakerLabels] = useState<Set<string>>(new Set());
  const audioRefs = useRef<Record<string, HTMLAudioElement | null>>({});

  const uniqueLabels = useMemo(() => {
    return meetingSpeakers.map((ms) => ms.speakerLabel);
  }, [meetingSpeakers]);

  const colorForLabel = (label: string) => {
    const idx = uniqueLabels.indexOf(label) % SPEAKER_COLORS.length;
    return SPEAKER_COLORS[idx >= 0 ? idx : 0];
  };

  const handleSelectChange = (speakerLabel: string, value: string) => {
    if (value === "__new__") {
      setNewSpeakerLabels((prev) => new Set(prev).add(speakerLabel));
      setAssignments((prev) => {
        const next = { ...prev };
        delete next[speakerLabel];
        return next;
      });
    } else if (value === "") {
      setAssignments((prev) => {
        const next = { ...prev };
        delete next[speakerLabel];
        return next;
      });
      setNewSpeakerLabels((prev) => {
        const next = new Set(prev);
        next.delete(speakerLabel);
        return next;
      });
    } else {
      setAssignments((prev) => ({ ...prev, [speakerLabel]: value }));
      setNewSpeakerLabels((prev) => {
        const next = new Set(prev);
        next.delete(speakerLabel);
        return next;
      });
      const found = knownSpeakers.find((s) => s.id === value);
      if (found) {
        setCustomNames((prev) => ({ ...prev, [speakerLabel]: found.name }));
      }
    }
  };

  const handleApply = async () => {
    setApplying(true);
    try {
      const results: Array<{ speakerLabel: string; speakerId: string }> = [];
      for (const label of uniqueLabels) {
        if (newSpeakerLabels.has(label)) {
          const name = customNames[label]?.trim();
          if (name) {
            const created = await commands.createSpeaker(name);
            results.push({ speakerLabel: label, speakerId: created.id });
          }
        } else if (assignments[label]) {
          results.push({ speakerLabel: label, speakerId: assignments[label] });
        }
      }
      await onApply(results);
    } finally {
      setApplying(false);
    }
  };

  return (
    <div className="speaker-id-panel">
      <div className="speaker-id-header">
        <strong>Identify speakers</strong>
        <span className="speaker-id-hint">Match detected speakers to known contacts</span>
      </div>
      <div className="speaker-id-cards">
        {meetingSpeakers.map((ms, i) => {
          const color = colorForLabel(ms.speakerLabel);
          const isNew = newSpeakerLabels.has(ms.speakerLabel);
          const clipSrc = ms.clipPath ? audioSourceForPath(ms.clipPath) : null;
          return (
            <div key={ms.speakerLabel} className="speaker-id-card">
              <div className="speaker-id-card-header">
                <span className="speaker-label-badge" style={{ color }}>
                  {ms.speakerLabel}
                </span>
                {clipSrc ? (
                  <button
                    className="speaker-play-btn"
                    aria-label={`Play clip for ${ms.speakerLabel}`}
                    onClick={() => {
                      const audio = audioRefs.current[ms.speakerLabel];
                      if (audio) {
                        if (audio.paused) {
                          void audio.play();
                        } else {
                          audio.pause();
                          audio.currentTime = 0;
                        }
                      }
                    }}
                  >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                      <path d="M3 2L10 6L3 10V2Z" fill="currentColor" />
                    </svg>
                    <audio
                      ref={(el) => { audioRefs.current[ms.speakerLabel] = el; }}
                      src={clipSrc}
                      preload="none"
                      style={{ display: "none" }}
                    />
                  </button>
                ) : null}
              </div>
              <input
                className="speaker-name-input"
                value={customNames[ms.speakerLabel] ?? `Speaker ${i + 1}`}
                onChange={(e) =>
                  setCustomNames((prev) => ({ ...prev, [ms.speakerLabel]: e.target.value }))
                }
                placeholder="Enter name..."
                aria-label={`Name for ${ms.speakerLabel}`}
              />
              <select
                className="speaker-contact-select"
                value={isNew ? "__new__" : (assignments[ms.speakerLabel] ?? "")}
                onChange={(e) => handleSelectChange(ms.speakerLabel, e.target.value)}
                aria-label={`Contact for ${ms.speakerLabel}`}
              >
                <option value="">No contact</option>
                {knownSpeakers.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
                <option value="__new__">Create new contact</option>
              </select>
            </div>
          );
        })}
      </div>
      <div className="speaker-id-actions">
        <button
          className="primary-button"
          disabled={applying}
          onClick={() => void handleApply()}
        >
          {applying ? "Applying..." : "Apply"}
        </button>
        <button className="ghost-button" onClick={onDismiss}>
          Dismiss
        </button>
      </div>
    </div>
  );
}

// ─── Audio player bar ───────────────────────────────────────────────────────

function AudioPlayerBar({
  audioUrl,
  playback,
  onSeekPlayback,
}: {
  audioUrl: string | null;
  playback: PlaybackState | null;
  onSeekPlayback: (seconds: number) => Promise<void>;
}) {
  const mediaRef = useRef<HTMLMediaElement | null>(null);

  useEffect(() => {
    if (!mediaRef.current || !playback) return;
    if (Math.abs(mediaRef.current.currentTime - playback.positionSeconds) > 0.75) {
      mediaRef.current.currentTime = playback.positionSeconds;
    }
    if (playback.isPlaying) {
      void mediaRef.current.play().catch(() => undefined);
    } else {
      mediaRef.current.pause();
    }
  }, [playback]);

  if (!audioUrl) return null;

  return (
    <div className="audio-player-bar">
      <div className="audio-player-inner">
        <audio
          key={audioUrl}
          ref={(node) => { mediaRef.current = node; }}
          controls
          preload="metadata"
          src={audioUrl}
          onPlay={() => void commands.playPlayback()}
          onPause={() => void commands.pausePlayback()}
          onSeeked={(event) =>
            void onSeekPlayback((event.target as HTMLAudioElement).currentTime)
          }
        />
        {playback ? (
          <span className="audio-player-time">
            {formatDuration(Math.floor(playback.positionSeconds))} / {formatDuration(Math.floor(playback.durationSeconds))}
          </span>
        ) : null}
      </div>
    </div>
  );
}

// ─── Meeting detail panel ───────────────────────────────────────────────────

function MeetingDetailPanel({
  detail,
  tasks,
  playback,
  audioUrl,
  isSummarizing,
  templates,
  knownSpeakers,
  allTags,
  onRenameMeeting,
  onDeleteMeeting,
  onDeleteTranscript,
  onExportMeeting,
  onLoadPlayback,
  onSeekPlayback,
  onSummarize,
  onToggleTask,
  onDeleteTask,
  onUpdateTask,
  onCreateTask,
  onUpdateScratchpad,
  onAssignSpeakers,
  onAddTagToMeeting,
  onRemoveTagFromMeeting,
}: {
  detail: MeetingDetail;
  tasks: Task[];
  playback: PlaybackState | null;
  audioUrl: string | null;
  isSummarizing: boolean;
  templates: SummaryTemplate[];
  knownSpeakers: Speaker[];
  allTags: Tag[];
  onRenameMeeting: (meetingId: string, title: string) => Promise<void>;
  onDeleteMeeting: (meetingId: string) => Promise<void>;
  onDeleteTranscript: (meetingId: string) => Promise<void>;
  onExportMeeting: (meetingId: string, format: (typeof exportFormats)[number]) => Promise<void>;
  onLoadPlayback: (meetingId: string) => Promise<void>;
  onSeekPlayback: (seconds: number) => Promise<void>;
  onSummarize: (meetingId: string, templateId?: string) => Promise<void>;
  onToggleTask: (taskId: string) => Promise<void>;
  onDeleteTask: (taskId: string) => Promise<void>;
  onUpdateTask: (taskId: string, text: string, assignee: string | null, completed: boolean) => Promise<void>;
  onCreateTask: (meetingId: string, text: string) => Promise<void>;
  onUpdateScratchpad: (meetingId: string, content: string) => Promise<void>;
  onAssignSpeakers: (meetingId: string, assignments: Array<{ speakerLabel: string; speakerId: string }>) => Promise<void>;
  onAddTagToMeeting: (meetingId: string, tagId: string) => Promise<void>;
  onRemoveTagFromMeeting: (meetingId: string, tagId: string) => Promise<void>;
}) {
  const [activeTab, setActiveTab] = useState<DetailTab>("summary");
  const [draftTitle, setDraftTitle] = useState(detail.title);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmDeleteTranscript, setConfirmDeleteTranscript] = useState(false);
  const [showActions, setShowActions] = useState(false);
  const [speakerPanelDismissed, setSpeakerPanelDismissed] = useState(false);
  const [showTagDropdown, setShowTagDropdown] = useState(false);
  const tagDropdownRef = useRef<HTMLDivElement>(null);

  // Close tag dropdown on outside click
  useEffect(() => {
    if (!showTagDropdown) return;
    const handler = (e: MouseEvent) => {
      if (tagDropdownRef.current && !tagDropdownRef.current.contains(e.target as Node)) {
        setShowTagDropdown(false);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [showTagDropdown]);
  const [selectedTemplateId, setSelectedTemplateId] = useState<string | null>(null);

  useEffect(() => {
    setDraftTitle(detail.title);
    setConfirmDelete(false);
    setConfirmDeleteTranscript(false);
    setSpeakerPanelDismissed(false);
    setSelectedTemplateId(null);
  }, [detail.id, detail.title]);

  const incompleteTasks = tasks.filter((t) => !t.completed);
  const isProcessing = detail.status === "processing" || detail.status === "recording";

  // Show speaker panel when: completed, has multiple speakers, not all identified, not dismissed
  const hasSpeakers = (detail.speakers ?? []).length > 1;
  const allIdentified = (detail.speakers ?? []).every((ms) => ms.speakerId !== null);
  const showSpeakerPanel =
    detail.status === "completed" &&
    hasSpeakers &&
    !allIdentified &&
    !speakerPanelDismissed;

  return (
    <div className="detail-panel">
      {/* Detail header */}
      <div className="detail-header">
        <div className="detail-header-top">
          <div className="detail-title-row">
            <input
              className="detail-title-input"
              aria-label="Meeting title"
              autoComplete="off"
              name="meeting-title"
              value={draftTitle}
              onChange={(e) => setDraftTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void onRenameMeeting(detail.id, draftTitle);
              }}
              onBlur={() => {
                if (draftTitle !== detail.title) {
                  void onRenameMeeting(detail.id, draftTitle);
                }
              }}
            />
            <span className={clsx("status-pill", detail.status)}>{detail.status}</span>
          </div>
          <div className="detail-meta-row">
            <span>{formatClock(detail.startedAt)}</span>
            <span>{formatDuration(detail.durationSeconds)}</span>
            <span>{detail.transcriptSegments.length} segments</span>
          </div>
          {/* Tags row */}
          <div className="detail-tags-row">
            {(detail.tags ?? []).map((mt) => (
              <button
                key={mt.tagId}
                className="tag-pill tag-pill--removable"
                style={{ "--tag-color": mt.tagColor } as React.CSSProperties}
                onClick={() => void onRemoveTagFromMeeting(detail.id, mt.tagId)}
                aria-label={`Remove tag: ${mt.tagName}`}
                title="Click to remove"
              >
                <span className="tag-pill-dot" />
                {mt.tagName}
              </button>
            ))}
            <div className="tag-add-wrapper" ref={tagDropdownRef}>
              <button
                className="tag-add-btn"
                onClick={() => setShowTagDropdown((v) => !v)}
                aria-label="Add tag"
                aria-expanded={showTagDropdown}
              >
                +
              </button>
              {showTagDropdown ? (
                <div className="tag-dropdown" role="menu">
                  {allTags.length === 0 ? (
                    <span className="tag-dropdown-empty">No tags. Create some in Settings.</span>
                  ) : (
                    allTags.map((tag) => {
                      const assigned = (detail.tags ?? []).some((mt) => mt.tagId === tag.id);
                      return (
                        <button
                          key={tag.id}
                          className={clsx("tag-dropdown-item", assigned && "tag-dropdown-item--assigned")}
                          role="menuitem"
                          onClick={() => {
                            setShowTagDropdown(false);
                            if (assigned) {
                              void onRemoveTagFromMeeting(detail.id, tag.id);
                            } else {
                              void onAddTagToMeeting(detail.id, tag.id);
                            }
                          }}
                        >
                          <span className="tag-pill-dot" style={{ backgroundColor: tag.color }} />
                          <span>{tag.name}</span>
                          {assigned ? (
                            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true" style={{ marginLeft: "auto" }}>
                              <path d="M2 6L5 9L10 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                            </svg>
                          ) : null}
                        </button>
                      );
                    })
                  )}
                </div>
              ) : null}
            </div>
          </div>
        </div>

        <div className="detail-header-actions">
          {/* Audio control */}
          {audioUrl ? null : (
            <button
              className="secondary-button"
              onClick={() => void onLoadPlayback(detail.id)}
            >
              Load audio
            </button>
          )}

          {/* Overflow actions */}
          <div className="detail-actions-menu">
            <button
              className="secondary-button detail-actions-trigger"
              onClick={() => setShowActions((prev) => !prev)}
              aria-expanded={showActions}
              aria-haspopup="true"
            >
              Actions
              <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true" style={{ marginLeft: 4 }}>
                <path d="M3 5L6 8L9 5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
              </svg>
            </button>
            {showActions ? (
              <div className="detail-actions-dropdown" role="menu">
                {exportFormats.map((format) => (
                  <button
                    key={format}
                    className="detail-actions-item"
                    role="menuitem"
                    onClick={() => {
                      setShowActions(false);
                      void onExportMeeting(detail.id, format);
                    }}
                  >
                    Export {format.toUpperCase()}
                    {format === "html" ? (
                      <span className="detail-actions-item-hint"> (Google Docs)</span>
                    ) : null}
                  </button>
                ))}
                {detail.transcriptSegments.length > 0 ? (
                  confirmDeleteTranscript ? (
                    <>
                      <button
                        className="detail-actions-item"
                        role="menuitem"
                        onClick={() => { setConfirmDeleteTranscript(false); setShowActions(false); }}
                      >
                        Cancel
                      </button>
                      <button
                        className="detail-actions-item danger"
                        role="menuitem"
                        onClick={() => { setShowActions(false); void onDeleteTranscript(detail.id); }}
                      >
                        Confirm delete transcript
                      </button>
                    </>
                  ) : (
                    <button
                      className="detail-actions-item danger"
                      role="menuitem"
                      onClick={() => setConfirmDeleteTranscript(true)}
                    >
                      Delete transcript
                    </button>
                  )
                ) : null}
                {confirmDelete ? (
                  <>
                    <button
                      className="detail-actions-item"
                      role="menuitem"
                      onClick={() => { setConfirmDelete(false); setShowActions(false); }}
                    >
                      Cancel
                    </button>
                    <button
                      className="detail-actions-item danger"
                      role="menuitem"
                      onClick={() => { setShowActions(false); void onDeleteMeeting(detail.id); }}
                    >
                      Confirm delete meeting
                    </button>
                  </>
                ) : (
                  <button
                    className="detail-actions-item danger"
                    role="menuitem"
                    onClick={() => setConfirmDelete(true)}
                  >
                    Delete meeting
                  </button>
                )}
              </div>
            ) : null}
          </div>
        </div>
      </div>

      {/* Processing state - replace tabs */}
      {isProcessing ? (
        <ProcessingProgress detail={detail} />
      ) : (
        <>
          {/* Speaker identification panel */}
          {showSpeakerPanel ? (
            <SpeakerIdentificationPanel
              meetingId={detail.id}
              meetingSpeakers={detail.speakers ?? []}
              knownSpeakers={knownSpeakers}
              onApply={async (assignments) => {
                await onAssignSpeakers(detail.id, assignments);
                setSpeakerPanelDismissed(true);
              }}
              onDismiss={() => setSpeakerPanelDismissed(true)}
            />
          ) : null}

          {/* Tab bar */}
          <div className="detail-tabs" role="tablist" aria-label="Meeting sections">
            {(["summary", "transcript", "tasks", "scratchpad"] as DetailTab[]).map((tab) => (
              <button
                key={tab}
                role="tab"
                className={clsx("detail-tab", activeTab === tab && "active")}
                aria-selected={activeTab === tab}
                onClick={() => setActiveTab(tab)}
              >
                {tab === "tasks" ? (
                  <>
                    Tasks
                    {incompleteTasks.length > 0 ? (
                      <span className="tab-badge">{incompleteTasks.length}</span>
                    ) : null}
                  </>
                ) : (
                  tab.charAt(0).toUpperCase() + tab.slice(1)
                )}
              </button>
            ))}
          </div>

          {/* Tab panels */}
          <div className="detail-tab-content" role="tabpanel">
            {activeTab === "summary" ? (
              <SummaryTab
                detail={detail}
                onSummarize={(templateId) => onSummarize(detail.id, templateId)}
                isSummarizing={isSummarizing}
                templates={templates}
                selectedTemplateId={selectedTemplateId}
                onSelectTemplate={setSelectedTemplateId}
                onCopy={detail.summaryText ? async () => {
                  const content = await commands.copySummary(detail.id);
                  await copyToClipboard(content);
                } : undefined}
                onUpdateSummary={async (summary) => {
                  await commands.updateSummary(detail.id, summary);
                }}
              />
            ) : activeTab === "transcript" ? (
              <TranscriptTab
                detail={detail}
                onSeekPlayback={onSeekPlayback}
                onCopy={detail.transcriptSegments.length > 0 ? async () => {
                  const content = await commands.copyTranscript(detail.id);
                  await copyToClipboard(content);
                } : undefined}
                onUpdateSegment={async (segmentId, text) => {
                  await commands.updateTranscriptSegment(segmentId, text);
                }}
              />
            ) : activeTab === "tasks" ? (
              <TasksTab
                tasks={tasks}
                onToggleTask={onToggleTask}
                onDeleteTask={onDeleteTask}
                onUpdateTask={onUpdateTask}
                onCreateTask={(text) => onCreateTask(detail.id, text)}
                onCopy={tasks.length > 0 ? async () => {
                  const content = await commands.copyTasks(detail.id);
                  await copyToClipboard(content);
                } : undefined}
              />
            ) : (
              <ScratchpadTab
                detail={detail}
                onUpdateScratchpad={(content) => onUpdateScratchpad(detail.id, content)}
              />
            )}
          </div>
        </>
      )}

      {/* Audio player - sticky at bottom of detail panel */}
      <AudioPlayerBar
        audioUrl={audioUrl}
        playback={playback}
        onSeekPlayback={onSeekPlayback}
      />
    </div>
  );
}

// ─── Meeting list sidebar ───────────────────────────────────────────────────

function MeetingListSidebar({
  meetings,
  activeMeetingId,
  search,
  results,
  allTags,
  selectedTagIds,
  onSelectMeeting,
  onSearch,
  onToggleTagFilter,
  onBatchDelete,
}: {
  meetings: MeetingSummary[];
  activeMeetingId: string | null;
  search: string;
  results: SearchResult[];
  allTags: Tag[];
  selectedTagIds: string[];
  onSelectMeeting: (meetingId: string) => Promise<void>;
  onSearch: (value: string) => Promise<void>;
  onToggleTagFilter: (tagId: string) => void;
  onBatchDelete: (meetingIds: string[]) => Promise<void>;
}) {
  const PAGE_SIZE = 20;
  const [page, setPage] = useState(0);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [confirmBatchDelete, setConfirmBatchDelete] = useState(false);
  const [batchDeleteBusy, setBatchDeleteBusy] = useState(false);
  const totalPages = Math.max(1, Math.ceil(meetings.length / PAGE_SIZE));
  const paginated = meetings.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

  useEffect(() => {
    setPage(0);
  }, [search]);

  // Clear selection when meetings list changes (after delete, refresh)
  useEffect(() => {
    setSelectedIds(new Set());
    setConfirmBatchDelete(false);
    setBatchDeleteBusy(false);
  }, [meetings]);

  // Cmd+ArrowDown/Up: extend selection
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!e.metaKey || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
      if (paginated.length === 0) return;
      e.preventDefault();

      setSelectedIds((prev) => {
        const next = new Set(prev);
        if (e.key === "ArrowDown") {
          const lastIdx = [...next].reduce((max, id) => {
            const idx = paginated.findIndex((m) => m.id === id);
            return idx > max ? idx : max;
          }, -1);
          const nextIdx = lastIdx === -1 ? 0 : lastIdx + 1;
          if (nextIdx < paginated.length) next.add(paginated[nextIdx].id);
        } else {
          const firstIdx = [...next].reduce((min, id) => {
            const idx = paginated.findIndex((m) => m.id === id);
            return idx < min ? idx : min;
          }, paginated.length);
          const prevIdx = firstIdx <= 0 ? 0 : firstIdx - 1;
          next.add(paginated[prevIdx].id);
        }
        return next;
      });
      setConfirmBatchDelete(false);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [paginated]);

  // Backspace/Delete key to trigger batch delete
  useEffect(() => {
    if (selectedIds.size === 0) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Backspace" || e.key === "Delete") {
        e.preventDefault();
        setConfirmBatchDelete(true);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [selectedIds.size]);

  const handleBatchDelete = async () => {
    if (batchDeleteBusy || selectedIds.size === 0) return;
    setBatchDeleteBusy(true);
    try {
      await onBatchDelete([...selectedIds]);
    } finally {
      setBatchDeleteBusy(false);
    }
  };

  return (
    <aside className="meeting-list-sidebar">
      <div className="meeting-list-search">
        <label className="search-box">
          <svg className="search-icon" width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <circle cx="7" cy="7" r="5.5" stroke="currentColor" strokeWidth="1.5" />
            <path d="M11 11L14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          <input
            aria-label="Search meetings"
            autoComplete="off"
            name="meeting-search"
            value={search}
            onChange={(event) => void onSearch(event.target.value)}
            placeholder="Search..."
          />
        </label>
      </div>
      {allTags.length > 0 ? (
        <div className="tag-filter-bar" aria-label="Filter by tag">
          {allTags.map((tag) => {
            const active = selectedTagIds.includes(tag.id);
            return (
              <button
                key={tag.id}
                className={clsx("tag-filter-pill", active && "tag-filter-pill--active")}
                style={{ "--tag-color": tag.color } as React.CSSProperties}
                onClick={() => onToggleTagFilter(tag.id)}
                aria-pressed={active}
              >
                <span className="tag-pill-dot" />
                {tag.name}
              </button>
            );
          })}
        </div>
      ) : null}

      {results.length > 0 ? (
        <div className="meeting-search-results">
          <span className="meeting-list-section-label">{results.length} results</span>
          {results.map((result) => (
            <button
              key={result.segmentId}
              className={clsx("meeting-list-item", activeMeetingId === result.meetingId && "active")}
              onClick={() => void onSelectMeeting(result.meetingId)}
            >
              <strong className="meeting-list-title">{result.meetingTitle}</strong>
              <p className="meeting-list-snippet">{result.snippet}</p>
            </button>
          ))}
        </div>
      ) : null}

      <div className="meeting-list-items">
        {results.length > 0 ? (
          <span className="meeting-list-section-label">All meetings</span>
        ) : null}
        {paginated.length > 0 ? (
          paginated.map((meeting) => (
            <button
              key={meeting.id}
              className={clsx(
                "meeting-list-item",
                activeMeetingId === meeting.id && "active",
                selectedIds.has(meeting.id) && "selected",
              )}
              onClick={(e) => {
                if (e.metaKey) {
                  setSelectedIds((prev) => {
                    const next = new Set(prev);
                    if (next.has(meeting.id)) next.delete(meeting.id);
                    else next.add(meeting.id);
                    return next;
                  });
                  setConfirmBatchDelete(false);
                } else {
                  setSelectedIds(new Set());
                  setConfirmBatchDelete(false);
                  void onSelectMeeting(meeting.id);
                }
              }}
              aria-current={activeMeetingId === meeting.id ? "true" : undefined}
            >
              <div className="meeting-list-item-header">
                <strong className="meeting-list-title">{meeting.title}</strong>
                <span className={clsx("status-pill", meeting.status)}>{meeting.status}</span>
              </div>
              <div className="meeting-list-item-meta">
                <span>{formatClock(meeting.startedAt)}</span>
                <span>{formatDuration(meeting.durationSeconds)}</span>
              </div>
              {(meeting.tags ?? []).length > 0 ? (
                <div className="meeting-list-item-tags">
                  {meeting.tags.map((mt) => (
                    <span
                      key={mt.tagId}
                      className="meeting-tag-dot"
                      style={{ backgroundColor: mt.tagColor }}
                      title={mt.tagName}
                      aria-label={mt.tagName}
                    />
                  ))}
                </div>
              ) : null}
            </button>
          ))
        ) : (
          <div className="meeting-list-empty">
            <CarlaLogo className="empty-state-logo" compact />
            <strong>No meetings yet</strong>
            <p>Start a recording to build your local archive.</p>
          </div>
        )}
      </div>

      {totalPages > 1 ? (
        <nav className="pagination" aria-label="Meeting pages">
          <button
            className="pagination-button"
            disabled={page === 0}
            onClick={() => setPage((p) => p - 1)}
          >
            Prev
          </button>
          {Array.from({ length: totalPages }, (_, i) => (
            <button
              key={i}
              className={clsx("pagination-button", page === i && "active")}
              onClick={() => setPage(i)}
            >
              {i + 1}
            </button>
          ))}
          <button
            className="pagination-button"
            disabled={page === totalPages - 1}
            onClick={() => setPage((p) => p + 1)}
          >
            Next
          </button>
        </nav>
      ) : null}

      {selectedIds.size > 0 ? (
        <div className="batch-action-bar">
          <span>{selectedIds.size} selected</span>
          {confirmBatchDelete ? (
            <>
              <button
                className="batch-action-btn batch-action-btn--cancel"
                onClick={() => setConfirmBatchDelete(false)}
              >
                Cancel
              </button>
              <button
                className="batch-action-btn batch-action-btn--delete"
                disabled={batchDeleteBusy}
                onClick={() => void handleBatchDelete()}
              >
                {batchDeleteBusy ? "Deleting..." : "Confirm"}
              </button>
            </>
          ) : (
            <button
              className="batch-action-btn batch-action-btn--delete"
              onClick={() => setConfirmBatchDelete(true)}
            >
              Delete
            </button>
          )}
        </div>
      ) : null}
    </aside>
  );
}

// ─── Meetings view ──────────────────────────────────────────────────────────

function MeetingsView({
  meetings,
  activeMeeting,
  tasks,
  search,
  results,
  isSummarizing,
  templates,
  knownSpeakers,
  allTags,
  selectedTagIds,
  onSelectMeeting,
  onSearch,
  onRenameMeeting,
  onDeleteMeeting,
  onDeleteTranscript,
  onExportMeeting,
  playback,
  audioUrl,
  onLoadPlayback,
  onSeekPlayback,
  onSummarize,
  onToggleTask,
  onDeleteTask,
  onUpdateTask,
  onCreateTask,
  onUpdateScratchpad,
  onAssignSpeakers,
  onAddTagToMeeting,
  onRemoveTagFromMeeting,
  onToggleTagFilter,
  onBatchDelete,
}: {
  meetings: MeetingSummary[];
  activeMeeting: MeetingDetail | null;
  tasks: Task[];
  search: string;
  results: SearchResult[];
  isSummarizing: boolean;
  templates: SummaryTemplate[];
  knownSpeakers: Speaker[];
  allTags: Tag[];
  selectedTagIds: string[];
  onSelectMeeting: (meetingId: string) => Promise<void>;
  onSearch: (value: string) => Promise<void>;
  onRenameMeeting: (meetingId: string, title: string) => Promise<void>;
  onDeleteMeeting: (meetingId: string) => Promise<void>;
  onDeleteTranscript: (meetingId: string) => Promise<void>;
  onExportMeeting: (meetingId: string, format: (typeof exportFormats)[number]) => Promise<void>;
  playback: PlaybackState | null;
  audioUrl: string | null;
  onLoadPlayback: (meetingId: string) => Promise<void>;
  onSeekPlayback: (seconds: number) => Promise<void>;
  onSummarize: (meetingId: string, templateId?: string) => Promise<void>;
  onToggleTask: (taskId: string) => Promise<void>;
  onDeleteTask: (taskId: string) => Promise<void>;
  onUpdateTask: (taskId: string, text: string, assignee: string | null, completed: boolean) => Promise<void>;
  onCreateTask: (meetingId: string, text: string) => Promise<void>;
  onUpdateScratchpad: (meetingId: string, content: string) => Promise<void>;
  onAssignSpeakers: (meetingId: string, assignments: Array<{ speakerLabel: string; speakerId: string }>) => Promise<void>;
  onAddTagToMeeting: (meetingId: string, tagId: string) => Promise<void>;
  onRemoveTagFromMeeting: (meetingId: string, tagId: string) => Promise<void>;
  onToggleTagFilter: (tagId: string) => void;
  onBatchDelete: (meetingIds: string[]) => Promise<void>;
}) {
  // Filter meetings by selected tags (AND logic)
  const filteredMeetings = useMemo(() => {
    if (selectedTagIds.length === 0) return meetings;
    return meetings.filter((m) =>
      selectedTagIds.every((tagId) => (m.tags ?? []).some((mt) => mt.tagId === tagId)),
    );
  }, [meetings, selectedTagIds]);

  return (
    <div className="meetings-layout">
      <MeetingListSidebar
        meetings={filteredMeetings}
        activeMeetingId={activeMeeting?.id ?? null}
        search={search}
        results={results}
        allTags={allTags}
        selectedTagIds={selectedTagIds}
        onSelectMeeting={onSelectMeeting}
        onSearch={onSearch}
        onToggleTagFilter={onToggleTagFilter}
        onBatchDelete={onBatchDelete}
      />
      <div className="meetings-detail-area">
        {activeMeeting ? (
          <MeetingDetailPanel
            detail={activeMeeting}
            tasks={tasks}
            playback={playback}
            audioUrl={audioUrl}
            isSummarizing={isSummarizing}
            templates={templates}
            knownSpeakers={knownSpeakers}
            allTags={allTags}
            onRenameMeeting={onRenameMeeting}
            onDeleteMeeting={onDeleteMeeting}
            onDeleteTranscript={onDeleteTranscript}
            onExportMeeting={onExportMeeting}
            onLoadPlayback={onLoadPlayback}
            onSeekPlayback={onSeekPlayback}
            onSummarize={onSummarize}
            onToggleTask={onToggleTask}
            onDeleteTask={onDeleteTask}
            onUpdateTask={onUpdateTask}
            onCreateTask={onCreateTask}
            onUpdateScratchpad={onUpdateScratchpad}
            onAssignSpeakers={onAssignSpeakers}
            onAddTagToMeeting={onAddTagToMeeting}
            onRemoveTagFromMeeting={onRemoveTagFromMeeting}
          />
        ) : (
          <div className="meetings-detail-empty">
            <CarlaLogo className="empty-state-logo" compact />
            <strong>Select a meeting</strong>
            <p>Choose a meeting from the list to view its details.</p>
          </div>
        )}
      </div>
    </div>
  );
}

// ─── Transcript view ────────────────────────────────────────────────────────

function TranscriptView({
  activeMeeting,
}: {
  activeMeeting: MeetingDetail | null;
}) {
  const transcript = activeMeeting?.transcriptSegments ?? [];
  const transcriptionJob = activeMeeting?.jobs.find((job) => job.kind === "transcription");
  return (
    <section className="single-column">
      <div className="panel">
        <div className="panel-header">
          <div>
            <span className="panel-eyebrow">Transcript</span>
            <h2>Live transcript</h2>
            <p>{activeMeeting ? activeMeeting.title : "Select or start a meeting"}</p>
          </div>
          {activeMeeting ? <span className="recording-chip">{transcript.length} saved</span> : null}
        </div>
        <div className="transcript-timeline">
          {transcript.length > 0 ? (
            transcript.map((segment) => (
              <article key={segment.id} className="timeline-segment">
                <span>{formatDuration(Math.floor(segment.startTime))}</span>
                <div>
                  <strong>{segment.speaker ?? "Speaker"}</strong>
                  <p>{segment.text}</p>
                </div>
              </article>
            ))
          ) : (
            <div className="empty-state">
              <strong>No transcript yet</strong>
              <p>
                {activeMeeting?.status === "processing" || transcriptionJob?.status === "running"
                  ? "Local transcription is still running."
                  : "Start a meeting or install a model in Settings to generate local transcripts."}
              </p>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

// ─── People management (in Settings) ────────────────────────────────────────

function PeoplePanel({
  speakers,
  onAddPerson,
  onRenamePerson,
  onDeletePerson,
}: {
  speakers: Speaker[];
  onAddPerson: (name: string) => Promise<void>;
  onRenamePerson: (speakerId: string, name: string) => Promise<void>;
  onDeletePerson: (speakerId: string) => Promise<void>;
}) {
  const [newName, setNewName] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    setNewName("");
    await onAddPerson(name);
  };

  const startEdit = (speaker: Speaker) => {
    setEditingId(speaker.id);
    setEditName(speaker.name);
  };

  const commitEdit = async (speakerId: string) => {
    const name = editName.trim();
    if (!name) return;
    await onRenamePerson(speakerId, name);
    setEditingId(null);
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <div>
          <span className="panel-eyebrow">Contacts</span>
          <h2>People</h2>
          <p>Known contacts matched against detected speakers.</p>
        </div>
      </div>
      <form className="inline-form" onSubmit={(e) => void handleAdd(e)}>
        <input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="Add person..."
          autoComplete="off"
          aria-label="New person name"
        />
        <button type="submit" className="secondary-button" disabled={!newName.trim()}>
          Add
        </button>
      </form>
      {speakers.length === 0 ? (
        <p className="empty-hint">No contacts yet.</p>
      ) : (
        <ul className="people-list">
          {speakers.map((s) => (
            <li key={s.id} className="people-item">
              {editingId === s.id ? (
                <div className="task-edit-row">
                  <input
                    autoFocus
                    className="task-edit-input"
                    value={editName}
                    onChange={(e) => setEditName(e.target.value)}
                    onKeyDown={async (e) => {
                      if (e.key === "Enter") await commitEdit(s.id);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    aria-label="Edit name"
                  />
                  <button className="ghost-button" onClick={() => void commitEdit(s.id)}>Save</button>
                  <button className="ghost-button" onClick={() => setEditingId(null)}>Cancel</button>
                </div>
              ) : (
                <>
                  <button
                    className="task-text-button"
                    onClick={() => startEdit(s)}
                    aria-label={`Edit ${s.name}`}
                  >
                    {s.name}
                  </button>
                  <button
                    className="task-delete"
                    aria-label={`Delete ${s.name}`}
                    onClick={() => void onDeletePerson(s.id)}
                  >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                      <path d="M2 2L10 10M10 2L2 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                    </svg>
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ─── Summary templates (in Settings) ────────────────────────────────────────

const PRESET_TEMPLATES = [
  {
    id: "__preset_standard__",
    name: "Standard",
    promptTemplate: "Generate an extensive meeting summary including key discussion points, decisions made, and action items with assignees. Format with sections for Overview, Key Points, Decisions, and Action Items.",
    isDefault: true,
    createdAt: "",
  },
  {
    id: "__preset_brief__",
    name: "Brief",
    promptTemplate: "Generate a short executive summary of this meeting in 3-5 sentences. Focus on the most important outcomes only.",
    isDefault: false,
    createdAt: "",
  },
  {
    id: "__preset_action__",
    name: "Action-Focused",
    promptTemplate: "Focus on decisions made and action items from this meeting. Minimize narrative - output only: Decisions (bullet list) and Action Items (bullet list with owner and deadline if mentioned).",
    isDefault: false,
    createdAt: "",
  },
] satisfies SummaryTemplate[];

function TemplatesPanel({
  templates,
  onAddTemplate,
  onUpdateTemplate,
  onDeleteTemplate,
  onSetDefault,
}: {
  templates: SummaryTemplate[];
  onAddTemplate: (name: string, promptTemplate: string) => Promise<void>;
  onUpdateTemplate: (templateId: string, name: string, promptTemplate: string) => Promise<void>;
  onDeleteTemplate: (templateId: string) => Promise<void>;
  onSetDefault: (templateId: string) => Promise<void>;
}) {
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editPrompt, setEditPrompt] = useState("");
  const [newName, setNewName] = useState("");
  const [newPrompt, setNewPrompt] = useState("");
  const [addingNew, setAddingNew] = useState(false);

  const allTemplates = [...PRESET_TEMPLATES, ...templates];

  const startEdit = (t: SummaryTemplate) => {
    if (t.id.startsWith("__preset_")) {
      setExpandedId(expandedId === t.id ? null : t.id);
      return;
    }
    setExpandedId(t.id);
    setEditName(t.name);
    setEditPrompt(t.promptTemplate);
  };

  const commitEdit = async (templateId: string) => {
    const name = editName.trim();
    const prompt = editPrompt.trim();
    if (!name || !prompt) return;
    await onUpdateTemplate(templateId, name, prompt);
    setExpandedId(null);
  };

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    const prompt = newPrompt.trim();
    if (!name || !prompt) return;
    await onAddTemplate(name, prompt);
    setNewName("");
    setNewPrompt("");
    setAddingNew(false);
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <div>
          <span className="panel-eyebrow">AI</span>
          <h2>Summary Templates</h2>
          <p>Prompt templates used when generating meeting summaries.</p>
        </div>
        <button className="secondary-button" onClick={() => setAddingNew(true)}>
          Add Template
        </button>
      </div>

      {addingNew ? (
        <form className="template-add-form" onSubmit={(e) => void handleAdd(e)}>
          <input
            autoFocus
            className="template-name-input"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="Template name"
            aria-label="New template name"
          />
          <textarea
            className="template-prompt-textarea"
            value={newPrompt}
            onChange={(e) => setNewPrompt(e.target.value)}
            placeholder="Prompt instructions..."
            rows={4}
            aria-label="New template prompt"
          />
          <div className="action-row">
            <button type="submit" className="primary-button" disabled={!newName.trim() || !newPrompt.trim()}>
              Save
            </button>
            <button type="button" className="ghost-button" onClick={() => setAddingNew(false)}>
              Cancel
            </button>
          </div>
        </form>
      ) : null}

      <div className="template-list">
        {allTemplates.map((t) => {
          const isPreset = t.id.startsWith("__preset_");
          const isExpanded = expandedId === t.id;
          return (
            <div key={t.id} className={clsx("template-item", isExpanded && "expanded")}>
              <div className="template-item-header">
                <button
                  className="template-item-name-btn"
                  onClick={() => startEdit(t)}
                  aria-expanded={isExpanded}
                >
                  <span className="template-item-name">{t.name}</span>
                  {t.isDefault ? (
                    <span className="template-default-badge">Default</span>
                  ) : null}
                  {isPreset ? (
                    <span className="template-preset-badge">Built-in</span>
                  ) : null}
                </button>
                <div className="template-item-actions">
                  {!t.isDefault ? (
                    <button
                      className="ghost-button"
                      onClick={() => void onSetDefault(t.id)}
                    >
                      Set default
                    </button>
                  ) : null}
                  {!isPreset && templates.length > 1 ? (
                    <button
                      className="ghost-button danger"
                      aria-label={`Delete ${t.name}`}
                      onClick={() => void onDeleteTemplate(t.id)}
                    >
                      Delete
                    </button>
                  ) : null}
                </div>
              </div>
              {isExpanded ? (
                <div className="template-item-body">
                  {isPreset ? (
                    <p className="template-prompt-preview">{t.promptTemplate}</p>
                  ) : (
                    <>
                      <input
                        className="template-name-input"
                        value={editName}
                        onChange={(e) => setEditName(e.target.value)}
                        placeholder="Template name"
                        aria-label="Template name"
                      />
                      <textarea
                        className="template-prompt-textarea"
                        value={editPrompt}
                        onChange={(e) => setEditPrompt(e.target.value)}
                        rows={5}
                        aria-label="Template prompt"
                      />
                      <div className="action-row">
                        <button
                          className="primary-button"
                          disabled={!editName.trim() || !editPrompt.trim()}
                          onClick={() => void commitEdit(t.id)}
                        >
                          Save
                        </button>
                        <button className="ghost-button" onClick={() => setExpandedId(null)}>
                          Cancel
                        </button>
                      </div>
                    </>
                  )}
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── Tags management panel (in Settings) ────────────────────────────────────

function TagsPanel({
  tags,
  onAddTag,
  onUpdateTag,
  onDeleteTag,
}: {
  tags: Tag[];
  onAddTag: (name: string, color: string) => Promise<void>;
  onUpdateTag: (tagId: string, name: string, color: string) => Promise<void>;
  onDeleteTag: (tagId: string) => Promise<void>;
}) {
  const [newName, setNewName] = useState("");
  const [newColor, setNewColor] = useState(TAG_COLORS[5].value); // Blue default
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editColor, setEditColor] = useState("");

  const handleAdd = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    await onAddTag(name, newColor);
    setNewName("");
    setNewColor(TAG_COLORS[5].value);
  };

  const startEdit = (tag: Tag) => {
    setEditingId(tag.id);
    setEditName(tag.name);
    setEditColor(tag.color);
  };

  const commitEdit = async (tagId: string) => {
    const name = editName.trim();
    if (!name) return;
    await onUpdateTag(tagId, name, editColor);
    setEditingId(null);
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <div>
          <span className="panel-eyebrow">Organization</span>
          <h2>Tags</h2>
          <p>Color-coded tags to organize and filter meetings.</p>
        </div>
      </div>

      <form className="tag-add-form" onSubmit={(e) => void handleAdd(e)}>
        <div className="tag-color-picker">
          {TAG_COLORS.map((c) => (
            <button
              key={c.value}
              type="button"
              className={clsx("tag-color-swatch", newColor === c.value && "tag-color-swatch--selected")}
              style={{ backgroundColor: c.value }}
              onClick={() => setNewColor(c.value)}
              aria-label={`Select color: ${c.name}`}
              title={c.name}
            />
          ))}
        </div>
        <input
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          placeholder="Tag name..."
          autoComplete="off"
          aria-label="New tag name"
        />
        <button type="submit" className="secondary-button" disabled={!newName.trim()}>
          Add Tag
        </button>
      </form>

      {tags.length === 0 ? (
        <p className="empty-hint">No tags yet.</p>
      ) : (
        <ul className="tags-list">
          {tags.map((tag) => (
            <li key={tag.id} className="tags-list-item">
              {editingId === tag.id ? (
                <div className="tags-edit-row">
                  <div className="tag-color-picker tag-color-picker--compact">
                    {TAG_COLORS.map((c) => (
                      <button
                        key={c.value}
                        type="button"
                        className={clsx("tag-color-swatch", editColor === c.value && "tag-color-swatch--selected")}
                        style={{ backgroundColor: c.value }}
                        onClick={() => setEditColor(c.value)}
                        aria-label={`Select color: ${c.name}`}
                        title={c.name}
                      />
                    ))}
                  </div>
                  <input
                    autoFocus
                    className="task-edit-input"
                    value={editName}
                    onChange={(e) => setEditName(e.target.value)}
                    onKeyDown={async (e) => {
                      if (e.key === "Enter") await commitEdit(tag.id);
                      if (e.key === "Escape") setEditingId(null);
                    }}
                    aria-label="Edit tag name"
                  />
                  <button className="ghost-button" onClick={() => void commitEdit(tag.id)}>Save</button>
                  <button className="ghost-button" onClick={() => setEditingId(null)}>Cancel</button>
                </div>
              ) : (
                <>
                  <span className="tag-pill" style={{ "--tag-color": tag.color } as React.CSSProperties}>
                    <span className="tag-pill-dot" />
                    <button
                      className="tag-pill-name-btn"
                      onClick={() => startEdit(tag)}
                      aria-label={`Edit tag: ${tag.name}`}
                    >
                      {tag.name}
                    </button>
                  </span>
                  <button
                    className="task-delete"
                    aria-label={`Delete tag: ${tag.name}`}
                    onClick={() => void onDeleteTag(tag.id)}
                  >
                    <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                      <path d="M2 2L10 10M10 2L2 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                    </svg>
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ─── Webhooks panel (in Settings) ────────────────────────────────────────────

const WEBHOOK_EVENTS = [
  { value: "meeting.completed", label: "Meeting Completed" },
  { value: "meeting.summarized", label: "Meeting Summarized" },
  { value: "meeting.deleted", label: "Meeting Deleted" },
] as const;

type WebhookEvent = (typeof WEBHOOK_EVENTS)[number]["value"];

function webhookEventBadgeClass(event: string): string {
  if (event === "meeting.completed") return "webhook-badge webhook-badge--completed";
  if (event === "meeting.summarized") return "webhook-badge webhook-badge--summarized";
  if (event === "meeting.deleted") return "webhook-badge webhook-badge--deleted";
  return "webhook-badge";
}

function statusCodeClass(code: number | null): string {
  if (code === null) return "delivery-status-code delivery-status-code--unknown";
  if (code >= 200 && code < 300) return "delivery-status-code delivery-status-code--ok";
  if (code >= 300 && code < 400) return "delivery-status-code delivery-status-code--redirect";
  return "delivery-status-code delivery-status-code--error";
}

function WebhookFormPanel({
  initial,
  onSave,
  onCancel,
}: {
  initial?: Partial<Webhook>;
  onSave: (data: { name: string; url: string; events: WebhookEvent[]; secret: string | null; enabled: boolean }) => Promise<void>;
  onCancel: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [events, setEvents] = useState<WebhookEvent[]>((initial?.events ?? []) as WebhookEvent[]);
  const [secret, setSecret] = useState(initial?.secret ?? "");
  const [showSecret, setShowSecret] = useState(false);
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [busy, setBusy] = useState(false);

  const toggleEvent = (event: WebhookEvent) => {
    setEvents((prev) =>
      prev.includes(event) ? prev.filter((e) => e !== event) : [...prev, event],
    );
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || !url.trim() || events.length === 0) return;
    setBusy(true);
    try {
      await onSave({
        name: name.trim(),
        url: url.trim(),
        events,
        secret: secret.trim() || null,
        enabled,
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="webhook-form" onSubmit={(e) => void handleSubmit(e)}>
      <div className="webhook-form-field">
        <label htmlFor="wh-name">Name</label>
        <input
          id="wh-name"
          autoComplete="off"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="My webhook"
          required
        />
      </div>
      <div className="webhook-form-field">
        <label htmlFor="wh-url">URL</label>
        <input
          id="wh-url"
          autoComplete="off"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/webhook"
          required
        />
      </div>
      <div className="webhook-form-field">
        <span className="webhook-form-label">Events</span>
        <div className="webhook-events-checks">
          {WEBHOOK_EVENTS.map((ev) => (
            <label key={ev.value} className="checkbox-row">
              <input
                type="checkbox"
                checked={events.includes(ev.value)}
                onChange={() => toggleEvent(ev.value)}
              />
              {ev.label}
            </label>
          ))}
        </div>
      </div>
      <div className="webhook-form-field">
        <label htmlFor="wh-secret">Secret (optional)</label>
        <div className="webhook-secret-row">
          <input
            id="wh-secret"
            type={showSecret ? "text" : "password"}
            autoComplete="new-password"
            value={secret}
            onChange={(e) => setSecret(e.target.value)}
            placeholder="Signing secret"
          />
          <button
            type="button"
            className="ghost-button"
            onClick={() => setShowSecret((v) => !v)}
            aria-label={showSecret ? "Hide secret" : "Show secret"}
          >
            {showSecret ? "Hide" : "Show"}
          </button>
        </div>
      </div>
      <div className="webhook-form-field">
        <label className="checkbox-row">
          <button
            type="button"
            role="switch"
            aria-checked={enabled}
            className={clsx("toggle-switch", enabled && "active")}
            onClick={() => setEnabled((v) => !v)}
            aria-label="Enable webhook"
          />
          Enabled
        </label>
      </div>
      <div className="webhook-form-actions">
        <button
          type="submit"
          className="primary-button"
          disabled={busy || !name.trim() || !url.trim() || events.length === 0}
        >
          {busy ? "Saving..." : "Save"}
        </button>
        <button type="button" className="secondary-button" onClick={onCancel} disabled={busy}>
          Cancel
        </button>
      </div>
    </form>
  );
}

function WebhookRow({
  webhook,
  onEdit,
  onDelete,
  onTest,
  onToggleEnabled,
}: {
  webhook: Webhook;
  onEdit: () => void;
  onDelete: () => Promise<void>;
  onTest: () => Promise<void>;
  onToggleEnabled: () => Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const [deliveries, setDeliveries] = useState<WebhookDelivery[]>([]);
  const [deliveriesLoading, setDeliveriesLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [testBusy, setTestBusy] = useState(false);
  const [toggleBusy, setToggleBusy] = useState(false);

  const loadDeliveries = async () => {
    setDeliveriesLoading(true);
    try {
      const list = await commands.listWebhookDeliveries(webhook.id, 10);
      setDeliveries(list);
    } catch {
      setDeliveries([]);
    } finally {
      setDeliveriesLoading(false);
    }
  };

  const handleExpand = async () => {
    const next = !expanded;
    setExpanded(next);
    if (next) {
      await loadDeliveries();
    }
  };

  const handleTest = async () => {
    setTestBusy(true);
    try {
      await onTest();
      await loadDeliveries();
    } finally {
      setTestBusy(false);
    }
  };

  const handleToggle = async () => {
    setToggleBusy(true);
    try {
      await onToggleEnabled();
    } finally {
      setToggleBusy(false);
    }
  };

  const truncateUrl = (u: string) => (u.length > 60 ? `${u.slice(0, 57)}...` : u);

  return (
    <li className="webhook-item">
      <div className="webhook-item-header">
        <div className="webhook-item-info">
          <strong className="webhook-item-name">{webhook.name}</strong>
          <span className="webhook-item-url" title={webhook.url}>
            {truncateUrl(webhook.url)}
          </span>
          <div className="webhook-item-badges">
            {webhook.events.map((ev) => (
              <span key={ev} className={webhookEventBadgeClass(ev)}>
                {WEBHOOK_EVENTS.find((e) => e.value === ev)?.label ?? ev}
              </span>
            ))}
          </div>
        </div>
        <div className="webhook-item-controls">
          <button
            type="button"
            role="switch"
            aria-checked={webhook.enabled}
            className={clsx("toggle-switch", webhook.enabled && "active", toggleBusy && "toggle-switch--busy")}
            onClick={() => void handleToggle()}
            aria-label={webhook.enabled ? "Disable webhook" : "Enable webhook"}
            disabled={toggleBusy}
          />
          <button className="ghost-button" onClick={onEdit}>
            Edit
          </button>
          <button
            className="ghost-button"
            onClick={() => void handleExpand()}
            aria-expanded={expanded}
          >
            {expanded ? "Collapse" : "Details"}
          </button>
        </div>
      </div>

      {expanded ? (
        <div className="webhook-detail">
          <div className="webhook-detail-url">
            <span className="webhook-detail-label">URL</span>
            <span className="webhook-detail-full-url">{webhook.url}</span>
          </div>

          <div className="webhook-deliveries">
            <div className="webhook-deliveries-header">
              <span className="webhook-detail-label">Recent deliveries</span>
              <div className="webhook-deliveries-actions">
                <button
                  className="secondary-button"
                  onClick={() => void handleTest()}
                  disabled={testBusy}
                >
                  {testBusy ? "Sending..." : "Send test"}
                </button>
                {confirmDelete ? (
                  <>
                    <button className="ghost-button" onClick={() => setConfirmDelete(false)}>
                      Cancel
                    </button>
                    <button
                      className="secondary-button danger-button"
                      onClick={() => void onDelete()}
                    >
                      Confirm delete
                    </button>
                  </>
                ) : (
                  <button
                    className="secondary-button danger-button"
                    onClick={() => setConfirmDelete(true)}
                  >
                    Delete
                  </button>
                )}
              </div>
            </div>

            {deliveriesLoading ? (
              <p className="empty-hint">Loading...</p>
            ) : deliveries.length === 0 ? (
              <p className="empty-hint">No deliveries yet.</p>
            ) : (
              <ul className="delivery-list">
                {deliveries.map((d) => (
                  <li key={d.id} className="delivery-item">
                    <span className={clsx("delivery-success-icon", d.success ? "delivery-success-icon--ok" : "delivery-success-icon--fail")}>
                      {d.success ? (
                        <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                          <path d="M2 6L5 9L10 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                        </svg>
                      ) : (
                        <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                          <path d="M2 2L10 10M10 2L2 10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                        </svg>
                      )}
                    </span>
                    <span className={webhookEventBadgeClass(d.eventType)}>{d.eventType}</span>
                    <span className={statusCodeClass(d.responseStatus)}>
                      {d.responseStatus ?? "—"}
                    </span>
                    <span className="delivery-time">{formatClock(d.createdAt)}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      ) : null}
    </li>
  );
}

function WebhooksPanel({
  onAlert,
}: {
  onAlert: (alert: { level: "info" | "warning" | "error" | "success"; title: string; message: string }) => void;
}) {
  const [webhooks, setWebhooks] = useState<Webhook[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [editingWebhook, setEditingWebhook] = useState<Webhook | null>(null);

  const loadWebhooks = async () => {
    try {
      const list = await commands.listWebhooks();
      setWebhooks(list);
    } catch {
      setWebhooks([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadWebhooks();
  }, []);

  const handleSave = async (data: {
    name: string;
    url: string;
    events: string[];
    secret: string | null;
    enabled: boolean;
  }) => {
    try {
      if (editingWebhook) {
        await commands.updateWebhook(
          editingWebhook.id,
          data.name,
          data.url,
          data.events,
          data.secret,
          data.enabled,
        );
        onAlert({ level: "success", title: "Webhook updated", message: `"${data.name}" was saved.` });
      } else {
        await commands.createWebhook(data.name, data.url, data.events, data.secret ?? undefined);
        onAlert({ level: "success", title: "Webhook created", message: `"${data.name}" is now active.` });
      }
      setShowForm(false);
      setEditingWebhook(null);
      await loadWebhooks();
    } catch (error) {
      onAlert({ level: "error", title: "Save failed", message: String(error) });
    }
  };

  const handleDelete = async (webhook: Webhook) => {
    try {
      await commands.deleteWebhook(webhook.id);
      onAlert({ level: "info", title: "Webhook deleted", message: `"${webhook.name}" was removed.` });
      await loadWebhooks();
    } catch (error) {
      onAlert({ level: "error", title: "Delete failed", message: String(error) });
    }
  };

  const handleTest = async (webhook: Webhook) => {
    try {
      await commands.testWebhook(webhook.id);
      onAlert({ level: "info", title: "Test event sent", message: `Test event dispatched to "${webhook.name}".` });
    } catch (error) {
      onAlert({ level: "error", title: "Test failed", message: String(error) });
    }
  };

  const handleToggleEnabled = async (webhook: Webhook) => {
    try {
      await commands.updateWebhook(
        webhook.id,
        webhook.name,
        webhook.url,
        webhook.events,
        webhook.secret,
        !webhook.enabled,
      );
      await loadWebhooks();
    } catch (error) {
      onAlert({ level: "error", title: "Toggle failed", message: String(error) });
    }
  };

  const handleEdit = (webhook: Webhook) => {
    setEditingWebhook(webhook);
    setShowForm(true);
  };

  const handleCancelForm = () => {
    setShowForm(false);
    setEditingWebhook(null);
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <div>
          <span className="panel-eyebrow">Automation</span>
          <h2>Webhooks</h2>
          <p>Receive HTTP callbacks when meetings are completed, summarized, or deleted.</p>
        </div>
        {!showForm ? (
          <button
            className="secondary-button"
            onClick={() => { setEditingWebhook(null); setShowForm(true); }}
          >
            Add Webhook
          </button>
        ) : null}
      </div>

      {showForm ? (
        <WebhookFormPanel
          initial={editingWebhook ?? undefined}
          onSave={handleSave}
          onCancel={handleCancelForm}
        />
      ) : null}

      {loading ? (
        <p className="empty-hint">Loading webhooks...</p>
      ) : webhooks.length === 0 && !showForm ? (
        <p className="empty-hint">No webhooks configured yet.</p>
      ) : (
        <ul className="webhook-list">
          {webhooks.map((wh) => (
            <WebhookRow
              key={wh.id}
              webhook={wh}
              onEdit={() => handleEdit(wh)}
              onDelete={() => handleDelete(wh)}
              onTest={() => handleTest(wh)}
              onToggleEnabled={() => handleToggleEnabled(wh)}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

// ─── Ask AI view ─────────────────────────────────────────────────────────────

function AskAiView({
  activeMeeting,
}: {
  activeMeeting: MeetingDetail | null;
}) {
  const navigate = useNavigate();
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [scope, setScope] = useState<"all" | "current">("all");
  const bottomRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  // Load chat history on mount
  useEffect(() => {
    void commands.listChatMessages(50, 0)
      .then((msgs) => setMessages(msgs.slice().reverse()))
      .catch(() => setMessages([]));
  }, []);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, isLoading]);

  const handleSend = async () => {
    const text = input.trim();
    if (!text || isLoading) return;
    setInput("");

    const userMsg: ChatMessage = {
      id: `local-${Date.now()}`,
      role: "user",
      content: text,
      meetingReferences: null,
      createdAt: new Date().toISOString(),
    };
    setMessages((prev) => [...prev, userMsg]);
    setIsLoading(true);

    try {
      const meetingId = scope === "current" && activeMeeting ? activeMeeting.id : undefined;
      const response: AskAiResponse = await commands.askAi(text, meetingId);

      const assistantMsg: ChatMessage = {
        id: `local-assistant-${Date.now()}`,
        role: "assistant",
        content: response.answer,
        meetingReferences: response.meetingReferences.map((r) => r.meetingId),
        createdAt: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, assistantMsg]);
    } catch (error) {
      const errorMsg: ChatMessage = {
        id: `local-error-${Date.now()}`,
        role: "assistant",
        content: `Error: ${String(error)}`,
        meetingReferences: null,
        createdAt: new Date().toISOString(),
      };
      setMessages((prev) => [...prev, errorMsg]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleClearHistory = async () => {
    await commands.clearChatHistory();
    setMessages([]);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  };

  // Extract meeting references from response and make them navigable
  const renderAssistantContent = (msg: ChatMessage) => {
    const nodes = renderMarkdown(msg.content);
    return nodes;
  };

  return (
    <section className="ask-ai-view">
      <div className="ask-ai-header">
        <div className="ask-ai-header-left">
          <h2>Ask AI</h2>
          <p>Ask questions about your meetings.</p>
        </div>
        <div className="ask-ai-header-actions">
          {activeMeeting ? (
            <div className="ask-ai-scope-toggle">
              <button
                className={clsx("scope-toggle-btn", scope === "all" && "active")}
                onClick={() => setScope("all")}
              >
                All meetings
              </button>
              <button
                className={clsx("scope-toggle-btn", scope === "current" && "active")}
                onClick={() => setScope("current")}
                title={activeMeeting.title}
              >
                Current meeting
              </button>
            </div>
          ) : null}
          <button
            className="ghost-button"
            onClick={() => void handleClearHistory()}
            disabled={messages.length === 0}
          >
            Clear history
          </button>
        </div>
      </div>

      <div className="ask-ai-messages" aria-live="polite" aria-label="Chat messages">
        {messages.length === 0 && !isLoading ? (
          <div className="ask-ai-empty">
            <strong>Ask anything about your meetings</strong>
            <p>Questions, summaries, action items, follow-ups...</p>
          </div>
        ) : null}
        {messages.map((msg) => (
          <div
            key={msg.id}
            className={clsx("chat-message", msg.role === "user" ? "chat-message--user" : "chat-message--assistant")}
          >
            <div className="chat-message-bubble">
              {msg.role === "assistant" ? (
                <div className="md-body chat-md-body">
                  {renderAssistantContent(msg)}
                  {msg.meetingReferences && msg.meetingReferences.length > 0 ? (
                    <div className="chat-meeting-refs">
                      {msg.meetingReferences.map((id) => (
                        <button
                          key={id}
                          className="chat-meeting-ref-link"
                          onClick={() => navigate(`/?meeting=${id}`)}
                          aria-label={`Go to meeting ${id}`}
                        >
                          View meeting
                        </button>
                      ))}
                    </div>
                  ) : null}
                </div>
              ) : (
                <p>{msg.content}</p>
              )}
            </div>
            <span className="chat-message-time">{formatClock(msg.createdAt)}</span>
          </div>
        ))}
        {isLoading ? (
          <div className="chat-message chat-message--assistant">
            <div className="chat-message-bubble">
              <div className="chat-typing-indicator" aria-label="AI is typing">
                <span /><span /><span />
              </div>
            </div>
          </div>
        ) : null}
        <div ref={bottomRef} />
      </div>

      <div className="ask-ai-input-bar">
        <textarea
          ref={inputRef}
          className="ask-ai-textarea"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Ask a question... (Enter to send, Shift+Enter for new line)"
          rows={2}
          disabled={isLoading}
          aria-label="Chat input"
        />
        <button
          className="primary-button ask-ai-send-btn"
          onClick={() => void handleSend()}
          disabled={!input.trim() || isLoading}
          aria-label="Send message"
        >
          {isLoading ? (
            <span className="processing-spinner" aria-hidden="true" />
          ) : (
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
              <path d="M14 8L2 2L5 8L2 14L14 8Z" fill="currentColor" />
            </svg>
          )}
        </button>
      </div>
    </section>
  );
}

// ─── LLM settings panel ────────────────────────────────────────────────────

const LLM_PROVIDERS = [
  { value: "anthropic", label: "Anthropic (Claude)" },
  { value: "openai", label: "OpenAI" },
] as const;

function LlmSettingsPanel({ onAlert }: { onAlert: (alert: AlertEvent) => void }) {
  const [llm, setLlm] = useState<LlmSettings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    void commands.getLlmSettings().then(setLlm);
  }, []);

  if (!llm) return null;

  const handleSave = async () => {
    setSaving(true);
    try {
      const updated = await commands.updateLlmSettings(llm);
      setLlm(updated);
      onAlert({ level: "success", title: "LLM settings saved", message: "Your API configuration has been updated." });
    } catch (error) {
      onAlert({ level: "error", title: "Failed to save LLM settings", message: String(error) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="panel">
      <div className="panel-header">
        <div>
          <span className="panel-eyebrow">AI</span>
          <h2>LLM Configuration</h2>
          <p>API key and model for summaries and Ask AI.</p>
        </div>
        <button className="primary-button" disabled={saving} onClick={() => void handleSave()}>
          {saving ? "Saving..." : "Save"}
        </button>
      </div>
      <div className="settings-grid">
        <label>
          Provider
          <select
            value={llm.provider}
            onChange={(e) => setLlm((c) => c ? { ...c, provider: e.target.value } : c)}
          >
            {LLM_PROVIDERS.map((p) => (
              <option key={p.value} value={p.value}>{p.label}</option>
            ))}
          </select>
        </label>
        <label>
          API Key
          <input
            type="password"
            autoComplete="off"
            placeholder={llm.provider === "anthropic" ? "sk-ant-..." : "sk-..."}
            value={llm.apiKey}
            onChange={(e) => setLlm((c) => c ? { ...c, apiKey: e.target.value } : c)}
          />
        </label>
        <label>
          Model
          <input
            autoComplete="off"
            placeholder={llm.provider === "anthropic" ? "claude-sonnet-4-20250514" : "gpt-4o"}
            value={llm.model}
            onChange={(e) => setLlm((c) => c ? { ...c, model: e.target.value } : c)}
          />
        </label>
        <label>
          Detail level
          <select
            value={llm.detailLevel}
            onChange={(e) => setLlm((c) => c ? { ...c, detailLevel: e.target.value } : c)}
          >
            <option value="brief">Brief</option>
            <option value="standard">Standard</option>
            <option value="extensive">Extensive</option>
          </select>
        </label>
      </div>
    </div>
  );
}

// ─── Settings view ──────────────────────────────────────────────────────────

function SettingsView({
  settings,
  helperStatus,
  permissions,
  models,
  modelBusyId,
  speakers,
  templates,
  tags,
  onUpdateSettings,
  onRequestMic,
  onRequestScreen,
  onOpenSystemSettings,
  onDownloadModel,
  onSelectModel,
  onAddPerson,
  onRenamePerson,
  onDeletePerson,
  onAddTemplate,
  onUpdateTemplate,
  onDeleteTemplate,
  onSetDefaultTemplate,
  onAddTag,
  onUpdateTag,
  onDeleteTag,
  onAlert,
}: {
  settings: AppSettings | null;
  helperStatus: NativeHelperStatus | null;
  permissions: PermissionStatus;
  models: TranscriptionModel[];
  modelBusyId: string | null;
  speakers: Speaker[];
  templates: SummaryTemplate[];
  tags: Tag[];
  onUpdateSettings: (settings: AppSettings) => Promise<void>;
  onRequestMic: () => Promise<void>;
  onRequestScreen: () => Promise<void>;
  onOpenSystemSettings: () => Promise<void>;
  onDownloadModel: (modelId: string) => Promise<void>;
  onSelectModel: (modelId: string) => Promise<void>;
  onAddPerson: (name: string) => Promise<void>;
  onRenamePerson: (speakerId: string, name: string) => Promise<void>;
  onDeletePerson: (speakerId: string) => Promise<void>;
  onAddTemplate: (name: string, promptTemplate: string) => Promise<void>;
  onUpdateTemplate: (templateId: string, name: string, promptTemplate: string) => Promise<void>;
  onDeleteTemplate: (templateId: string) => Promise<void>;
  onSetDefaultTemplate: (templateId: string) => Promise<void>;
  onAddTag: (name: string, color: string) => Promise<void>;
  onUpdateTag: (tagId: string, name: string, color: string) => Promise<void>;
  onDeleteTag: (tagId: string) => Promise<void>;
  onAlert: (alert: AlertEvent) => void;
}) {
  const [draft, setDraft] = useState<AppSettings | null>(settings);

  useEffect(() => {
    setDraft(settings);
  }, [settings]);

  if (!draft) {
    return (
      <section className="single-column">
        <div className="panel empty-state">
          <strong>Loading settings</strong>
        </div>
      </section>
    );
  }

  return (
    <section className="single-column settings-layout">
      <div className="panel">
        <div className="panel-header">
          <div>
            <span className="panel-eyebrow">Preferences</span>
            <h2>Preferences</h2>
            <p>Rust owns the saved settings; this window edits the local state.</p>
          </div>
          <button className="primary-button" onClick={() => void onUpdateSettings(draft)}>
            Save settings
          </button>
        </div>

        <div className="settings-grid">
          <label>
            Primary language
            <input
              autoComplete="off"
              name="primary-language"
              value={draft.primaryLanguage}
              onChange={(event) =>
                setDraft((current) =>
                  current ? { ...current, primaryLanguage: event.target.value } : current,
                )
              }
            />
          </label>
          <label>
            Input device
            <input
              autoComplete="off"
              name="input-device"
              value={draft.selectedInputDevice}
              onChange={(event) =>
                setDraft((current) =>
                  current ? { ...current, selectedInputDevice: event.target.value } : current,
                )
              }
            />
          </label>
          <label>
            Output device
            <input
              autoComplete="off"
              name="output-device"
              value={draft.selectedOutputDevice}
              onChange={(event) =>
                setDraft((current) =>
                  current ? { ...current, selectedOutputDevice: event.target.value } : current,
                )
              }
            />
          </label>
          <label>
            Storage path
            <input
              autoComplete="off"
              name="storage-path"
              value={draft.storagePath}
              onChange={(event) =>
                setDraft((current) =>
                  current ? { ...current, storagePath: event.target.value } : current,
                )
              }
            />
          </label>
          <label className="checkbox-row">
            <input
              checked={draft.launchAtLogin}
              type="checkbox"
              onChange={(event) =>
                setDraft((current) =>
                  current ? { ...current, launchAtLogin: event.target.checked } : current,
                )
              }
            />
            Launch at login
          </label>
        </div>
      </div>

      <div className="panel">
        <div className="panel-header">
          <div>
            <span className="panel-eyebrow">Security</span>
            <h2>Permissions</h2>
            <p>Microphone and screen recording permissions are both required for meeting capture.</p>
          </div>
        </div>
        <div className="permission-row">
          <div>
            <strong>Microphone</strong>
            <span>{permissions.microphone ? "Access granted" : "Access missing"}</span>
          </div>
          <span className={clsx("status-pill", permissions.microphone ? "completed" : "failed")}>
            {permissions.microphone ? "Granted" : "Missing"}
          </span>
          <button className="secondary-button" onClick={() => void onRequestMic()}>
            Request microphone
          </button>
        </div>
        <div className="permission-row">
          <div>
            <strong>Screen recording</strong>
            <span>{permissions.screenRecording ? "Access granted" : "Access missing"}</span>
          </div>
          <span
            className={clsx(
              "status-pill",
              permissions.screenRecording ? "completed" : "failed",
            )}
          >
            {permissions.screenRecording ? "Granted" : "Missing"}
          </span>
          <button className="secondary-button" onClick={() => void onRequestScreen()}>
            Request screen recording
          </button>
        </div>
        <button className="secondary-button" onClick={() => void onOpenSystemSettings()}>
          Open system settings
        </button>
      </div>

      <div className="panel">
        <div className="panel-header">
          <div>
            <span className="panel-eyebrow">Transcription Engines</span>
            <h2>Models</h2>
            <p>MLX is primary. Whisper remains available as a local fallback.</p>
          </div>
        </div>
        <div className="model-list">
          {models.map((model) => (
            <article key={model.id} className="model-card">
              <div className="model-card-copy">
                <span className="recording-chip">{model.family.toUpperCase()}</span>
                <strong>{model.name}</strong>
                <span>
                  {model.sizeMb} MB - {model.installed ? "installed" : "not installed"}
                </span>
              </div>
              <div className="model-actions">
                {model.active ? (
                  <span className="status-pill completed">Active</span>
                ) : model.installed ? (
                  <button
                    className="secondary-button"
                    disabled={modelBusyId === model.id}
                    onClick={() => void onSelectModel(model.id)}
                  >
                    {modelBusyId === model.id ? "Switching..." : "Set active"}
                  </button>
                ) : (
                  <button
                    className="secondary-button"
                    disabled={modelBusyId === model.id}
                    onClick={() => void onDownloadModel(model.id)}
                  >
                    {modelBusyId === model.id ? "Downloading..." : "Download"}
                  </button>
                )}
              </div>
            </article>
          ))}
        </div>
      </div>

      <PeoplePanel
        speakers={speakers}
        onAddPerson={onAddPerson}
        onRenamePerson={onRenamePerson}
        onDeletePerson={onDeletePerson}
      />

      <LlmSettingsPanel onAlert={onAlert} />

      <TemplatesPanel
        templates={templates}
        onAddTemplate={onAddTemplate}
        onUpdateTemplate={onUpdateTemplate}
        onDeleteTemplate={onDeleteTemplate}
        onSetDefault={onSetDefaultTemplate}
      />

      <TagsPanel
        tags={tags}
        onAddTag={onAddTag}
        onUpdateTag={onUpdateTag}
        onDeleteTag={onDeleteTag}
      />

      <WebhooksPanel onAlert={onAlert} />

      <div className="panel helper-panel">
        <span className="panel-eyebrow">Diagnostics</span>
        <h2>Native helper boundary</h2>
        <p>
          {helperStatus?.mode === "connected"
            ? "Helper connected for real permissions and combined meeting capture."
            : "Native helper unavailable."}
        </p>
        <span className="path-text">
          {helperStatus?.executablePath ?? "Build native-helper/ to attach the Swift bridge."}
        </span>
      </div>
    </section>
  );
}

// ─── Desktop app ────────────────────────────────────────────────────────────

type QuitDialogState = "hidden" | "visible";

function DesktopApp() {
  const location = useLocation();
  const navigate = useNavigate();
  const requestedMeetingId =
    location.pathname === "/" ? new URLSearchParams(location.search).get("meeting") : null;
  const [recording, setRecording] = useState<RecordingStatus>({
    state: "idle",
    meetingId: null,
    startedAt: null,
    durationSeconds: 0,
  });
  const [permissions, setPermissions] = useState<PermissionStatus>({
    microphone: false,
    screenRecording: false,
  });
  const [meetings, setMeetings] = useState<MeetingSummary[]>([]);
  const [activeMeeting, setActiveMeeting] = useState<MeetingDetail | null>(null);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [search, setSearch] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [playback, setPlayback] = useState<PlaybackState | null>(null);
  const [helperStatus, setHelperStatus] = useState<NativeHelperStatus | null>(null);
  const [alerts, setAlerts] = useState<AlertEvent[]>([]);
  const [models, setModels] = useState<TranscriptionModel[]>([]);
  const [modelBusyId, setModelBusyId] = useState<string | null>(null);
  const [isSummarizing, setIsSummarizing] = useState(false);
  const [quitDialog, setQuitDialog] = useState<QuitDialogState>("hidden");
  const [quitBusy, setQuitBusy] = useState(false);
  const [speakers, setSpeakers] = useState<Speaker[]>([]);
  const [templates, setTemplates] = useState<SummaryTemplate[]>([]);
  const [tags, setTags] = useState<Tag[]>([]);
  const [selectedTagIds, setSelectedTagIds] = useState<string[]>([]);
  const deferredSearch = useDeferredValue(search);
  const meetingUpdateTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const recordingRef = useRef(recording);
  recordingRef.current = recording;
  const activeAudioUrl = useMemo(
    () => audioSourceForPath(playback?.mediaPath ?? activeMeeting?.audioFilePath ?? null),
    [activeMeeting?.audioFilePath, playback?.mediaPath],
  );

  const pushAlert = useCallback((alert: AlertEvent) => {
    setAlerts((current) => {
      const isDuplicate = current.some(
        (existing) => existing.title === alert.title && existing.message === alert.message,
      );
      if (isDuplicate) return current;
      return [alert, ...current].slice(0, 5);
    });
  }, []);

  const requestPermission = async (
    label: "microphone" | "screen recording",
    request: () => Promise<PermissionStatus>,
    granted: (nextPermissions: PermissionStatus) => boolean,
  ) => {
    try {
      const nextPermissions = await request();
      setPermissions(nextPermissions);
      pushAlert({
        level: granted(nextPermissions) ? "success" : "info",
        title: `${label[0].toUpperCase()}${label.slice(1)} permission`,
        message: granted(nextPermissions)
          ? `${label[0].toUpperCase()}${label.slice(1)} access is now granted.`
          : `${label[0].toUpperCase()}${label.slice(1)} access is still denied. If you previously blocked it, enable it in System Settings.`,
      });
    } catch (error) {
      pushAlert({
        level: "error",
        title: `${label[0].toUpperCase()}${label.slice(1)} request failed`,
        message: String(error),
      });
    }
  };

  const refreshTasks = async (meetingId: string) => {
    try {
      const nextTasks = await commands.listTasks(meetingId);
      setTasks(nextTasks);
    } catch {
      // Tasks are non-critical; ignore failures silently
      setTasks([]);
    }
  };

  const refreshMeetings = async (nextSearch = search) => {
    const meetingList = await commands.listMeetings(nextSearch);
    setMeetings(meetingList);
    const preferredId =
      requestedMeetingId && meetingList.some((meeting) => meeting.id === requestedMeetingId)
        ? requestedMeetingId
        : activeMeeting?.id && meetingList.some((meeting) => meeting.id === activeMeeting.id)
        ? activeMeeting.id
        : null;
    if (preferredId) {
      const detail = await commands.getMeetingDetail(preferredId);
      setActiveMeeting(detail);
      await refreshTasks(preferredId);
    } else {
      setActiveMeeting(null);
      setTasks([]);
    }
    if (nextSearch.trim()) {
      setResults(await commands.searchTranscripts(nextSearch));
    } else {
      setResults([]);
    }
  };

  const refreshSettings = async () => {
    const [nextSettings, nextPermissions, nextHelper, nextModels, nextSpeakers, nextTemplates, nextTags] =
      await Promise.all([
        commands.getSettings(),
        commands.checkPermissions(),
        commands.getNativeHelperStatus(),
        commands.listModels(),
        commands.listSpeakers().catch(() => [] as Speaker[]),
        commands.listTemplates().catch(() => [] as SummaryTemplate[]),
        commands.listTags().catch(() => [] as Tag[]),
      ]);
    setSettings(nextSettings);
    setPermissions(nextPermissions);
    setHelperStatus(nextHelper);
    setModels(nextModels);
    setSpeakers(nextSpeakers);
    setTemplates(nextTemplates);
    setTags(nextTags);
  };

  const refreshSpeakers = async () => {
    const nextSpeakers = await commands.listSpeakers().catch(() => [] as Speaker[]);
    setSpeakers(nextSpeakers);
  };

  const refreshTags = async () => {
    const nextTags = await commands.listTags().catch(() => [] as Tag[]);
    setTags(nextTags);
  };

  const refreshTemplates = async () => {
    const nextTemplates = await commands.listTemplates().catch(() => [] as SummaryTemplate[]);
    setTemplates(nextTemplates);
  };

  useEffect(() => {
    void Promise.all([commands.getRecordingState(), refreshSettings(), commands.getPlaybackState()])
      .then(([nextRecording, , nextPlayback]) => {
        setRecording(nextRecording);
        setPlayback(nextPlayback);
        if (nextRecording.meetingId) {
          navigate("/transcript", { replace: true });
        }
      })
      .catch((error: unknown) => {
        pushAlert({
          level: "error",
          title: "Bootstrap failed",
          message: String(error),
        });
      });

  }, []);

  useEffect(() => {
    void refreshMeetings(deferredSearch);
  }, [deferredSearch]);

  useEffect(() => {
    if (location.pathname !== "/" || !requestedMeetingId) {
      return;
    }

    void commands
      .getMeetingDetail(requestedMeetingId)
      .then(async (detail) => {
        setActiveMeeting(detail);
        await refreshTasks(requestedMeetingId);
      })
      .catch(() => undefined);
  }, [location.pathname, requestedMeetingId]);

  // Auto-select the most recent meeting when navigating to /transcript with nothing selected
  useEffect(() => {
    if (location.pathname === "/transcript" && !activeMeeting && meetings.length > 0) {
      void commands.getMeetingDetail(meetings[0].id).then(setActiveMeeting);
    }
  }, [location.pathname, activeMeeting, meetings]);

  useEffect(() => {
    const unsubs: Array<() => void> = [];
    void Promise.all([
      listen<RecordingStatus>("recording-state-changed", (event) => {
        setRecording(event.payload);
        // Only refresh meetings on state transitions (start/stop/finalize), not every tick
        const isTransition = event.payload.state !== "recording";
        if (isTransition && event.payload.meetingId) {
          void commands.getMeetingDetail(event.payload.meetingId).then(setActiveMeeting);
          void commands.listMeetings(deferredSearch).then(setMeetings);
        }
      }),
      listen<PlaybackState>("playback-state-changed", (event) => setPlayback(event.payload)),
      listen<AlertEvent>("user-alert", (event) => pushAlert(event.payload)),
      listen<string>("meeting-updated", () => {
        // Debounce: live transcription chunks fire many updates per minute
        if (meetingUpdateTimerRef.current) clearTimeout(meetingUpdateTimerRef.current);
        meetingUpdateTimerRef.current = setTimeout(() => {
          void refreshMeetings(deferredSearch);
        }, 2000);
      }),
      listen<string>("models-changed", () => {
        void commands.listModels().then(setModels);
      }),
    ]).then((listeners) => {
      listeners.forEach((unsubscribe) => unsubs.push(unsubscribe));
    });

    return () => {
      unsubs.forEach((unsubscribe) => unsubscribe());
      if (meetingUpdateTimerRef.current) clearTimeout(meetingUpdateTimerRef.current);
    };
  }, [activeMeeting?.id, deferredSearch, requestedMeetingId]);

  // Close-requested protection when recording is active
  useEffect(() => {
    if (!IS_TAURI) return;
    const appWindow = getCurrentWindow();
    let unlisten: (() => void) | undefined;

    void appWindow
      .onCloseRequested((event) => {
        if (recordingRef.current.state === "recording") {
          event.preventDefault();
          setQuitDialog("visible");
        }
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      unlisten?.();
    };
  }, []);

  const startRecording = async () => {
    try {
      const nextRecording = await commands.startRecording();
      setRecording(nextRecording);
      pushAlert({
        level: "success",
        title: "Recording started",
        message: "Meeting capture is live with microphone and system audio.",
      });
      await refreshMeetings(search);
      navigate("/transcript");
    } catch (error) {
      pushAlert({ level: "error", title: "Recording failed", message: String(error) });
    }
  };

  const stopRecording = async () => {
    try {
      const nextRecording = await commands.stopRecording();
      setRecording(nextRecording);
      await refreshMeetings(search);
      pushAlert({
        level: "success",
        title: "Recording saved",
        message: "The meeting recording was saved locally. Transcript processing continues in the background.",
      });
      navigate("/");
    } catch (error) {
      pushAlert({ level: "error", title: "Stop failed", message: String(error) });
    }
  };

  const handleStopAndQuit = async () => {
    setQuitBusy(true);
    try {
      await commands.stopRecording();
      await getCurrentWindow().close();
    } catch {
      setQuitBusy(false);
    }
  };

  const handleQuitWithoutSaving = async () => {
    await getCurrentWindow().destroy();
  };

  const handleSelectMeeting = async (meetingId: string) => {
    const detail = await commands.getMeetingDetail(meetingId);
    setActiveMeeting(detail);
    await refreshTasks(meetingId);
    navigate(`/?meeting=${meetingId}`, { replace: true });
  };

  const handleSummarize = async (meetingId: string, templateId?: string) => {
    setIsSummarizing(true);
    try {
      if (templateId && !templateId.startsWith("__preset_")) {
        await commands.summarizeMeetingWithTemplate(meetingId, templateId);
      } else {
        await commands.summarizeMeeting(meetingId);
      }
      const detail = await commands.getMeetingDetail(meetingId);
      setActiveMeeting(detail);
    } catch (error) {
      pushAlert({ level: "error", title: "Summary failed", message: String(error) });
    } finally {
      setIsSummarizing(false);
    }
  };

  const handleAssignSpeakers = async (
    meetingId: string,
    assignments: Array<{ speakerLabel: string; speakerId: string }>,
  ) => {
    try {
      for (const { speakerLabel, speakerId } of assignments) {
        await commands.assignSpeakerToMeeting(meetingId, speakerLabel, speakerId);
      }
      const detail = await commands.getMeetingDetail(meetingId);
      setActiveMeeting(detail);
      await refreshSpeakers();
      pushAlert({ level: "success", title: "Speakers identified", message: "Speaker assignments saved." });
    } catch (error) {
      pushAlert({ level: "error", title: "Assignment failed", message: String(error) });
    }
  };

  const handleToggleTask = async (taskId: string) => {
    try {
      await commands.toggleTask(taskId);
      if (activeMeeting) await refreshTasks(activeMeeting.id);
    } catch (error) {
      pushAlert({ level: "error", title: "Task update failed", message: String(error) });
    }
  };

  const handleDeleteTask = async (taskId: string) => {
    try {
      await commands.deleteTask(taskId);
      if (activeMeeting) await refreshTasks(activeMeeting.id);
    } catch (error) {
      pushAlert({ level: "error", title: "Task delete failed", message: String(error) });
    }
  };

  const handleUpdateTask = async (
    taskId: string,
    text: string,
    assignee: string | null,
    completed: boolean,
  ) => {
    try {
      await commands.updateTask(taskId, text, assignee, completed);
      if (activeMeeting) await refreshTasks(activeMeeting.id);
    } catch (error) {
      pushAlert({ level: "error", title: "Task update failed", message: String(error) });
    }
  };

  const handleCreateTask = async (meetingId: string, text: string) => {
    try {
      await commands.createTask(meetingId, text);
      await refreshTasks(meetingId);
    } catch (error) {
      pushAlert({ level: "error", title: "Task create failed", message: String(error) });
    }
  };

  const handleUpdateScratchpad = async (meetingId: string, content: string) => {
    try {
      await commands.updateScratchpad(meetingId, content);
    } catch (error) {
      pushAlert({ level: "error", title: "Scratchpad save failed", message: String(error) });
    }
  };

  return (
    <>
      {quitDialog === "visible" && (
        <div className="quit-dialog-backdrop">
          <div className="quit-dialog" role="alertdialog" aria-modal="true" aria-labelledby="quit-dialog-title">
            <h2 id="quit-dialog-title" className="quit-dialog-title">Recording in progress</h2>
            <p className="quit-dialog-body">Stop the recording before quitting?</p>
            <div className="quit-dialog-actions">
              <button
                className="quit-dialog-btn quit-dialog-btn--primary"
                disabled={quitBusy}
                onClick={() => void handleStopAndQuit()}
              >
                {quitBusy ? "Stopping..." : "Stop & Quit"}
              </button>
              <button
                className="quit-dialog-btn quit-dialog-btn--secondary"
                disabled={quitBusy}
                onClick={() => setQuitDialog("hidden")}
              >
                Continue Recording
              </button>
              <button
                className="quit-dialog-btn quit-dialog-btn--danger"
                disabled={quitBusy}
                onClick={() => void handleQuitWithoutSaving()}
              >
                Quit Without Saving
              </button>
            </div>
          </div>
        </div>
      )}
    <AppShell
      recording={recording}
      alerts={alerts}
      onDismissAlert={(index) =>
        setAlerts((current) => current.filter((_, i) => i !== index))
      }
      onStart={startRecording}
      onStop={stopRecording}
    >
      <Routes>
        <Route
          path="/"
          element={
            <MeetingsView
              meetings={meetings}
              activeMeeting={activeMeeting}
              tasks={tasks}
              search={search}
              results={results}
              isSummarizing={isSummarizing}
              templates={templates}
              knownSpeakers={speakers}
              allTags={tags}
              selectedTagIds={selectedTagIds}
              playback={playback}
              audioUrl={activeAudioUrl}
              onSelectMeeting={handleSelectMeeting}
              onSearch={async (value) => {
                startTransition(() => {
                  setSearch(value);
                });
              }}
              onToggleTagFilter={(tagId) => {
                setSelectedTagIds((prev) =>
                  prev.includes(tagId) ? prev.filter((id) => id !== tagId) : [...prev, tagId],
                );
              }}
              onAddTagToMeeting={async (meetingId, tagId) => {
                await commands.addTagToMeeting(meetingId, tagId);
                await refreshMeetings(search);
              }}
              onRemoveTagFromMeeting={async (meetingId, tagId) => {
                await commands.removeTagFromMeeting(meetingId, tagId);
                await refreshMeetings(search);
              }}
              onRenameMeeting={async (meetingId, title) => {
                await commands.renameMeeting(meetingId, title);
                await refreshMeetings(search);
              }}
              onDeleteMeeting={async (meetingId) => {
                await commands.deleteMeeting(meetingId);
                setPlayback(null);
                setActiveMeeting(null);
                setTasks([]);
                navigate("/", { replace: true });
                await refreshMeetings(search);
              }}
              onDeleteTranscript={async (meetingId) => {
                await commands.deleteTranscript(meetingId);
                await refreshMeetings(search);
                pushAlert({
                  level: "info",
                  title: "Transcript deleted",
                  message: "The transcript was removed. The meeting and local audio file remain available.",
                });
              }}
              onExportMeeting={async (meetingId, format) => {
                const path = await commands.exportMeeting(meetingId, format);
                pushAlert({
                  level: "info",
                  title: "Meeting exported",
                  message: `${format.toUpperCase()} written to ${path}`,
                });
              }}
              onLoadPlayback={async (meetingId) => {
                try {
                  setPlayback(await commands.loadPlayback(meetingId));
                  setActiveMeeting(await commands.getMeetingDetail(meetingId));
                } catch (error) {
                  pushAlert({
                    level: "error",
                    title: "Playback failed",
                    message: String(error),
                  });
                }
              }}
              onSeekPlayback={async (seconds) => {
                setPlayback(await commands.seekPlayback(seconds));
              }}
              onSummarize={handleSummarize}
              onToggleTask={handleToggleTask}
              onDeleteTask={handleDeleteTask}
              onUpdateTask={handleUpdateTask}
              onCreateTask={handleCreateTask}
              onUpdateScratchpad={handleUpdateScratchpad}
              onAssignSpeakers={handleAssignSpeakers}
              onBatchDelete={async (meetingIds) => {
                for (const id of meetingIds) {
                  await commands.deleteMeeting(id);
                }
                if (activeMeeting && meetingIds.includes(activeMeeting.id)) {
                  setPlayback(null);
                  setActiveMeeting(null);
                  setTasks([]);
                  navigate("/", { replace: true });
                }
                await refreshMeetings(search);
              }}
            />
          }
        />
        <Route path="/transcript" element={<TranscriptView activeMeeting={activeMeeting} />} />
        <Route
          path="/settings"
          element={
            <SettingsView
              settings={settings}
              helperStatus={helperStatus}
              permissions={permissions}
              models={models}
              modelBusyId={modelBusyId}
              speakers={speakers}
              templates={templates}
              onUpdateSettings={async (nextSettings) => {
                setSettings(await commands.updateSettings(nextSettings));
              }}
              onRequestMic={async () =>
                requestPermission(
                  "microphone",
                  commands.requestMicrophonePermission,
                  (nextPermissions) => nextPermissions.microphone,
                )
              }
              onRequestScreen={async () =>
                requestPermission(
                  "screen recording",
                  commands.requestScreenRecordingPermission,
                  (nextPermissions) => nextPermissions.screenRecording,
                )
              }
              onOpenSystemSettings={async () => {
                const message = await commands.openSystemSettings();
                pushAlert({ level: "info", title: "System settings", message });
              }}
              onDownloadModel={async (modelId) => {
                try {
                  setModelBusyId(modelId);
                  const nextModels = await commands.downloadModel(modelId);
                  setModels(nextModels);
                  pushAlert({
                    level: "success",
                    title: "Model downloaded",
                    message: `${modelId} is installed locally and ready for transcription.`,
                  });
                } catch (error) {
                  pushAlert({
                    level: "error",
                    title: "Model download failed",
                    message: String(error),
                  });
                } finally {
                  setModelBusyId(null);
                }
              }}
              onSelectModel={async (modelId) => {
                try {
                  setModelBusyId(modelId);
                  const nextModels = await commands.selectModel(modelId);
                  setModels(nextModels);
                  setSettings((current) =>
                    current ? { ...current, selectedTranscriptionModel: modelId } : current,
                  );
                  pushAlert({
                    level: "success",
                    title: "Model selected",
                    message: `${modelId} will be used first for local transcription.`,
                  });
                } catch (error) {
                  pushAlert({
                    level: "error",
                    title: "Model selection failed",
                    message: String(error),
                  });
                } finally {
                  setModelBusyId(null);
                }
              }}
              onAddPerson={async (name) => {
                await commands.createSpeaker(name);
                await refreshSpeakers();
              }}
              onRenamePerson={async (speakerId, name) => {
                await commands.renameSpeaker(speakerId, name);
                await refreshSpeakers();
              }}
              onDeletePerson={async (speakerId) => {
                await commands.deleteSpeaker(speakerId);
                await refreshSpeakers();
              }}
              onAddTemplate={async (name, promptTemplate) => {
                await commands.createTemplate(name, promptTemplate);
                await refreshTemplates();
              }}
              onUpdateTemplate={async (templateId, name, promptTemplate) => {
                await commands.updateTemplate(templateId, name, promptTemplate);
                await refreshTemplates();
              }}
              onDeleteTemplate={async (templateId) => {
                await commands.deleteTemplate(templateId);
                await refreshTemplates();
              }}
              onSetDefaultTemplate={async (templateId) => {
                await commands.setDefaultTemplate(templateId);
                await refreshTemplates();
              }}
              tags={tags}
              onAddTag={async (name, color) => {
                await commands.createTag(name, color);
                await refreshTags();
              }}
              onUpdateTag={async (tagId, name, color) => {
                await commands.updateTag(tagId, name, color);
                await refreshTags();
              }}
              onDeleteTag={async (tagId) => {
                await commands.deleteTag(tagId);
                await refreshTags();
              }}
              onAlert={pushAlert}
            />
          }
        />
        <Route
          path="/ask-ai"
          element={<AskAiView activeMeeting={activeMeeting} />}
        />
      </Routes>
    </AppShell>
    </>
  );
}

// ─── Notch overlay ──────────────────────────────────────────────────────────

const IS_TAURI = "__TAURI_INTERNALS__" in window;

function formatTimerMmSs(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

const SHORT_RECORDING_THRESHOLD = 300; // 5 minutes

function NotchOverlay() {
  const [recording, setRecording] = useState<RecordingStatus>({
    state: IS_TAURI ? "idle" : "recording",
    meetingId: null,
    startedAt: null,
    durationSeconds: 0,
  });
  const [elapsed, setElapsed] = useState(0);
  const [stopBusy, setStopBusy] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [scratchpad, setScratchpad] = useState("");
  const [showShortWarning, setShowShortWarning] = useState(false);
  const [showMicMenu, setShowMicMenu] = useState(false);
  const [audioDevices, setAudioDevices] = useState<AudioDevice[]>([]);
  const [selectedDevice, setSelectedDevice] = useState<string>("");
  const micMenuRef = useRef<HTMLDivElement>(null);
  const scratchpadDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    document.documentElement.classList.add("notch-window");
    document.body.classList.add("notch-window");
    return () => {
      document.documentElement.classList.remove("notch-window");
      document.body.classList.remove("notch-window");
    };
  }, []);

  // Sync recording state
  useEffect(() => {
    const unsubs: Array<() => void> = [];

    void commands
      .getRecordingState()
      .then((status) => {
        setRecording(status);
        setElapsed(status.durationSeconds);
      })
      .catch(() => undefined);

    void listen<RecordingStatus>("recording-state-changed", (event) => {
      setRecording(event.payload);
      if (event.payload.state !== "recording") {
        setStopBusy(false);
      }
    }).then((fn) => unsubs.push(fn));

    return () => unsubs.forEach((fn) => fn());
  }, []);

  // Load settings for selected device
  useEffect(() => {
    void commands.getSettings().then((s) => setSelectedDevice(s.selectedInputDevice)).catch(() => undefined);
  }, []);

  // Timer tick
  useEffect(() => {
    if (recording.state !== "recording") return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [recording.state]);

  // Close mic menu on outside click
  useEffect(() => {
    if (!showMicMenu) return;
    const onPointerDown = (e: PointerEvent) => {
      if (micMenuRef.current && !micMenuRef.current.contains(e.target as Node)) {
        setShowMicMenu(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [showMicMenu]);

  // Load audio devices when mic menu opens
  useEffect(() => {
    if (!showMicMenu) return;
    void commands.listAudioDevices().then((devices) => setAudioDevices(devices)).catch(() => undefined);
  }, [showMicMenu]);

  const handleDragStart = (e: React.MouseEvent) => {
    // Only drag on the widget body, not interactive elements
    if ((e.target as HTMLElement).closest("button,textarea,select")) return;
    if (IS_TAURI) {
      void getCurrentWindow().startDragging();
    }
  };

  const handleStopClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (stopBusy) return;
    if (elapsed < SHORT_RECORDING_THRESHOLD) {
      setShowShortWarning(true);
    } else {
      void doStop();
    }
  };

  const doStop = async () => {
    setStopBusy(true);
    try {
      await commands.stopRecording();
    } catch {
      setStopBusy(false);
    }
  };

  const handleScratchpadChange = (value: string) => {
    setScratchpad(value);
    if (recording.meetingId) {
      if (scratchpadDebounceRef.current) clearTimeout(scratchpadDebounceRef.current);
      scratchpadDebounceRef.current = setTimeout(() => {
        if (recording.meetingId) {
          void commands.updateScratchpad(recording.meetingId, value).catch(() => undefined);
        }
      }, 600);
    }
  };

  const handleSelectDevice = async (deviceId: string) => {
    setSelectedDevice(deviceId);
    setShowMicMenu(false);
    try {
      const current = await commands.getSettings();
      await commands.updateSettings({ ...current, selectedInputDevice: deviceId });
    } catch {
      // non-critical
    }
  };

  if (recording.state !== "recording") {
    return null;
  }

  return (
    <div className="notch-shell">
      <section
        aria-label="Recording controls"
        className={clsx("notch-card", expanded && "notch-card--expanded")}
        onMouseDown={handleDragStart}
        onClick={() => {
          if (!expanded) setExpanded(true);
        }}
        style={{ cursor: expanded ? "default" : "grab" }}
      >
        {/* ── Short recording warning (replaces header) ── */}
        {showShortWarning ? (
          <div
            className="notch-warning-overlay"
            role="alertdialog"
            aria-modal="true"
            onClick={(e) => e.stopPropagation()}
          >
            <button
              className="notch-warn-btn notch-warn-btn--stop"
              disabled={stopBusy}
              onClick={() => {
                setShowShortWarning(false);
                void doStop();
              }}
            >
              Stop
            </button>
            <button
              className="notch-warn-btn notch-warn-btn--continue"
              onClick={() => setShowShortWarning(false)}
            >
              Continue
            </button>
          </div>
        ) : (
          <>
            {/* ── Compact header row ── */}
            <div className="notch-header">
              {/* Red pulsing dot + timer */}
              <div className="notch-rec-indicator">
                <span className="notch-rec-dot" aria-hidden="true" />
                <span className="notch-timer" aria-label={`Recording duration: ${formatTimerMmSs(elapsed)}`}>
                  {formatTimerMmSs(elapsed)}
                </span>
              </div>

              {/* Mic level wave */}
              <div className="notch-wave" aria-hidden="true">
                <span className="notch-bar" style={{ animationDelay: "0ms" }} />
                <span className="notch-bar" style={{ animationDelay: "150ms" }} />
                <span className="notch-bar" style={{ animationDelay: "300ms" }} />
                <span className="notch-bar" style={{ animationDelay: "100ms" }} />
                <span className="notch-bar" style={{ animationDelay: "250ms" }} />
              </div>

              {/* Mic selector button */}
              <div className="notch-mic-wrap" ref={micMenuRef}>
                <button
                  className="notch-mic-button"
                  aria-label="Select microphone"
                  onClick={(e) => {
                    e.stopPropagation();
                    setShowMicMenu((v) => !v);
                  }}
                >
                  <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                    <rect x="5" y="1" width="6" height="9" rx="3" fill="currentColor" />
                    <path d="M3 7.5A5 5 0 0 0 13 7.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                    <line x1="8" y1="12.5" x2="8" y2="15" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                    <line x1="5.5" y1="15" x2="10.5" y2="15" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
                  </svg>
                </button>
                {showMicMenu && (
                  <div className="notch-mic-menu" role="listbox" aria-label="Microphone selection">
                    {audioDevices.length === 0 ? (
                      <div className="notch-mic-empty">Loading devices...</div>
                    ) : (
                      audioDevices
                        .filter((d) => d.isInput)
                        .map((device) => (
                          <button
                            key={device.id}
                            role="option"
                            aria-selected={device.id === selectedDevice}
                            className={clsx("notch-mic-option", device.id === selectedDevice && "notch-mic-option--active")}
                            onClick={(e) => {
                              e.stopPropagation();
                              void handleSelectDevice(device.id);
                            }}
                          >
                            {device.id === selectedDevice && (
                              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                                <path d="M1.5 5l2.5 2.5 5-5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                              </svg>
                            )}
                            <span>{device.name}</span>
                          </button>
                        ))
                    )}
                  </div>
                )}
              </div>

              {/* Collapse button (only in expanded mode) */}
              {expanded && (
                <button
                  className="notch-collapse-button"
                  aria-label="Collapse widget"
                  onClick={(e) => {
                    e.stopPropagation();
                    setExpanded(false);
                  }}
                >
                  <svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true">
                    <path d="M2 8l4-4 4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                </button>
              )}

              {/* Stop button */}
              <button
                className="notch-stop-button"
                aria-label="Stop recording"
                disabled={stopBusy}
                onClick={handleStopClick}
              >
                <span className="notch-stop-icon" />
              </button>
            </div>

            {/* ── Expanded: scratchpad ── */}
            {expanded && (
              <div className="notch-scratchpad-wrap" onClick={(e) => e.stopPropagation()}>
                <textarea
                  className="notch-scratchpad"
                  placeholder="Quick notes..."
                  value={scratchpad}
                  onChange={(e) => handleScratchpadChange(e.target.value)}
                  aria-label="Meeting scratchpad"
                />
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}

// ─── Root ───────────────────────────────────────────────────────────────────

export function App() {
  const location = useLocation();

  if (location.pathname === "/notch") {
    return <NotchOverlay />;
  }

  return <DesktopApp />;
}
