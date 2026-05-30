# Agent Dashboard — Multi-Agent Activity Board

## Overview

ZeroMux needs a unified dashboard to track activity from multiple AI agents (Claude Code, Codex, Kiro, etc.) running in tmux sessions. The key challenge is that each agent has a different hook/plugin mechanism, so we decouple **data ingestion** from **agent-specific adapters**.

## Architecture

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│ Claude Code │  │   Codex     │  │    Kiro     │
│  (hooks)    │  │  (TBD)      │  │  (TBD)     │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘
       │ curl/POST       │                │
       ▼                 ▼                ▼
┌─────────────────────────────────────────────────┐
│  ZeroMux: POST /api/events  (generic ingest)    │
│  Schema: {agent, event, summary, ...}           │
└─────────────────────────┬───────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────┐
│  SQLite: agent_events table                     │
└─────────────────────────┬───────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────┐
│  GET /api/events — Dashboard UI                 │
└─────────────────────────────────────────────────┘
```

## Design Principles

1. **Ingest API 极简** — 任何 agent 只要能执行 `curl -X POST` 就能上报事件
2. **标准化 schema, 不标准化 hook** — 适配器是各 agent 独立的薄壳脚本
3. **Session 关联可选** — 事件可以关联 zeromux tmux session，也可以独立存在
4. **认证** — 使用同一套 token 认证（query param `?token=xxx`）

## Event Schema

```json
{
  "agent": "claude-code",         // agent 标识: claude-code | codex | kiro | custom
  "event": "task_done",           // 事件类型 (见下方枚举)
  "summary": "Implemented login page",  // 人类可读摘要
  "session_id": "uuid-optional",  // 关联的 zeromux session (可选)
  "work_dir": "/home/ubuntu/project",   // 工作目录 (可选)
  "metadata": {                   // 自由格式附加数据 (可选)
    "duration_sec": 120,
    "files_changed": ["src/login.rs"],
    "tool_name": "Edit",
    "cost": "$0.15"
  }
}
```

### Event Types

| event | 含义 | 触发时机 |
|-------|------|----------|
| `task_start` | Agent 开始处理任务 | hook: user message submit |
| `tool_use` | Agent 调用了工具 | hook: PostToolUse |
| `task_done` | Agent 完成任务 | hook: Stop / agent idle |
| `error` | Agent 报错 | hook: error 事件 |
| `milestone` | 关键进展 | 由 agent 自行输出 |
| `custom` | 自定义事件 | 适配器自行定义 |

## API Design

### POST /api/events

上报事件。需要 token 认证。

**Request:**
```bash
curl -X POST "https://zeromux.example.com/api/events?token=xxx" \
  -H "Content-Type: application/json" \
  -d '{
    "agent": "claude-code",
    "event": "task_done",
    "summary": "Added user authentication",
    "session_id": "12f21a0f-...",
    "metadata": {"files_changed": ["src/auth.rs"]}
  }'
```

**Response:** `201 Created`
```json
{ "id": "evt_abc123", "timestamp": "2026-05-30T10:00:00Z" }
```

### GET /api/events

查询事件列表。

**Query params:**
- `session_id` — 按 session 过滤
- `agent` — 按 agent 类型过滤
- `event` — 按事件类型过滤
- `since` — ISO 时间戳，返回此时间之后的事件
- `limit` — 最大返回数量 (default: 50, max: 500)

**Response:**
```json
{
  "events": [
    {
      "id": "evt_abc123",
      "agent": "claude-code",
      "event": "task_done",
      "summary": "Added user authentication",
      "session_id": "12f21a0f-...",
      "work_dir": "/home/ubuntu/zeromux",
      "metadata": {"files_changed": ["src/auth.rs"]},
      "timestamp": "2026-05-30T10:00:00Z"
    }
  ],
  "total": 1
}
```

### DELETE /api/events/{id}

删除单个事件。

### DELETE /api/events

清除事件。Query params: `before` (时间戳) 或 `session_id`。

## Storage

SQLite table `agent_events`:

```sql
CREATE TABLE agent_events (
  id TEXT PRIMARY KEY,
  agent TEXT NOT NULL,
  event TEXT NOT NULL,
  summary TEXT NOT NULL DEFAULT '',
  session_id TEXT,
  work_dir TEXT,
  metadata TEXT,  -- JSON string
  timestamp TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_events_session ON agent_events(session_id);
CREATE INDEX idx_events_timestamp ON agent_events(timestamp);
CREATE INDEX idx_events_agent ON agent_events(agent);
```

## Claude Code Hook Adapter

配置 `.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [{
          "type": "command",
          "command": "/home/ubuntu/.local/bin/zeromux-hook stop"
        }]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "",
        "hooks": [{
          "type": "command",
          "command": "/home/ubuntu/.local/bin/zeromux-hook tool_use"
        }]
      }
    ]
  }
}
```

`zeromux-hook` 脚本 (bash):

```bash
#!/bin/bash
# /home/ubuntu/.local/bin/zeromux-hook
# Reads Claude Code hook environment and posts to ZeroMux

