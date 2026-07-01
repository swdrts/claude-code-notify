//! State file save/load/delete.
//!
//! State file: %TEMP%\claude-notify-{session_id}.txt
//! Format: 4 lines (HWND, RuntimeId, caller exe path, user prompt)

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

/// Data stored in and loaded from the state file.
pub struct State {
    pub target_hwnd: HWND,
    pub wt_hwnd: HWND,
    pub wt_runtime_id: String,
    pub icon_path: String,
    pub user_prompt: String,
    pub project_name: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            target_hwnd: HWND::default(),
            wt_hwnd: HWND::default(),
            wt_runtime_id: String::new(),
            icon_path: String::new(),
            user_prompt: String::new(),
            project_name: String::new(),
        }
    }
}

/// Get the state file path for a session.
pub fn state_file_path(session_id: &str) -> std::path::PathBuf {
    let temp = std::env::temp_dir();
    temp.join(format!("claude-notify-{}.txt", session_id))
}

/// Save state to the state file (5 lines: HWND, RuntimeId, icon, prompt, project).
pub fn save_state(
    session_id: &str,
    hwnd: HWND,
    runtime_id: &str,
    icon_path: &str,
    prompt: &str,
    project_name: &str,
) {
    let path = state_file_path(session_id);
    let hwnd_val = hwnd.0 as usize;
    let content = format!(
        "{}\n{}\n{}\n{}\n{}",
        hwnd_val, runtime_id, icon_path, prompt, project_name
    );
    let _ = std::fs::write(&path, content);
}

/// Load state from the state file.
pub fn load_state(session_id: &str) -> State {
    let mut state = State::default();
    let path = state_file_path(session_id);

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return state,
    };

    let lines: Vec<&str> = content.lines().collect();

    // Line 1: HWND
    if let Some(line) = lines.first() {
        if let Ok(val) = line.trim().parse::<usize>() {
            let hwnd = HWND(val as *mut _);
            if unsafe { IsWindow(Some(hwnd)).as_bool() } {
                state.target_hwnd = hwnd;
                // Check if this is Windows Terminal
                let class = crate::util::get_class_name(hwnd);
                if class == "CASCADIA_HOSTING_WINDOW_CLASS" {
                    state.wt_hwnd = hwnd;
                }
            }
        }
    }

    // Line 2: RuntimeId
    if let Some(line) = lines.get(1) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            state.wt_runtime_id = trimmed.to_string();
        }
    }

    // Line 3: Caller exe path
    if let Some(line) = lines.get(2) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            state.icon_path = trimmed.to_string();
        }
    }

    // Line 4: User prompt
    if let Some(line) = lines.get(3) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            state.user_prompt = trimmed.to_string();
        }
    }

    // Line 5: Project name (optional — legacy 4-line files have no line 5)
    if let Some(line) = lines.get(4) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            state.project_name = trimmed.to_string();
        }
    }

    state
}

/// Delete the state file for a session.
pub fn delete_state(session_id: &str) {
    let path = state_file_path(session_id);
    let _ = std::fs::remove_file(&path);
}

/// Derive a display name from the session's working directory.
/// Returns the last path segment; falls back to "Claude Code" when empty/unparseable.
/// Slash-agnostic (works with both `\` and `/`).
pub fn project_name_from_cwd(cwd: &str) -> String {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "Claude Code".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_from_cwd_trailing_segment() {
        assert_eq!(project_name_from_cwd(r"C:\Users\swdrt\Desktop\My Project"), "My Project");
    }

    #[test]
    fn project_name_from_cwd_forward_slashes() {
        assert_eq!(project_name_from_cwd("C:/Users/swdrt/dev/app"), "app");
    }

    #[test]
    fn project_name_from_cwd_with_trailing_slash() {
        // std::Path::file_name ignores a trailing separator
        assert_eq!(project_name_from_cwd(r"C:\Users\swdrt\Desktop\Proj\"), "Proj");
    }

    #[test]
    fn project_name_from_cwd_empty_falls_back() {
        assert_eq!(project_name_from_cwd(""), "Claude Code");
    }

    #[test]
    fn save_then_load_roundtrips_project_name() {
        // Use a temp dir + a fake session id so we don't collide with real state.
        let session = "unit-test-roundtrip-DO-NOT-USE";
        let hwnd = HWND(0x1234 as *mut _);
        // Clean any leftover
        let _ = std::fs::remove_file(state_file_path(session));

        save_state(session, hwnd, "rid", "icon", "the prompt", "MyProject");

        let loaded = load_state(session);
        assert_eq!(loaded.project_name, "MyProject");
        assert_eq!(loaded.user_prompt, "the prompt");
        assert_eq!(loaded.wt_runtime_id, "rid");

        let _ = std::fs::remove_file(state_file_path(session));
    }

    #[test]
    fn load_state_reads_legacy_4_line_file() {
        // Simulate a file written by the OLD binary (no line 5).
        let session = "unit-test-legacy-DO-NOT-USE";
        let path = state_file_path(session);
        let _ = std::fs::remove_file(&path);
        // HWND 0 is invalid so target_hwnd stays default, but load must not panic
        // and project_name must default to empty.
        std::fs::write(&path, "0\n\n\nlegacy prompt\n").unwrap();

        let loaded = load_state(session);
        assert_eq!(loaded.project_name, "");
        assert_eq!(loaded.user_prompt, "legacy prompt");

        let _ = std::fs::remove_file(&path);
    }
}
