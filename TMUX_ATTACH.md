# Task: Attach to Existing tmux Session

## Goal

Allow users to attach to an already-running tmux session through ZeroMux's web UI, using **方案 A**:
spawn `tmux attach -t <session_name>` inside a ZeroMux PTY.

ZeroMux crash or browser断开 only kills the tmux **client**, not the tmux server/session.

---

## Changes

### 1. Backend: tmux list API

**New endpoint**: `GET /api/tmux/sessions`

- Run `tmux ls -F "#{session_name}|#{session_windows}|#{session_attached}|#{session_created}"` on the host
- Parse output, return JSON array:
  ```json
  [
    { "name": "main", "windows": 3, "attached": 1, "created": "2026-05-01T..." },
    { "name": "dev",  "windows": 1, "attached": 0, "created": "2026-04-30T..." }
  ]
  ```
- If `tmux ls` fails (no server running), return empty array `[]`

### 2. Backend: session creation supports attach mode

**Modify** `POST /api/sessions`:

- Add optional field `tmux_target: Option<String>` to `CreateSessionReq`
- When `type == tmux` and `tmux_target` is set:
  - `PtyHandle::spawn("tmux", &["attach", "-t", &tmux_target], ...)` instead of spawning a bare shell
  - `work_dir` can be ignored (tmux session has its own cwd)
  - Session name defaults to `tmux_target` if not explicitly provided

### 3. Frontend: session creation flow

**Modify** Sidebar.tsx session creation modal:

- When user selects "Terminal" type, show a **sub-choice**:
  - **New Shell** — current flow (pick directory → create)
  - **Attach tmux** — fetch `GET /api/tmux/sessions` → show list → select → create
- If no tmux sessions exist, show "No tmux sessions running" with the list empty/disabled
- After attach, the session appears in sidebar like any other tmux session (Terminal icon, green)

### 4. Frontend: API client

**Add** to `lib/api.ts`:
- `listTmuxSessions()` — `GET /api/tmux/sessions`
- Update `createSession()` to accept optional `tmuxTarget` param

---

## Scope boundaries

- No new session type — reuse `SessionType::Tmux`, differentiated only by how the PTY is spawned
- No tmux control mode — just a plain `tmux attach` inside a PTY
- No tmux session **creation** from the UI (user creates tmux sessions themselves via CLI)
- No special detach handling — if the tmux session ends, the ZeroMux PTY closes naturally

---

## Files to modify

| File | Change |
|------|--------|
| `src/web.rs` | Add `/api/tmux/sessions` endpoint; extend `CreateSessionReq` with `tmux_target` |
| `src/session_manager.rs` | Branch on `tmux_target` in `create_pty_session` to spawn attach command |
| `src/pty_bridge.rs` | No change needed (already accepts arbitrary cmd + args) |
| `frontend/src/lib/api.ts` | Add `listTmuxSessions()`; update `createSession()` signature |
| `frontend/src/components/Sidebar.tsx` | Add sub-choice UI and tmux session picker |