ZEROMUX_URL="${ZEROMUX_URL:-http://127.0.0.1:8400}"
ZEROMUX_TOKEN="${ZEROMUX_TOKEN:-H8xO67hOuaG2hZ1w}"
EVENT_TYPE="${1:-custom}"

# Claude Code hooks provide context via stdin (JSON)
INPUT=$(cat)

# Extract useful fields
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')
SUMMARY=$(echo "$INPUT" | jq -r '.output // .stop_reason // "agent event"' | head -c 200)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
WORK_DIR=$(pwd)

# Map hook type to event
case "$EVENT_TYPE" in
  stop)
    EVENT="task_done"
    SUMMARY=$(echo "$INPUT" | jq -r '.stop_reason // "Task completed"' | head -c 200)
    ;;
  tool_use)
    EVENT="tool_use"
    SUMMARY="Used tool: ${TOOL_NAME}"
    ;;
  *)
    EVENT="custom"
    ;;
esac

curl -s -X POST "${ZEROMUX_URL}/api/events?token=${ZEROMUX_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "$(jq -n \
    --arg agent "claude-code" \
    --arg event "$EVENT" \
    --arg summary "$SUMMARY" \
    --arg session_id "$SESSION_ID" \
    --arg work_dir "$WORK_DIR" \
    --arg tool_name "$TOOL_NAME" \
    '{agent: $agent, event: $event, summary: $summary, session_id: $session_id, work_dir: $work_dir, metadata: {tool_name: $tool_name}}'
  )" > /dev/null 2>&1 &
```

## Dashboard UI

新增一个面板入口（类似 Files / Git），可在 SessionInfoBar 或独立页面展示：

### 视图模式

1. **Timeline 视图** — 按时间倒序显示所有事件，带 agent 图标 + 颜色区分
2. **Session 视图** — 按 session 分组，显示该 session 内的 agent 活动
3. **Summary 视图** — 聚合统计：各 agent 的 task_done 数量、活跃时间、工具使用分布

### UI 组件

- `AgentDashboard.tsx` — 主面板
- 每条事件显示：时间 | agent icon | event badge | summary
- 过滤器：agent 类型、事件类型、时间范围
- 实时更新：轮询 GET /api/events?since=last_timestamp (每 10s)

## Implementation Plan

### Phase 1: Backend (MVP)
1. 创建 `src/events.rs` — event CRUD handlers
2. 扩展 SQLite schema (在现有 db.rs 中添加 agent_events 表)
3. 注册 API routes: POST/GET/DELETE /api/events
4. 认证: 复用现有 token 机制 (events 路由放在 auth middleware 下)

### Phase 2: Claude Code Adapter
1. 编写 `zeromux-hook` bash 脚本
2. 配置 Claude Code hooks
3. 测试 Stop + PostToolUse 事件上报

### Phase 3: Dashboard UI
1. `AgentDashboard.tsx` — timeline 视图
2. SessionInfoBar 添加看板入口按钮
3. 实时轮询更新

### Phase 4: Multi-Agent Expansion
1. Codex adapter (待 Codex 稳定后)
2. Kiro adapter (待 Kiro hook 机制确定后)
3. 通用 adapter 模板/文档

## Future Considerations

- WebSocket 推送 events (替代轮询)
- Event 聚合/摘要 (自动用 LLM 总结一天的活动)
- 成本追踪 (从 metadata 中提取 token 用量)
- 跨 session 关联 (同一个任务跨多个 session)
