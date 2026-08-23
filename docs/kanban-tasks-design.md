# ZeroMux Tasks / Kanban 设计

> 定稿:2026-08-23。上位文档:[roadmap-2026-08-execution-layers.md](roadmap-2026-08-execution-layers.md)
> 定位判据:tasks-as-a-service 是"调度"电池 —— "任务丢进去 → agent 领走"
> 这条路径的极致缩短。不是控制:无 daemon 派工、无 DAG、无 agent 身份体系。

## 调研结论(为什么这么设计)

| 项目 | 吸收 | 拒绝 |
|---|---|---|
| saltbo/agent-kanban | 角色/评审流参考 | daemon 拉起 agent、Ed25519 身份 —— 是第二个控制平面 |
| abpai/agent-kanban | "Bash is the API":CLI 输出结构化 JSON | 独立单品,接入要打通三条线 |
| appsoftware vscode-agent-kanban | markdown+frontmatter 存储、sentinel 防上下文衰减、agentkanban.io 的 remote board 形态(API key 接入) | board.yaml(V1 列固定,无处安放) |
| LachyFS kanban-markdown | 文件格式直接参考(YAML frontmatter + 正文)、skill 分发(`npx skills add`) | — |
| KiroCrew taskrunner | task=意图 vs run=执行 的分层验证;crash 时 running→paused 诚实降级;force_approval 门 | 步骤级 DAG/replan/checkpoint —— 那是执行器(agent 侧)的地盘 |

## 核心模型

**Task 是实体(意图),Kanban 是投影(视图)。**

- Task = 一个工作单元:标题、描述、验收标准、状态机、时间线。
  存在于文件,不依赖看板 UI。
- Kanban = 按 status 分列的渲染 + 拖拽改 status。board 无实体:
  **project = board = work_dir**,不建 boards 表、不建 board.yaml。
- **Task(意图)1:N 执行(execution)**:一张卡可被多次执行(失败重试、
  换 agent)。frontmatter 的 `session`/`runner` 指向当前执行;
  完整执行历史在卡片正文时间线(append-only)。
- 卡片间依赖不建模(一句话写正文);步骤级 DAG 归执行器
  (KiroCrew taskrunner、agent 自己的 planner)。
  **分工:板管 what/who/status,runner 管 how/order。**

## 存储:markdown 文件为 canonical

延续 repo 既有原则("SQLite 只做文件索引,不做主存储"—— recordings 同款):

```
{work_dir}/.zeromux/tasks/
  fix-login-bug.md
  README.md            # 任务板约定(canonical 说明书)
```

```markdown
---
id: fix-login-bug
status: in-progress   # backlog | todo | in-progress | review | done
                      # + blocked(游离态,UI 标红)
assignee: codex       # 人名 | claude | codex | kiro |
                      # microvm:<task-id> | agentcore:<session>
session: <zeromux session id>    # 当前执行的会话,可空
runner: kirocrew:<task-id>       # 可选:执行器指针(不镜像其内部状态)
branch: kirocrew/task/xxx        # 可选:产出分支,链 GitViewer diff
needs_approval: false            # true:agent 不得自行移到 done
labels: [bug]
order: 2
---
# Fix login bug

描述、验收标准。

## Timeline
- 2026-08-23T10:00Z claude@sess-a claimed
- 2026-08-23T10:40Z claude@sess-a note: 复现了,是 cookie 域问题
```

status 是**协作状态机**(谁该看它了),不是进度条 —— agent 的细粒度进度
进时间线,防止三个 agent 的习惯把 status 集合撑爆。

git 版本化是 remote 的天然接口:MicroVM clone repo 即拿到板,
push 分支时卡片状态随代码回来(frontmatter diff/merge 友好)。

## 访问:三界面收敛于同一份文件

```
界面 1: 文件      agent 用原生 Read/Edit,零集成(本地)
界面 2: CLI       zmux-task(本地打 localhost API;
                  ZEROMUX_TASK_URL/TOKEN 存在时自动切远程,命令不变)
界面 3: HTTP API  /api/tasks/…(remote agent 唯一通道;
                  实现 = ZeroMux 代写同一批文件)
```

- **CLI 是三 agent(Claude/Codex/Kiro)的最大公约数**,输出结构化 JSON:
  `zmux-task list --status todo` / `claim t_x` / `move t_x review --note "…"` / `show t_x`
- **claim 必须原子**(唯一真实竞争):API 层 compare-and-set
  (assignee 为空才允许),这是 CLI 本地也走 API 而非直编文件的原因。
  文件直编路径靠约定 + 索引器冲突检测(双 assignee 标红,人仲裁)。
- **归因**:本地会话创建时注入 `ZEROMUX_SESSION_ID` env;
  远程 task token 签发时写入 actor;文件直编兜底 git blame。

## 分发:skill 为主

`zeromux-tasks` skill(SKILL.md + `scripts/zmux-task` 打包在内):
- 装了 skill 就有 CLI,不装 PATH;MicroVM 镜像烘焙 skill 即可
- 渐进加载:description 一行常驻,任务相关时才载全文(对抗 context decay)
- 说明书与工具同版本演进
- Kiro 若不认 skill:steering 目录放精简说明指向 `.zeromux/tasks/README.md`(兜底)
- AgentCore 通道:bridge 已有 `_meta.claudeCode.options` skill 注入机制

## Remote agent(为 Agent Team 预埋)

- **Task API 一等公民**:CRUD / move / claim(原子)/ 通用 query
  (`?work_dir=&status=&assignee=` —— 看板、列表、agent 领活队列同一端点)
- **Task-scoped token**:remote agent 只能操作被指派的卡/项目;
  签发复用 proxy JWT 模式(slug→task_id);任务合同注入
  `task_id + task_token + repo`,每个动作可归因
- 同步语义:文件 canonical,API 写 = 代写文件 + bump 索引;
  MicroVM push 回的 frontmatter 变更走 git merge;
  极端冲突 last-write-wins + 时间线留痕,不做 CRDT

## 里程碑

- **M1 地基**:文件格式定型、索引器(SQLite 缓存,模式同 recordings)、
  Task API(含 claim 原子性 + task token)、zmux-task CLI、
  zeromux-tasks skill、会话 env 注入
- **M2 看板 UI**:项目切换(=work_dir)、五列拖拽(=改 frontmatter)、
  卡片详情(正文 + events 时间线)、会话/分支跳转
- **M3 Hook 深化**:`.zeromux/current-task` 注入、stop hook 自动移列
  (仅 hook 能力够的 agent,做不到的靠指令纪律)、
  stale in-progress 检测(ZeroMux 重启后无活跃会话的卡提示人工确认)
- **M4(挂 MicroVM 主线)**:任务合同带卡 —— 领卡 → 分支 → push →
  卡片自动进 review

## 明确不做

daemon 派工 / 任务级 DAG 与循环检测 / agent 身份密钥 / 多板 /
自定义列(V2 再议,预留 `.zeromux/tasks/.board.yaml`)/ CRDT 同步 /
甘特图与报表。判据:不让"任务丢进去 → agent 领走"更快的,都不做。
