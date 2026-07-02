# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A **Claude Code plugin** that shows native Windows toast notifications when Claude needs attention (task complete, permission prompt, idle, plan approval, MCP elicitation, error) and clicks the toast to focus the terminal/editor tab you were using.

This repo is the **swdrts fork** of `chuilishi/claude-code-notify`, with two added features over upstream: **project-name display** in the completion toast and an **error state** (`StopFailure` hook → red-bordered toast showing the API error). Version metadata lives in `.claude-plugin/marketplace.json` (plugin version, currently `1.2.0`) and `.claude-plugin/plugin.json` (crate/binary `1.2.1`); bump both in lockstep when releasing.

## Build

The Rust project is in `src-rust/` (crate name `toast-window`, binary `toast-window.exe`). It targets Windows only (edition 2021, `windows` 0.61 with many `Win32_*` features).

```bash
cd src-rust
cargo build --release                       # → src-rust/target/release/toast-window.exe
cargo test                                  # unit tests (state.rs has the bulk of them)
cargo test project_name_from_cwd_trailing_segment   # run a single test by name
```

The committed binary that the plugin actually invokes is **`notifications/ToastWindow.exe`** — after a release build, copy it there:

```bash
cp src-rust/target/release/toast-window.exe notifications/ToastWindow.exe
```

`.gitignore` excludes `src-rust/target/` but explicitly keeps `notifications/ToastWindow.exe` (`!notifications/ToastWindow.exe`), so the committed binary is the shipped one.

## Debugging the binary

The binary is a GUI-subsystem app (`#![windows_subsystem = "windows"]` in `main.rs`), so it has **no stdout by default**. Debug logging is opt-in via the `--debug` / `-d` flag, which writes to `debug.log` **in the same directory as the exe** (never to stdout/console — allocating a console would pop up a CMD window). `debug_log!` is the project-wide logging macro (defined in `log.rs`, gated by `log::init(args.debug)` in `main`).

To test a hook manually, pipe Claude Code's stdin JSON shape:

```bash
echo '{"session_id":"test123","cwd":"C:\\proj"}' | ./notifications/ToastWindow.exe --save --debug
echo '{"session_id":"test123"}' | ./notifications/ToastWindow.exe --notify --debug
echo '{"session_id":"test123","last_assistant_message":"rate limit"}' | ./notifications/ToastWindow.exe --error --debug
```

Then read `notifications/debug.log`.

## How it ships (plugin system)

No `settings.json` editing — Claude Code discovers the hooks automatically via the plugin. The flow:

1. **`hooks/hooks.json`** maps Claude Code hook events to `ToastWindow.exe` invocations using `${CLAUDE_PLUGIN_ROOT}`. Users install with `claude plugin marketplace add chuilishi/claude-code-notify` then `claude plugin install claude-code-notify@claude-code-notify`. (Note: marketplace/install commands still reference upstream `chuilishi`; only `marketplace.json` `owner` is `swdrts`.)
2. **`notifications/ToastWindow.exe`** is the runtime binary called by every hook.
3. **`.claude-plugin/`** holds `plugin.json` + `marketplace.json` (metadata only).
4. **`notifications/assets/`** holds runtime assets the exe discovers by glob: `img/claude.ico` (default icon), `font/JetBrainsMono-ExtraBold.ttf` (custom toast font), `sound/notification.wav` (chime). The exe resolves these relative to its own directory via `FindFirstFileW`, so asset filenames can vary but extensions/patterns must match.

## Architecture: the two-process model

The binary runs in **six modes** selected by the first CLI flag (`--save`, `--notify`, `--input`, `--error`, `--notify-show`, `--cleanup`). The critical design constraint: **hooks have short timeouts (5–10s) but showing a toast blocks until the user dismisses it**. So most modes follow a two-process split:

- **Hook process** (fast, returns immediately): reads stdin JSON → builds a command line → calls `spawn::spawn_detached()` (Win32 `CreateProcessW` with `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`) to launch a **detached child** of itself with `--notify-show`, then exits. The hook never blocks.
- **Detached child** (`run_notify_show_mode` in `main.rs`): the long-lived process that actually renders the toast window, blocks in the message loop, and handles click-to-activate.

