# ZeroMux Roadmap

## Product Direction

ZeroMux should become a durable control plane for tmux-backed AI agents.

The operating model is:

- One tmux session maps to one agent workspace.
- **tmux is the source of truth for session existence.** ZeroMux discovers sessions from tmux rather than maintaining a competing persisted copy.
- tmux remains the process runtime.
- SQLite stores durable *control-plane side-data* (recordings index, events, notes) — not session existence.
- Project-local `.zeromux/context/` files remain the portable context surface agents can read directly.

## Current Baseline

ZeroMux already has:

- Browser UI for tmux sessions.
- Claude, Codex, and Kiro session types (ACP).
- Agent event ingestion (`events.rs`, SQLite).
- Context recording backed by tmux `capture-pane` with incremental baseline diffing.
- Context files stored in `{work_dir}/.zeromux/context/`.
- A SQLite index for recording metadata (`recordings.rs`) with interrupted-recording recovery on startup.
- Owner/admin checks on session, context, recording, and download routes (`web.rs`).
- nginx routing for `zeromux.awscode.dev` through CloudFront.

### Verified state vs. earlier roadmap assumptions (2026-06-06)

| Area | Earlier roadmap said | Actual code |
|---|---|---|
| Owner/admin route checks | gap | **Mostly done** — `is_owner` + admin checks in `web.rs` |
| Recording persistence | gap | **Done** — `recordings.rs` SQLite, interrupted recovery |
| Session persistence | gap | **Intentionally not pursued** — tmux is source of truth |
| `/healthz`, `/api/admin/status` | gap | **Not done** |
| systemd / nginx packaging | gap | **Not done** — only `start.sh`/`stop.sh` |

The main remaining gaps are tmux session re-discovery on restart, operational visibility, deployment hardening, and explicit cross-agent workflows.

## P0: Production Stability

Goal: make the current deployment predictable and safe.

Tasks:

- Create and commit a `zeromux.service` systemd template.
- Run ZeroMux with stable runtime arguments:
  - `--host 127.0.0.1`
  - `--port 8400`
  - `--data-dir /home/ubuntu/.zeromux`
- Add nginx WebSocket timeouts for `zeromux.awscode.dev`.
- Confirm CloudFront forwards WebSocket upgrade headers.
- Ensure only nginx/CloudFront exposes ZeroMux publicly.
- Add backup guidance for `/home/ubuntu/.zeromux` and project `.zeromux/context/`.
- Enforce owner/admin checks on all session, file, context, recording, notes, and event routes.

Acceptance criteria:

- `systemctl restart zeromux` restores service without changing data paths.
- `https://zeromux.awscode.dev/auth/mode` works.
- Web terminal sessions stay connected during long idle periods.
- Non-owner users cannot read, delete, download, or record another user's session context.

## P1: tmux Session Re-discovery (no SQLite mirror)

Goal: after a ZeroMux restart, the UI should re-discover the tmux sessions that are still alive — without maintaining a persisted session table that can drift out of sync with tmux.

Design decision (2026-06-06): **We are NOT persisting session existence in SQLite.** tmux already owns session lifecycle; mirroring it in a `sessions.db` introduces two sources of truth and reconciliation bugs. Instead, ZeroMux treats `tmux ls` as authoritative and rebuilds its in-memory view from it on demand.

Tasks:

- On startup (and optionally on a periodic tick), run `tmux list-sessions` and adopt any sessions whose names match ZeroMux's naming convention into the in-memory `SessionManager`.
- Recover only the metadata derivable from tmux itself (tmux name, work_dir via `#{pane_current_path}`, created time). Owner is unknown for adopted sessions — see open question below.
- Keep all session state in memory (`Mutex<HashMap>`); SQLite stays reserved for recordings/events/notes side-data.
- Sessions that disappear from `tmux ls` simply stop appearing in the UI — no `missing`/`archived` bookkeeping needed.

Open question to resolve before implementing:

- Adopted sessions have no `owner_id` (it was only in memory). Options: (a) attribute adopted sessions to admin only, (b) encode owner into the tmux session name, (c) keep a *thin* owner-only mapping in SQLite (session_name → owner_id) — the one piece worth persisting, since it cannot be recovered from tmux.

Acceptance criteria:

- Create a tmux agent session, restart ZeroMux, and the still-running session reappears in the UI.
- No duplicate records are created when re-adopting an existing tmux session.
- A killed tmux session is absent from the UI after the next discovery pass.

## P2: Recording Reliability

Goal: recorded context should be durable, searchable, and understandable after restarts.

Tasks:

- Keep `.log` files as the raw durable artifact.
- Keep SQLite as the index and status tracker.
- Track recording states:
  - `recording`
  - `completed`
  - `interrupted`
  - `deleted`
- On startup, mark in-progress recordings as `interrupted`.
- Sync existing `.zeromux/context/*.log` files into SQLite on list.
- Add filters by session, status, work directory, and time.
- Add record metadata:
  - line count
  - byte size
  - start time
  - stop time
  - source tmux session
- Decide whether interrupted recordings should be manually resumable or only visible as partial records.

Recommended design choice:

- Do not auto-resume an interrupted recording.
- Make the partial log visible.
- Let the user start a new recording explicitly.

Acceptance criteria:

- Completed recordings remain visible after service restart.
- Already-existing log files are indexed automatically.
- Interrupted recordings are visible and clearly labeled.
- Deleting a recording removes both the file and SQLite row.

## P3: Better Capture Semantics

Goal: reduce incorrect chunks and make logs useful to agents.

Tasks:

