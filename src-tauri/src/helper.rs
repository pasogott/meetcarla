use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::{AudioDevice, NativeHelperStatus, PermissionStatus};

#[derive(Debug, Serialize)]
struct HelperRequest<'a> {
    command: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperResponse {
    status: String,
    message: Option<String>,
    microphone: Option<bool>,
    screen_recording: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioDevicesResponse {
    #[allow(dead_code)]
    status: String,
    devices: Vec<AudioDevice>,
}

#[derive(Clone)]
pub struct SwiftHelperManager {
    candidate_paths: Vec<PathBuf>,
}

impl SwiftHelperManager {
    pub fn discover() -> Self {
        let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| manifest_root.clone());
        let current_dir = std::env::current_dir().ok();
        let current_exe = std::env::current_exe().ok();
        let mut candidate_paths = Vec::new();

        if let Some(current_exe) = current_exe {
            if let Some(macos_dir) = current_exe.parent() {
                candidate_paths.push(macos_dir.join("CarlaNativeHelper"));
                if let Some(contents_dir) = macos_dir.parent() {
                    candidate_paths.push(contents_dir.join("Resources/CarlaNativeHelper"));
                }
            }
        }

        candidate_paths.extend([
            manifest_root.join("bin/CarlaNativeHelper"),
            workspace_root
                .join("native-helper/.build/arm64-apple-macosx/release/CarlaNativeHelper"),
            workspace_root.join("native-helper/.build/debug/CarlaNativeHelper"),
            workspace_root.join("native-helper/.build/release/CarlaNativeHelper"),
        ]);

        if let Some(current_dir) = current_dir {
            candidate_paths.extend([
                current_dir.join("src-tauri/bin/CarlaNativeHelper"),
                current_dir
                    .join("native-helper/.build/arm64-apple-macosx/release/CarlaNativeHelper"),
                current_dir.join("native-helper/.build/debug/CarlaNativeHelper"),
                current_dir.join("native-helper/.build/release/CarlaNativeHelper"),
            ]);
        }

        Self { candidate_paths }
    }

    fn executable_path(&self) -> Option<PathBuf> {
        self.candidate_paths
            .iter()
            .find(|path| path.exists())
            .cloned()
    }

    fn invoke(&self, command: &str) -> Result<HelperResponse> {
        let path = self
            .executable_path()
            .ok_or_else(|| anyhow!("native helper not found; build native-helper/ first"))?;
        let payload = serde_json::to_vec(&HelperRequest { command })?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start helper at {}", path.display()))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("failed to open helper stdin"))?
            .write_all(&payload)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "{}",
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            ));
        }
        serde_json::from_slice::<HelperResponse>(&output.stdout)
            .context("failed to parse helper response")
    }

    fn invoke_for_devices(&self, command: &str) -> Result<AudioDevicesResponse> {
        let path = self
            .executable_path()
            .ok_or_else(|| anyhow!("native helper not found; build native-helper/ first"))?;
        let payload = serde_json::to_vec(&HelperRequest { command })?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start helper at {}", path.display()))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("failed to open helper stdin"))?
            .write_all(&payload)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(anyhow!(
                "{}",
                String::from_utf8_lossy(&output.stderr).trim().to_string()
            ));
        }
        serde_json::from_slice::<AudioDevicesResponse>(&output.stdout)
            .context("failed to parse helper devices response")
    }

    pub fn spawn_meeting_recording(
        &self,
        output_path: &Path,
        stop_path: &Path,
        chunk_dir: &Path,
        device_id: Option<&str>,
    ) -> Result<Child> {
        let path = self
            .executable_path()
            .ok_or_else(|| anyhow!("native helper not found; build native-helper/ first"))?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = stop_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(chunk_dir)?;
        if stop_path.exists() {
            let _ = std::fs::remove_file(stop_path);
        }
        let mut cmd = Command::new(&path);
        cmd.arg("record-meeting")
            .arg("--output")
            .arg(output_path)
            .arg("--stop-file")
            .arg(stop_path)
            .arg("--chunk-dir")
            .arg(chunk_dir);
        if let Some(id) = device_id {
            cmd.arg("--device-id").arg(id);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start helper meeting recorder at {}",
                    path.display()
                )
            })
    }

    pub fn list_audio_devices(&self) -> Result<Vec<AudioDevice>> {
        let response = self.invoke_for_devices("list_audio_devices")?;
        Ok(response.devices)
    }

    pub fn check_permissions(&self) -> Result<PermissionStatus> {
        let response = self.invoke("check_permissions")?;
        Ok(PermissionStatus {
            microphone: response.microphone.unwrap_or(false),
            screen_recording: response.screen_recording.unwrap_or(false),
        })
    }

    pub fn request_microphone_permission(&self) -> Result<PermissionStatus> {
        let response = self.invoke("request_microphone_permission")?;
        Ok(PermissionStatus {
            microphone: response.microphone.unwrap_or(false),
            screen_recording: response.screen_recording.unwrap_or(false),
        })
    }

    pub fn request_screen_recording_permission(&self) -> Result<PermissionStatus> {
        let response = self.invoke("request_screen_recording_permission")?;
        Ok(PermissionStatus {
            microphone: response.microphone.unwrap_or(false),
            screen_recording: response.screen_recording.unwrap_or(false),
        })
    }

    pub fn open_system_settings(&self) -> Result<String> {
        let response = self.invoke("open_system_settings")?;
        if response.status == "ok" {
            Ok(response
                .message
                .unwrap_or_else(|| "Opened macOS privacy settings.".into()))
        } else {
            Err(anyhow!(
                "{}",
                response
                    .message
                    .unwrap_or_else(|| "helper returned an error".into())
            ))
        }
    }

    pub fn status(&self) -> NativeHelperStatus {
        for path in &self.candidate_paths {
            if path.exists() {
                return match Command::new(path).arg("--ping").output() {
                    Ok(output) if output.status.success() => NativeHelperStatus {
                        mode: "connected".into(),
                        executable_path: Some(path.to_string_lossy().to_string()),
                        last_error: None,
                    },
                    Ok(output) => NativeHelperStatus {
                        mode: "stub".into(),
                        executable_path: Some(path.to_string_lossy().to_string()),
                        last_error: Some(
                            String::from_utf8_lossy(&output.stderr).trim().to_string(),
                        ),
                    },
                    Err(error) => NativeHelperStatus {
                        mode: "stub".into(),
                        executable_path: Some(path.to_string_lossy().to_string()),
                        last_error: Some(error.to_string()),
                    },
                };
            }
        }

        NativeHelperStatus {
            mode: "stub".into(),
            executable_path: None,
            last_error: None,
        }
    }
}
