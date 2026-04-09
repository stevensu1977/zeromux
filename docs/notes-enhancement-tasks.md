# Notes Enhancement — 实现任务分解

> 设计详情见 [notes-enhancement.md](notes-enhancement.md)

---

## Phase 1: 后端基础 — 存储 + CRUD API

最小可用后端，前端暂不改动。完成后可用 curl 验证。

### 1.1 SQLite schema + notes 模块

- [ ] `src/notes.rs` — 新建模块
  - `NoteEntry` 结构体 (id, work_dir, title, created_at, session_id, author, tags, file_path, content)
  - `init_notes_table(db)` — 建表 + 索引
  - `create_note(db, data_dir, work_dir, text, tags, session_id, author)` → NoteEntry
    - 生成 UUID 短 ID
    - 计算 work_dir hash（SHA256 前 8 位）
    - 创建 `{data_dir}/notes/{hash}/` 目录
    - 写入 md 文件（frontmatter + content）
    - 插入 SQLite index
  - `list_notes(db, work_dir)` → Vec\<NoteEntry\>
  - `delete_note(db, data_dir, note_id)` — 删 md 文件 + 删 index 记录
- [ ] `src/db.rs` — 在 `Database::new()` 中调用 `init_notes_table()`
- [ ] `src/main.rs` — 引入 `mod notes`

### 1.2 API handlers

- [ ] `src/web.rs` — 新增路由：
  - `GET /api/sessions/{id}/notes` → 从 session 获取 work_dir，查 SQLite 返回列表
  - `POST /api/sessions/{id}/notes` → 请求体 `{ text, tags? }`，调用 `create_note()`
  - `DELETE /api/sessions/{id}/notes/{note_id}` → 调用 `delete_note()`

### 1.3 清理旧 notes 字段

- [ ] `src/session_manager.rs` — Session 结构体移除 `notes: String`
- [ ] `src/web.rs` — `session_info()` 返回值移除 notes 字段
- [ ] `src/web.rs` — `update_session()` PATCH handler 移除 notes 参数

### Phase 1 验证

```bash
# 创建笔记
curl -X POST /api/sessions/{id}/notes -d '{"text":"test note","tags":["bug"]}'

# 列出笔记
curl /api/sessions/{id}/notes

# 确认 md 文件存在
ls ~/.zeromux/notes/

# 删除笔记
curl -X DELETE /api/sessions/{id}/notes/{note_id}
```

---

## Phase 2: 前端 UI — input + 时间线列表

后端 API 可用后，改造前端 SessionInfoBar。

### 2.1 API 层更新

- [ ] `frontend/src/lib/api.ts`
  - 新增 `NoteEntry` 类型 (id, text, created_at, session_id, author, tags)
  - 新增 `listNotes(sessionId)` → `{ notes: NoteEntry[], work_dir: string }`
  - 新增 `createNote(sessionId, text, tags?)` → NoteEntry
  - 新增 `deleteNote(sessionId, noteId)` → void
  - `SessionInfo` 移除 `notes: string` 字段

### 2.2 SessionInfoBar 改造

- [ ] `frontend/src/components/SessionInfoBar.tsx`
  - 移除 notes textarea + 相关 state (`notes`, `handleNotesBlur`)
  - 新增 notes state: `notes: NoteEntry[]`, `noteInput: string`
  - `useEffect` 加载 `listNotes(session.id)`
  - 输入框：单行 input + Enter 提交（调用 `createNote`）
  - 列表：按 `created_at` 倒序渲染
    - 每条显示：短日期 (`MM-DD HH:mm`) + 内容文本 + tag badges
    - hover 显示 ✕ 删除按钮（调用 `deleteNote`）
  - 同 work_dir 的 session 切换时重新加载 notes

### Phase 2 验证

- [ ] 展开 InfoBar → 输入框输入文字 → Enter → 新条目出现在列表顶部
- [ ] hover 条目 → 出现删除按钮 → 点击 → 条目消失
- [ ] 同 work_dir 开两个 session → 都能看到相同 notes
- [ ] 刷新页面 → notes 仍在

---

## Phase 3: 文件同步 + Reindex

支持用户在外部编辑 md 文件后，索引自动更新。

### 3.1 同步逻辑

- [ ] `src/notes.rs` — 新增 `sync_notes(db, data_dir, work_dir)`
  - 扫描 `{data_dir}/notes/{hash}/*.md`
  - md 存在但 index 无 → 解析 frontmatter + 插入
  - index 有但 md 不在 → 删除 index 记录
  - md 的 mtime 比 index 记录新 → 重新解析更新
- [ ] `src/notes.rs` — 新增 `reindex_all(db, data_dir)` — 扫描所有子目录
- [ ] frontmatter 解析器：简单的 `---` 分隔 + key-value 解析（不引入外部 crate）

### 3.2 触发时机

- [ ] `GET /api/sessions/{id}/notes` — 首次访问某 work_dir 时触发 `sync_notes()`
  - 用内存 cache（HashSet\<work_dir\>）记录已同步的目录，避免每次都扫
- [ ] `POST /api/notes/reindex` — 手动触发全量重建

### 3.3 API

- [ ] `src/web.rs` — 新增路由 `POST /api/notes/reindex`

### Phase 3 验证

- [ ] 手动在 `~/.zeromux/notes/{hash}/` 创建 md 文件 → 访问 API → 新笔记出现
- [ ] 手动删除 md 文件 → reindex → 索引记录消失
- [ ] 删除 `zeromux.db` → reindex → 全部数据恢复

---

## Phase 4: 增强功能（可选）

根据使用反馈决定是否实现。

### 4.1 Tags 过滤

- [ ] `GET /api/sessions/{id}/notes?tag=bug` — 按 tag 筛选
- [ ] 前端 tag 点击过滤

### 4.2 搜索

- [ ] `GET /api/sessions/{id}/notes?q=循环依赖` — 全文搜索（SQLite LIKE）
- [ ] 前端搜索框

### 4.3 导出

- [ ] `GET /api/notes/export?work_dir=...` — 导出某目录所有 notes 为单个 markdown
- [ ] 前端导出按钮

### 4.4 跨项目视图

- [ ] `GET /api/notes?recent=20` — 全局最近笔记（跨所有 work_dir）
- [ ] 前端全局 notes 面板
