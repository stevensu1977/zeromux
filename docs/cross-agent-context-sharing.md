# Cross-Agent Context Sharing

## Overview

ZeroMux provides two mechanisms for cross-agent context sharing:

1. **Hook-based event capture** — automatic task_done events from Claude Code / Codex
2. **Context Recorder** — user-controlled tmux output recording via `pipe-pane`

## Architecture

```
┌─────────────┐    Stop hook     ┌───────────────┐    POST /api/events
│ Claude Code │ ───────────────► │ zeromux-hook  │ ─────────────────────► ZeroMux DB
└─────────────┘                  └───────────────┘                        (events.db)
                                        │
┌─────────────┐    Stop hook            │
│   Codex     │ ────────────────────────┘
└─────────────┘

                   ┌──────────────────────────────────────────────┐
                   │          ZeroMux Web UI                       │
                   │                                               │
                   │  [Context tab] → Record / Stop                │
                   │         │                                     │
                   │         ▼  tmux pipe-pane -o                  │
                   │  .zeromux/context/{name}-{timestamp}.log      │
                   │         │                                     │
                   │         ▼                                     │
                   │  Agent B reads file on demand                 │
                   └──────────────────────────────────────────────┘
```

## Components

### 1. Hook Script (`~/.local/bin/zeromux-hook`)

**Trigger**: Agent's Stop event (task completion)

**Input** (JSON on stdin):
| Field | Source | Usage |
|-------|--------|-------|
| `cwd` | Agent runtime | → `git rev-parse --show-toplevel` → `work_dir` |
| `session_id` | Agent | Stored but not used for filtering |
| `last_assistant_message` | Agent | Full final response → stored as `metadata.detail` |
| `transcript_path` | Agent | Agent detection (`.claude` → claude-code, `.codex` → codex) |
| `hook_event_name` | Agent | Fallback detection |
| `model` | Agent | Secondary fallback for agent identification |

**Agent Detection Logic**:
```
transcript_path contains ".claude"  → claude-code
transcript_path contains "codex"    → codex
hook_event_name present + model check → fallback
```

**Output**: POST to `/api/events?token=xxx` with:
```json
{
  "agent": "claude-code",
  "event": "task_done",
  "summary": "<first 200 chars of last_assistant_message>",
  "work_dir": "/home/ubuntu/project",
  "metadata": { "detail": "<full last_assistant_message>" }
}
```

### 2. Agent Dashboard (Frontend)

- Displays events from all agents in timeline view
- Filters by agent type and event type
- Single-click to expand → shows full markdown detail
- **"Save to Context"** button (BookmarkPlus icon) → calls `POST /api/context`

### 3. Context File Storage (`POST /api/context`)

**Request**:
```json
{
  "work_dir": "/home/ubuntu/project",
  "content": "<markdown content>",
  "title": "Optional title"
}
```

**Behavior**:
- Creates `{work_dir}/.zeromux/context/` directory
- Appends to today's file: `{work_dir}/.zeromux/context/2026-06-05.md`
- Format per entry:
  ```markdown
  ## Title (HH:MM)

  <content>

  ---
  ```

**Security**: Path must be under `$HOME` (canonicalized check)

### 4. Hook Configuration

**Claude Code** (`~/.claude/settings.json`):
```json
{
  "hooks": {
    "Stop": [{
      "matcher": "",
      "hooks": [{ "type": "command", "command": "/home/ubuntu/.local/bin/zeromux-hook stop" }]
    }]
  }
}
```

**Codex** (`~/.codex/hooks.json`):
```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{ "type": "command", "command": "/home/ubuntu/.local/bin/zeromux-hook stop" }]
    }]
  }
}
```

## Usage Workflow

### Passing context from Agent A → Agent B

1. Agent A completes a task → hook fires → event stored in ZeroMux
2. User opens Agent Dashboard in ZeroMux UI
3. User clicks on the event to expand detail
4. User clicks "Save to Context" → written to `.zeromux/context/{date}.md`
5. User tells Agent B: `read .zeromux/context/2026-06-05.md for context`
6. Agent B reads the file and uses the information

### Automatic (no UI interaction)

Currently there is no automatic push of context to other agents. The flow requires manual "Save to Context" via the UI. Possible enhancements below.

## Limitations & Trade-offs

| Aspect | Current State | Trade-off |
|--------|---------------|-----------|
| **Trigger** | Only `Stop` event | Misses mid-task context; keeps volume low |
| **Content** | Full `last_assistant_message` | Can be very long; agent must judge relevance |
| **Agent detection** | Heuristic (transcript_path grep) | May fail for future agents |
| **File growth** | One file per day, append-only | Could grow unbounded on busy days |
| **Discovery** | Agent must be told to read the file | No auto-injection into agent context |
| **Dedup** | None | Same context can be saved multiple times |
| **Scope** | Per-project (work_dir based) | No cross-project context sharing |

## Potential Enhancements

### P0 — Immediate value
- **Auto-save on Stop**: Hook writes directly to `.zeromux/context/` without UI step
- **Summary extraction**: Save a compressed summary (first 500 chars + key decisions) instead of full message

### P1 — Better discovery
- **CLAUDE.md injection**: Add a line to project's CLAUDE.md: `"Check .zeromux/context/ for cross-agent context before starting"`
- **Pre-start hook**: On agent start, cat today's context file into initial prompt

### P2 — Smarter context
- **Token budget**: Truncate/summarize if day file exceeds N tokens
- **Tag-based routing**: Tag context entries (e.g., "architecture", "bug-fix") and let agents filter
- **Structured format**: JSON entries instead of markdown for machine parsing

### P3 — Full automation
- **Bidirectional sync**: Agent registers interest in topics, context pushed automatically
- **Context broker**: ZeroMux acts as MCP server, agents query context via tool call
- **Embedding search**: Semantic search over context history

## File Layout Example

```
/home/ubuntu/my-project/
├── .zeromux/
│   └── context/
│       ├── 2026-06-04.md    ← yesterday's context
│       └── 2026-06-05.md    ← today's context
├── src/
└── ...
```

Content of `2026-06-05.md`:
```markdown
## Refactored auth module (09:32)

Moved session validation from middleware to a dedicated `AuthService` struct.
Key change: tokens are now validated against Redis instead of in-memory cache.
Breaking: `validate_session()` is now async.

---

## Fixed race condition in event store (14:15)

The SQLite writer was not using WAL mode, causing SQLITE_BUSY under concurrent
hook POSTs. Fixed by adding `PRAGMA journal_mode=WAL` at connection init.

---
```
