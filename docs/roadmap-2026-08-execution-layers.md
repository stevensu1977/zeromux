# ZeroMux Roadmap — 执行层扩展与 Agent Team(2026-08)

> 讨论日期:2026-08-14
> 本文档记录战略层 roadmap(执行层扩展、Agent Team、远程 backend)。
> 运维/稳定性向的 roadmap 见 [roadmap.md](roadmap.md)(2026-06,大部分仍有效)。

## 一、定位回顾(不变)

ZeroMux 是 **Agent 编排控制平面** —— 不做 agent 智能,只管 Agent I/O、
会话生命周期、用户隔离。任务拆解与执行由 agent(Claude Code / Codex / Kiro)
自己完成;ZeroMux 提供管道、看板与产出物通道。

- 目标用户:小团队(2-5 人),共享实例 + 用户隔离
- 工作流:手动编排 —— 人创建会话、下指令、自己看结果
- P0(终端 mux + ACP + worktree + 日志)、P1(OAuth + 用户体系)已完成

## 二、核心架构演进:三层执行层

本次讨论确定的最大方向:ZeroMux 从"本地 tmux 控制台"演进为
**多执行层的统一控制平面**。

```
ZeroMux(常驻控制平面:人机交互、会话/任务管理、产出物通道)
 ├─ 本地执行层:tmux 会话(现状)
 │     交互式、人在回路、长生命周期、文件系统直达
 ├─ 远程执行层 A:AgentCore Runtime(计划)
 │     交互式远程会话、托管 microVM、SSE 数据面、8h 会话上限
 └─ 远程执行层 B:Lambda MicroVM(计划,即 "Agent Team")
       无人值守任务、YOLO 模式、跑完即回收、挂起计费 + 快照续跑
```

关键抽象:**"remote ACP backend" 接口**(会话创建 / prompt / 事件流 / 终止)。
ZeroMux 现有 ACP 实现(`src/acp/`)是本地 spawn 子进程 + stdio;
AgentCore 与 MicroVM 是同一接口的两种远程实现。

### 概念分离:会话 vs 任务

- **会话**(Session):有人在回路,出现在 Sidebar。tmux(永生)、
  AgentCore(8h 生命周期)语义不同,需类型标识 + 各自的生命周期操作。
- **任务**(Task):无人值守,提交 → 追踪 → 收产出。MicroVM 承载。
  UI 形态是任务看板 —— 这就是原 P2 "Dashboard" 的落地形态。

## 三、Agent Team(MicroVM 执行层)

### 依托项目

`~/sample-host-acp-on-awslambda-microvm`(目前仅有 docs/TASKS.md,代码未启动):
Serverless Headless Agent Task Service —— ACP 提供协议、MicroVM 提供牢笼、
harness 提供纪律。提交任务 → agent 无人值守执行 → 异步取回产出物 + 审计日志。

该项目"明确不做"的(多租户控制面、交互式体验)恰好是 ZeroMux 已有的;
ZeroMux 缺的弹性执行层恰好是它的主体。两项目互补,不重叠。

### Team 形态三档(已定:v1 做 A,B 是延伸,C 不做)

- **A. 并行任务扇出**:一个需求拆成 N 个独立子任务,每任务一个 MicroVM,
  各自跑完回收,ZeroMux 汇总产出。TASKS.md 架构的直接消费。
- **B. 角色化流水线**:architect → coder → reviewer 接力,每角色一个 VM,
  产出物(S3 工作区归档)在角色间传递。本质是"串行的 A + 产出物传递",
  依赖 harness 的快照/归档续跑机制。
- **C. 协作团队(常驻多 agent 互通)**:与"跑完即回收"的经济模型冲突
  (TASKS.md 风险表:心跳类行为破坏挂起经济性)。**明确不做。**

### 编排入口(初步口径,待定稿)

编排智能不进 ZeroMux。两个入口并存:

1. **本地 claude 会话调 ZeroMux 任务 API** —— 人在 tmux 里对 claude 说
   "把这个需求拆给 team 跑",claude 负责拆解并扇出;
2. **ZeroMux UI 手动提交**单个任务。

自动派发(如 GitHub Issue 打标签触发)未定,留待后续讨论。

### git 是产出物的主通道

任务合同带 repo + branch:VM 内 clone → 干活 → push branch,
ZeroMux 里看 diff(GitViewer 已有雏形)→ 本地合并。
S3 报告/日志是辅助,PR-able 的分支是主产出。

## 四、AgentCore 集成(远程交互式会话)

### 依托项目

`~/acp-on-agentcore`(**代码已完成、可部署**):ACP-WEB bridge 把
ACP agent(claude-agent-acp / codex / kiro)包装成 AgentCore Runtime 的
HTTP 契约(`POST /invocations` SSE + `GET /ping`),IAM/SigV4 鉴权。
含三个 agent 的 Dockerfile、IAM 策略、部署脚本、S3 会话归档(sessionstore)。

