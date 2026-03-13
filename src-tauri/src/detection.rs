use std::collections::HashSet;
use std::process::Command;

/// Dedicated video call app process name patterns (not browser-based)
const DEDICATED_VIDEO_APPS: &[(&str, &str)] = &[
    ("zoom.us", "Zoom"),
    ("us.zoom.xos", "Zoom"),
    ("Microsoft Teams", "Microsoft Teams"),
    ("Webex", "Webex"),
    ("Slack Helper", "Slack"),
];

pub struct VideoCallDetector {
    known_pids: HashSet<String>,
}

impl VideoCallDetector {
    pub fn new() -> Self {
        // Seed with currently running pids so we only alert on NEW processes
        let known_pids = scan_video_call_pids().unwrap_or_default();
        Self { known_pids }
    }

    /// Returns the name of a newly detected dedicated video call app, if any.
    /// Once a pid is seen, it is added to known_pids so we don't re-alert.
    pub fn check_for_new_calls(&mut self) -> Option<String> {
        let current = scan_video_call_pids_with_names().unwrap_or_default();

        let mut newly_detected: Option<String> = None;
        for (pid, app_name) in &current {
            if !self.known_pids.contains(pid) {
                if newly_detected.is_none() {
                    newly_detected = Some(app_name.clone());
                }
            }
        }

        // Update known pids to the full current set so we don't re-alert
        self.known_pids = current.into_iter().map(|(pid, _)| pid).collect();

        newly_detected
    }
}

/// Scan for running video call app pids. Returns a set of pid strings.
fn scan_video_call_pids() -> Option<HashSet<String>> {
    let output = Command::new("ps").args(["-eo", "pid,comm"]).output().ok()?;

    let process_list = String::from_utf8_lossy(&output.stdout);
    let mut pids = HashSet::new();

    for line in process_list.lines().skip(1) {
        let line = line.trim();
        let (pid_str, comm) = split_pid_comm(line)?;
        for (pattern, _) in DEDICATED_VIDEO_APPS {
            if comm.contains(pattern) {
                pids.insert(pid_str.to_string());
                break;
            }
        }
    }

    Some(pids)
}

/// Scan for running video call app pids with their resolved app names.
fn scan_video_call_pids_with_names() -> Option<Vec<(String, String)>> {
    let output = Command::new("ps").args(["-eo", "pid,comm"]).output().ok()?;

    let process_list = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in process_list.lines().skip(1) {
        let line = line.trim();
        let Some((pid_str, comm)) = split_pid_comm(line) else {
            continue;
        };
        for (pattern, app_name) in DEDICATED_VIDEO_APPS {
            if comm.contains(pattern) {
                results.push((pid_str.to_string(), app_name.to_string()));
                break;
            }
        }
    }

    Some(results)
}

fn split_pid_comm(line: &str) -> Option<(&str, &str)> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let pid = parts.next()?.trim();
    let comm = parts.next()?.trim();
    if pid.is_empty() || comm.is_empty() {
        return None;
    }
    Some((pid, comm))
}