- Keep using `tmux capture-pane`, not `pipe-pane`, for readable rendered output.
- Use `capture-pane -p -J -S -` for full history and joined wrapped lines.
- Configure tmux `history-limit` high enough for real sessions.
- Replace fragile baseline-diff logic with a more deterministic approach.

Options:

- Simple mode: on stop, write full final pane history.
- Delta mode: record start and stop history positions if tmux can expose a stable offset.
- Snapshot mode: periodically capture full screen/history and compute robust diffs offline.

Recommendation:

- Use simple mode first if exact start-to-stop boundaries are less important than readable logs.
- Add delta mode only after proving a reliable tmux history cursor strategy.

Acceptance criteria:

- Claude Code status/progress redraws no longer produce raw ANSI noise.
- OSC hyperlinks do not appear as broken byte chunks in the viewed content.
- Long terminal-wrapped lines are readable.

## P4: Cross-Agent Context Workflow

Goal: make Claude and Codex hand off useful state without relying on memory or manual copy/paste.

Tasks:

- Add a "Send to Agent" workflow from:
  - recordings
  - saved context
  - agent events
  - notes
- Store cross-agent messages in SQLite:
  - `id`
  - `source_session_id`
  - `target_session_id`
  - `work_dir`
  - `title`
  - `body`
  - `status`
  - `created_at`
  - `read_at`
- Write selected messages into `.zeromux/context/` for direct agent reading.
- Add UI states:
  - `new`
  - `sent`
  - `read`
  - `resolved`
- Keep delivery user-controlled before adding automation.

Acceptance criteria:

- User can send context from one tmux agent session to another.
- Target agent has a clear file path to read.
- The UI shows whether a handoff was created and resolved.

## P5: Agent Startup Context

Goal: new or resumed agents should start with project-relevant durable context.

Tasks:

- Add optional startup context injection for tmux sessions.
- Generate a short context preamble from:
  - today's context file
  - recent unresolved handoffs
  - active notes
  - last recording summaries
- Let the user choose:
  - no injection
  - paste context into terminal
  - write context file and show path
  - use agent-specific resume command
- Track whether context was injected.

Acceptance criteria:

- A new Codex/Claude tmux session can receive current project context without manual file lookup.
- Resume workflows remain explicit and do not accidentally pollute unrelated sessions.

## P6: Observability And Admin

Goal: make operations debuggable from the UI.

Tasks:

- Add `/healthz`.
- Add `/api/admin/status` with:
  - app version
  - data directory
  - tmux availability
  - active tmux sessions
  - database paths
  - recording counts
  - uptime
- Add structured logs for:
  - session create/delete/attach
  - record start/stop/interrupted
  - context read/delete/download
  - auth failures
- Add an admin cleanup view for:
  - old context files
  - orphaned recording rows
  - missing tmux sessions

Acceptance criteria:

- Admin can diagnose service, tmux, DB, and recording state without SSH.
- Cleanup actions are explicit and auditable.

## P7: Packaging

Goal: make ZeroMux easy to redeploy on a new server.

Tasks:

- Add systemd unit template.
- Add nginx snippet for WebSocket proxying.
- Add CloudFront checklist.
- Add backup and restore docs.
- Add release build script.
- Add upgrade notes for DB schema changes.

Acceptance criteria:

- A fresh machine can be configured from docs in under 30 minutes.
- Restart, backup, restore, and upgrade paths are documented.

## Recommended Milestone Order (revised 2026-06-06)

Rationale: the most user-visible gap is that a restart appears to "lose" all sessions even though tmux is still running them. That is addressed by lightweight re-discovery (P1), not by a persisted session model. Observability (P6) is cheap and high-value, so it is pulled forward ahead of deployment hardening — there is no point hardening the deploy of a service you cannot yet inspect.

1. tmux Session Re-discovery (P1) — biggest perceived data-loss gap, reuses no new storage.
2. Observability: `/healthz` + `/api/admin/status` (subset of P6) — ~half a day, immediate remote diagnosis.
3. Recording Reliability polish (P2) — filters + interrupted visibility; storage already exists.
4. Production Stability + Packaging (P0 + P7) — systemd, nginx timeouts, backup/restore docs.
5. Cross-Agent Context Workflow (P4) — "Send to Agent" handoffs.
6. Agent Startup Context (P5) — context preamble injection for new/resumed sessions.
7. Better Capture Semantics (P3) — `-J` joined lines, deterministic capture.

P4/P5 are deliberately later: cross-session collaboration is the product's headline, but it stands on session re-discovery and operational visibility being solid first.

## Near-Term Implementation Plan

Next 1-2 days:

- Implement tmux re-discovery: adopt matching `tmux list-sessions` entries into `SessionManager` on startup.
- Decide the adopted-session owner strategy (admin-only / name-encoded / thin owner map).
- Add `/healthz` and `/api/admin/status` (version, data dir, tmux availability, active session count, DB paths, recording counts, uptime).

Next 1 week:

- Add interrupted-recording visibility in the UI.
- Add basic recording filters (session, status, work dir, time).
- Add structured logs for session create/delete/attach and record start/stop.

Next 2-4 weeks:

- Add systemd unit template + nginx WebSocket timeout snippet + backup/restore docs.
- Add cross-agent handoff records and "Send to Agent" UI.
- Add startup context workflow.

## Non-Goals For Now

- Do not build a complex autonomous context router until manual handoff works well.
- Do not rely on `pipe-pane` for readable logs.
- Do not make ZeroMux replace tmux as the runtime.
- Do not hide raw log files behind SQLite; SQLite should index files, not become the primary blob store.