`--notify-show` is the only mode that draws a window. The other modes are state management (`--save`, `--cleanup`) or spawners (`--notify`, `--input`, `--error`).

### The session state file (per-session isolation)

Each Claude Code session passes a unique `session_id` in the stdin JSON. State is persisted as 5 newline-delimited lines in `%TEMP%\claude-notify-{session_id}.txt`:

1. `HWND` (foreground window at prompt time)
2. Windows Terminal tab `RuntimeId` (empty if not WT)
3. Caller exe path (for icon extraction)
4. User prompt
5. Project name (derived from `cwd`'s last path segment via `state::project_name_from_cwd`)

`--save` (UserPromptSubmit) writes it; `--notify-show` reads it; `--cleanup` (SessionEnd) deletes it. **Backward compatibility:** `load_state` tolerates legacy 4-line files (no project name) — line 5 is optional. Tests in `state.rs` lock this contract; keep them passing when changing the format.

### Click-to-activate: focus-stealing + Windows Terminal tabs

Two Windows-specific hard problems, both solved in dedicated modules — read these before touching activation:

- **`activate.rs`** — Windows blocks background processes from stealing focus. The 12-step sequence (`AllowSetForegroundObject(ASFW_ANY)` → ALT-key trick → `AttachThreadInput` across threads → `SetWindowPos`+`BringWindowToTop`+`SetForegroundWindow`) is the workaround. Don't simplify it; each step compensates for a different focus-refusal case.
- **`uiautomation.rs`** — Inside Windows Terminal (`CASCADIA_HOSTING_WINDOW_CLASS`), foregrounding the window isn't enough — the user may be on a different tab. Uses the `IUIAutomation` COM API to capture the selected tab's `RuntimeId` at prompt time, then `IUIAutomationSelectionItemPattern::Select()` to restore it on click.

### Toast rendering & stacking (`toast.rs`, ~780 lines, largest module)

One toast per process. Thread-local `TOAST` state via `with_toast`/`with_toast_mut` helpers. Key behaviors:
- Windows created with `WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_LAYERED` (never steal focus, always on top, alpha-fade animation).
- **Telegram-style stacking:** all toasts share class `ClaudeCodeToast` and find siblings via `EnumWindows`; new toasts appear above existing ones and slide down when a lower one closes. Only the bottom toast owns the dismiss timer; hover on any pauses all.
- Three border colors encode state: normal (completion, orange), input (yellow), error (red) — see `COLOR_BORDER_*` constants.
- **Persistent toast** (recent change as of v1.2.1): no auto-timeout; dismissed only by click/foreground-match. `TIMER_CHECK_FOREGROUND` polls the foreground window and auto-dismisses when the target window returns to front.

### Caller icon extraction (`process.rs`)

Walks the process tree up to 10 levels (Win32 ToolHelp snapshot), skipping `SKIP_LIST` shells/runtimes (cmd, bash, node, python, uv, claude, …) and matching `KNOWN_APPS` (code, cursor, JetBrains IDEs, Windows Terminal, …) to find the real editor, then `ExtractIconExW` pulls its icon. If you add editor support, update both lists.

## Conventions specific to this codebase

- **JSON on stdin is the input contract** — `json.rs` has minimal helpers (`read_stdin_json`, `extract_string`, `extract_cwd`, `extract_first_question`); hooks pass Claude Code's JSON payload there. Never read argv for hook payload, only for mode/flags.
- **Chinese titles** (`"任务完成"`, `"Claude 出错"`) are intentional in `run_notify_show_mode`/`run_error_mode` — this fork's users are Chinese-speaking. Don't "fix" them to English.
- **`sanitize_message()`** flattens newlines and truncates to 35 chars + `...` — applied to the toast body before rendering.
- **Modes pass data to the detached child via argv** (`--title`, `--message`, `--session`), escaping `"` → `\"`. The child never re-reads the original stdin.
- The binary is built with `#![windows_subsystem = "windows"]` — adding any `println!`/`eprintln!` for production debugging is pointless (no console). Use `debug_log!`.
