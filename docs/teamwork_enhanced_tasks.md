# ZeroMux Roadmap

> AI 团队的 tmux — 一个人（或小团队）开多个 AI Agent，统一调度、统一上下文、统一查看产出。

**Target Users:** One Person Company / AI Native 小团队 (2-4人)

---

## P0: Agent 自治 — "开了就不用管"

- [ ] **启动 Prompt** — 创建 Agent 会话时填入初始指令，Agent 自动开始工作
- [ ] **完成检测** — 检测 Agent 输出 `result` 事件，自动标记会话状态为 Done
- [ ] **浏览器通知** — Agent 完成时发送 Browser Notification + sidebar 闪烁提示
- [ ] **产出摘要** — Agent 结束时，最后一条消息作为 summary 显示在 sidebar

## P1: 共享上下文 — "Agent 自带记忆"

- [ ] **Workspace Context** — `.zeromux/context.md`，创建 Agent 时自动注入为 system prompt
- [ ] **Agent 模板** — 预设角色（Coder / Reviewer / Wiki Editor / Research），一键创建
- [ ] **会话产出持久化** — Agent 对话结束后，关键产出自动写入 work_dir

## P2: 多人协同 — "队友看得到"

- [ ] **共享 Session 列表** — 多用户场景下，所有人可查看所有 Agent 状态
- [ ] **Activity Feed** — 事件流："Alice 的 Agent 完成了 PR #42 的 review"
- [ ] **会话移交** — 把一个 Agent 会话转给队友继续

## P3: 知识积累 — Karpathy 模式

- [ ] **Wiki Agent 模板** — 预设 "读 raw/ → 更新 wiki/" 工作流
- [ ] **Lint Agent 模板** — 预设 "检查 wiki/ 一致性" 工作流
- [ ] **定时触发** — Cron 式调度，定期运行 Compile / Lint Agent

---

## 不做

| 不做 | 原因 |
|------|------|
| 实时协同编辑 (CRDT) | 复杂度太高，2-4人不需要 |
| 向量数据库 / RAG | 小规模 markdown + LLM 上下文窗口够用 |
| 内置 Obsidian | 用户用自己喜欢的编辑器 |
| 细粒度权限 (RBAC) | 小团队信任为主，admin/member 够了 |
| 计费 / Quota | one person company 不需要 |