### 集成方式

ZeroMux ACP 会话加 `backend: agentcore`:把"spawn 本地进程"换成
"SigV4 签 `/invocations` + 消费 SSE → 转发到现有 WS"。UI、事件流、
会话模型全部复用。凭据用 instance role 签名,token 不出 AWS。

### AgentCore vs MicroVM 选型(互补,非二选一)

| | AgentCore backend | MicroVM backend |
|---|---|---|
| 适合 | 交互式远程会话(人在回路) | 无人值守任务(跑完回收) |
| 数据面 | SSE 流,天然适配 WS 转发 | request/response + webhook |
| 运维 | 全托管 | 自建控制面,挂起计费 + 快照续跑 |
| 隔离 | 托管 microVM,IAM per-runtime | 全 VM 隔离,YOLO 更硬 |
| 现状 | **成品可部署** | 仅 TASKS.md,M0 未验证 |

### 为什么先做 AgentCore、后做 MicroVM

1. acp-on-agentcore 是成品,能让"远程 agent 会话"抽象先立起来;
2. 接口先行:集成时定义的 remote backend trait 就是 MicroVM 要实现的接口;
3. 风险递减:SigV4、SSE→WS 转发、远程会话 UI、断线重连等通用问题
   先在托管环境踩平。

### 待决问题

- AgentCore region(us-west-2 最稳)与现有 us-east-1 Bedrock 用量是否分开;
- FileBrowser 学会浏览 S3 归档工作区(AgentCore 与 MicroVM 产出物共用此能力)。

## 五、其余主线(2026-08-14 讨论确认)

1. **端口代理 / Preview**(方案已定型,未实施):
   `*.zeromux.awscode.dev` 泛域名(需新 ACM 证书,含
   `*.zeromux.awscode.dev` SAN,us-east-1,DNS 验证)+ CloudFront alias +
   GoDaddy 泛解析 + zeromux 内置反代(从 Host 头解析端口,仅代理
   127.0.0.1,必过鉴权)+ Ports 侧栏自动发现。
   兜底:`/proxy/{port}/` 路径式(绝对路径资源会坏,仅作 fallback)。
2. **通知闭环**:PWA 已有,加 Web Push —— agent 完成/阻塞/等确认推手机。
   远程任务(MicroVM)完成通知是最刚需场景,穿插在 AgentCore/Team 之间做。
3. **安全欠账**:
   - `KillMode=mixed` → `KillMode=process`(未改!web 创建的 tmux 会话
     在 systemctl restart 时仍会被 SIGKILL,数据丢失级,5 分钟的事);
   - legacy token 是密码明文等价物,**做端口代理前必须**换签名 token
     (可复用现有 JWT 逻辑),否则代理 cookie 把管理密码撒给每个被预览的
     dev server;
   - OAuth 已有代码未启用,团队真要共用时再开。

## 六、总排序(2026-08-14)

1. **KillMode 修复** —— 5 分钟,随时会爆的雷
2. **端口代理 + token 签名化** —— 工作流闭环断点,方案已定
3. **AgentCore backend 集成** —— 远程会话抽象立起来
4. **Agent Team / MicroVM** —— 先完成 sample 项目 M0(出网/凭据/挂起恢复/
   webhook 四项冒烟)→ M1-M3,再作为第二个 remote backend 插入;
   任务看板 = 原 P2 Dashboard 的落地形态
5. **通知闭环** —— 穿插在 3/4 之间(远程任务完成通知最刚需)

原 P2 的"项目分组/Dashboard"不再单独立项,由任务看板 + 会话类型标识吸收。

## 七、近期已完成(背景,均已上线)

- tmux 窗口/pane 按钮条(new-window/split/kill,白名单 API,
  最后 pane 保护)
- tmux mouse on + history-limit 50000(滚动看历史)+ 手机翻页按钮
- OSC 52 剪贴板(addon-clipboard,自定义 provider 兼容 tmux 空 selection
  参数,NBSP 归一化)
- attach 会话的 work_dir 从 pane 推导
- ⚠️ 以上改动尚未 commit(6 个文件在工作树)

## 八、部署注意(踩过的坑)

- 前端 dist 经 rust_embed **编译时嵌入**二进制:构建顺序必须
  `npm run build` → `cargo build` → restart;update.sh 应包含前端构建。
- CloudFront 对 assets 缓存 max-age=3600,发版后需 invalidation
  (distribution E3TO3WB4E19DQM)。
- tmux server 若由 web 端首次创建,会落在 zeromux.service cgroup 内,
  KillMode=mixed 下重启即被杀 —— 见安全欠账第一条。
