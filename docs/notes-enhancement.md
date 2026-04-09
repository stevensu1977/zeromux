# Notes Enhancement: Markdown + SQLite 集中式方案

## 目标

将 session notes 从单条内存字符串升级为持久化的时间线日志，支持多条笔记、自动时间戳、跨 session 继承。

## 设计原则

- **Markdown 是 source of truth** — 删 db 不丢数据，可重建索引
- **SQLite 是索引** — 加速列表、搜索、过滤
- **集中存储** — 不污染项目目录，Docker 友好
- **人可读可编辑** — 用户可直接修改 md 文件

## 存储结构

```
~/.zeromux/                          ← data-dir（已有）
├── zeromux.db                       ← 统一 SQLite（已有）
└── notes/
    ├── {work_dir_hash_8}/           ← 按 work_dir 的 SHA256 前8位隔离
    │   ├── 20260407_143200_a1b2.md
    │   ├── 20260407_150500_c3d4.md
    │   └── 20260407_161000_e5f6.md
    └── {another_hash}/
        └── ...
```

### 文件名格式

```
{YYYYMMDD}_{HHMMSS}_{id_short_4}.md
```

### Markdown 文件格式

```markdown
---
id: a1b2c3d4
created: 2026-04-07T14:32:00+08:00
work_dir: /home/ubuntu/my-project
session_id: sess_xxx
author: steven
tags: [bug, auth]
---

发现 auth middleware 有循环依赖，需要拆分公共模块。
```

## SQLite Schema

在已有的 `zeromux.db` 中新增表：

```sql
CREATE TABLE IF NOT EXISTS notes (
    id          TEXT PRIMARY KEY,
    work_dir    TEXT NOT NULL,
    title       TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    session_id  TEXT,
    author      TEXT,
    tags        TEXT,              -- JSON array, e.g. '["bug","auth"]'
    file_path   TEXT NOT NULL,     -- 相对于 ~/.zeromux/notes/ 的路径
    content     TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notes_workdir ON notes(work_dir);
CREATE INDEX IF NOT EXISTS idx_notes_created ON notes(created_at DESC);
```

## API

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/sessions/{id}/notes` | 按 session 的 work_dir 查询所有 notes |
| POST | `/api/sessions/{id}/notes` | 创建笔记（写 md 文件 + 插入 index） |
| DELETE | `/api/sessions/{id}/notes/{note_id}` | 删除笔记（删 md 文件 + 删 index） |
| POST | `/api/notes/reindex` | 从 md 文件重建全部索引 |

### POST 请求体

```json
{
  "text": "发现循环依赖",
  "tags": ["bug"]
}
```

### GET 响应

```json
{
  "notes": [
    {
      "id": "a1b2c3d4",
      "text": "发现循环依赖",
      "created_at": "2026-04-07T14:32:00+08:00",
      "session_id": "sess_xxx",
      "author": "steven",
      "tags": ["bug"]
    }
  ],
  "work_dir": "/home/ubuntu/my-project"
}
```

## 文件同步策略

首次访问某 work_dir 的 notes 时：

1. 扫描 `~/.zeromux/notes/{hash}/` 下所有 `*.md`
2. 对比 index：md 有但 index 无 → 解析 frontmatter 插入
3. 对比 index：index 有但 md 无 → 删除索引记录
4. md 的 mtime 比 index 记录新 → 重新解析更新

## 前端 UI

SessionInfoBar 展开后的 Notes 区域改为：

```
┌─ Notes ──────────────────────────────────────┐
│ [输入框: Add a note...]              [Enter] │
│                                              │
│ 04-07 14:32  发现循环依赖           [bug] ✕  │
│ 04-07 11:05  开始拆分 payment.rs          ✕  │
│ 04-07 10:30  会话已创建                   ✕  │
└──────────────────────────────────────────────┘
```

- 单行 input + Enter 快速添加（替代 textarea）
- 按时间倒序，最新在上
- 显示短日期 + 内容摘要 + tags 标签
- hover 显示删除按钮
- 同一 work_dir 的所有 session 共享同一组 notes

## 兼容性

- 移除 `PATCH /api/sessions/{id}` 中的 `notes` 字段
- `SessionInfo` 中的 `notes: String` 移除
- `description` 和 `status` 的 PATCH 不变
- 无 OAuth 模式（legacy）下 author 为 "admin"

## 实现任务

### 后端

- [ ] `zeromux.db` 新增 `notes` 表
- [ ] 新增 `src/notes.rs` — NoteEntry 结构体、CRUD 函数、文件同步
- [ ] `src/web.rs` — 新增 3 个 API handler (GET/POST/DELETE)
- [ ] `src/session_manager.rs` — Session 结构体移除 `notes: String`
- [ ] `src/web.rs` — PATCH handler 移除 notes 字段

### 前端

- [ ] `src/lib/api.ts` — NoteEntry 类型、新增 API 函数、SessionInfo 移除 notes
- [ ] `src/components/SessionInfoBar.tsx` — textarea 改为 input + list
- [ ] 同一 work_dir 的 session 切换时 notes 自动刷新

### 验证

- [ ] 创建笔记 → md 文件写入 + index 更新
- [ ] 删除笔记 → md 文件删除 + index 清理
- [ ] 同 work_dir 不同 session 看到相同 notes
- [ ] 外部编辑 md 文件 → reindex 后可见
- [ ] 删除 index.db → reindex 恢复全部数据
- [ ] Docker 部署：单 volume mount `~/.zeromux` 即可
