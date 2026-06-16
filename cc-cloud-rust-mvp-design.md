# 🦀 CC-Cloud: Rust MVP 设计方案

> **Auth + WebTmux + ACP 三合一 Claude Code Web 控制器**
>
> 日期: 2026-04-03

---

## 一、产品定义

**一个 Rust 单二进制 Web 服务**，提供：

1. **Auth** — API Token 单用户认证
2. **WebTmux** — 浏览器内 xterm.js 终端，PTY 直连 tmux/claude
3. **ACP 控制面** — 结构化 JSON-RPC 2.0 控制 Claude Code / Kiro CLI

---

## 二、架构总览

```
                          ┌──────────────────────────────────────┐
                          │        cc-cloud (Rust 单二进制)        │
 浏览器                    │                                      │
┌──────────┐   HTTPS      │  ┌──────────────┐                    │
│ xterm.js │◄────WS──────►│  │  PTY Bridge  │◄─PTY─► tmux/shell  │
│ 终端面板  │  /ws/term    │  │ (portable-pty│        └─► claude   │
│          │              │  │  + tokio)     │                    │
├──────────┤              │  └──────────────┘                    │
│ ACP 控制 │◄────WS──────►│  ┌──────────────┐                    │
│ 消息面板  │  /ws/acp     │  │  ACP Client  │◄─stdin/stdout──►   │
│ 工具审批  │              │  │ (JSON-RPC2.0)│  claude -p --stream │
│ Token追踪│              │  └──────────────┘   或 kiro-cli acp   │
├──────────┤              │  ┌──────────────┐                    │
│ REST API │◄────HTTP────►│  │  axum Router │                    │
│ 状态/配置 │  /api/*      │  │  + Bearer 中间件                   │
└──────────┘              │  └──────────────┘                    │
                          │  ┌──────────────┐                    │
                          │  │ rust-embed   │ HTML/JS/CSS 内嵌    │
                          │  └──────────────┘                    │
                          └──────────────────────────────────────┘
```

### 关键设计：两条独立通道

| 通道 | 路径 | I/O 模型 | 用途 |
|---|---|---|---|
| **Terminal** | `/ws/term` | PTY (伪终端) | 看到 claude 完整 TUI / tmux 面板 |
| **ACP Control** | `/ws/acp` | stdin/stdout 管道 | 结构化控制：发消息、审批工具、读取 token 用量 |

两个通道可以**同时存在**但控制**不同的 claude 进程**（interactive TUI vs headless stream-json），也可以只用其中一个。

---

## 三、技术选型

### 3.1 Rust 依赖

```toml
[dependencies]
# Web 框架 + WebSocket
axum = "0.8"                       # 155M downloads, 路由/中间件/WS一体
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.29"          # 158M downloads, WS 底层
tower-http = { version = "0.6", features = ["cors", "auth"] }

# PTY
portable-pty = "0.9"                # 5M downloads, WezTerm 作者出品, 跨平台

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# 前端内嵌
rust-embed = "8.11"                 # 28M downloads, 编译时打包 HTML/JS

# 工具
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
tracing-subscriber = "0.3"
clap = { version = "4", features = ["derive"] }
toml = "0.8"                        # 配置文件
```

**不需要的：** 无需 jsonrpc-core（ACP 消息类型少，serde_json 手写更灵活）

### 3.2 前端依赖

| 库 | 版本 | 用途 |
|---|---|---|
| xterm.js | 5.x | 终端渲染 (WebGL addon) |
| xterm-addon-fit | 0.10 | 自动 resize |
| xterm-addon-webgl | 0.18 | GPU 加速渲染 |
| 无框架 | — | 纯 vanilla JS，MVP 不引入 React |

---

## 四、项目结构

```
cc-cloud/
├── Cargo.toml
├── config.toml                     # 运行时配置
├── src/
│   ├── main.rs                     # 入口：加载配置 → 启动 axum server
│   ├── config.rs                   # 配置结构体 (Token, CLI路径, 端口等)
│   ├── auth.rs                     # Bearer Token 中间件 (~40行)
│   ├── server.rs                   # axum Router 组装
│   │
│   ├── terminal/                   # ── WebTmux 通道 ──
│   │   ├── mod.rs
│   │   ├── pty.rs                  # portable-pty 封装: spawn + async read/write
│   │   ├── ws_handler.rs           # /ws/term WebSocket handler
│   │   └── resize.rs              # 终端 resize 处理
│   │
│   ├── acp/                        # ── ACP 控制通道 ──
│   │   ├── mod.rs
│   │   ├── protocol.rs             # ACP JSON-RPC 2.0 消息类型定义
│   │   ├── claude_stream.rs        # Claude stream-json NDJSON 协议
│   │   ├── process.rs              # 子进程生命周期管理 (spawn/kill/resume)
│   │   └── ws_handler.rs           # /ws/acp WebSocket handler
│   │
│   └── web/                        # ── 前端资源 ──
│       └── mod.rs                  # rust-embed 服务静态文件
│
├── frontend/                       # 前端 (编译后嵌入二进制)
│   ├── index.html                  # 主页面：双面板布局
│   ├── terminal.js                 # xterm.js 终端逻辑
│   ├── acp.js                      # ACP WebSocket 客户端
│   ├── style.css                   # 样式
│   └── vendor/                     # xterm.js 等第三方 JS (vendored)
│
├── deploy/
│   ├── systemd/cc-cloud.service
│   └── Dockerfile
│
└── README.md
```

**预估代码量：~2000 行 Rust + ~500 行 JS**

---

## 五、模块详设

### 5.1 Auth (`src/auth.rs`) — ~40 行

MVP 最简方案：配置文件里一个 `api_token`，所有请求用 Bearer Token 校验。

```rust
// config.toml
[auth]
api_token = "cc-sk-xxxxxxxxxxxx"  # 启动时随机生成或手动配置

// 校验逻辑
async fn auth_middleware(
    State(config): State<AppConfig>,
    req: Request,
    next: Next,
) -> Response {
    let token = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) if t == config.auth.api_token => next.run(req).await,
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}
```

WebSocket 握手时通过 query param `?token=xxx` 或首条消息传 token。

### 5.2 Terminal 通道 (`src/terminal/`) — ~300 行

**核心流程：**

```
浏览器 xterm.js ◄──WS──► ws_handler ◄──async channel──► PTY read/write loop
```

```rust
// pty.rs — 核心封装
pub struct PtySession {
    pair: PtyPair,              // portable-pty 的 master/slave pair
    child: Box<dyn Child>,      // 子进程 handle
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

impl PtySession {
    pub fn spawn(cmd: &str, args: &[&str], size: (u16, u16)) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: size.1, cols: size.0,
            pixel_width: 0, pixel_height: 0,
        })?;

        let cmd = CommandBuilder::new(cmd);
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd)?;
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        Ok(Self { pair, child, reader, writer })
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.pair.master.resize(PtySize {
            rows, cols, pixel_width: 0, pixel_height: 0,
        })
    }
}
```

```rust
// ws_handler.rs — WebSocket ↔ PTY 桥接
pub async fn terminal_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(|socket| handle_terminal(socket, state))
}

async fn handle_terminal(socket: WebSocket, state: AppState) {
    let pty = PtySession::spawn("tmux", &["new-session", "-A", "-s", "main"], (80, 24))
        .expect("Failed to spawn PTY");

    let (mut ws_sink, mut ws_stream) = socket.split();

    // PTY → Browser (spawn_blocking 因为 portable-pty 是同步的)
    let reader = pty.reader;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => { tx.blocking_send(buf[..n].to_vec()).ok(); }
                Err(_) => break,
            }
        }
    });

    // 同时：
    // rx → ws_sink (PTY输出推给浏览器)
    // ws_stream → writer (浏览器输入写给PTY)
    // 处理 resize 消息
}
```

**WebSocket 消息协议（仿 WebTmux）：**

| 方向 | Type | 格式 | 说明 |
|---|---|---|---|
| Client→Server | `input` | `{"type":"input","data":"base64..."}` | 键盘输入 |
| Client→Server | `resize` | `{"type":"resize","cols":120,"rows":40}` | 终端尺寸变化 |
| Server→Client | `output` | `{"type":"output","data":"base64..."}` | 终端输出 |
| Server→Client | `exit` | `{"type":"exit","code":0}` | 进程退出 |

### 5.3 ACP 控制通道 (`src/acp/`) — ~600 行

**双协议支持，trait 抽象：**

```rust
// protocol.rs — 协议 trait
#[async_trait]
pub trait AgentProtocol: Send + Sync {
    fn name(&self) -> &str;
    fn build_args(&self, opts: &SpawnOptions) -> Vec<String>;
    async fn init(&self, rw: &mut JsonRW, resume_id: Option<&str>) -> Result<String>;
    async fn send_message(&self, writer: &mut impl AsyncWrite, text: &str) -> Result<()>;
    fn parse_event(&self, line: &[u8]) -> Result<AgentEvent>;
}
```

```rust
// claude_stream.rs — Claude stream-json 协议实现
pub struct ClaudeStreamProtocol;

impl AgentProtocol for ClaudeStreamProtocol {
    fn build_args(&self, opts: &SpawnOptions) -> Vec<String> {
        let mut args = vec![
            "-p".into(),
            "--output-format".into(), "stream-json".into(),
            "--input-format".into(), "stream-json".into(),
            "--verbose".into(),
            "--dangerously-skip-permissions".into(),
        ];
        if let Some(model) = &opts.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(id) = &opts.resume_id {
            args.extend(["--resume".into(), id.clone()]);
        }
        args
    }
    // ...
}
```

```rust
// protocol.rs — ACP JSON-RPC 2.0 协议实现
pub struct ACPProtocol;

// JSON-RPC 2.0 消息类型
#[derive(Serialize, Deserialize)]
struct RpcRequest {
    jsonrpc: String,  // "2.0"
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
struct RpcNotification {
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
}

// ACP 握手流程:
// 1. → initialize { protocolVersion: 1, clientInfo: {...} }
// 2. ← initialize response
// 3. → session/new {} 或 session/load { sessionId }
// 4. ← session/new response { sessionId }
// 5. → session/prompt { sessionId, content: [...] }
// 6. ← session/update (streaming notifications)
// 7. ← session/prompt response (turn complete)
```

```rust
// process.rs — 子进程管理
pub struct AgentProcess {
    child: tokio::process::Child,
    stdin: ChildStdin,
    stdout_lines: tokio::io::Lines<BufReader<ChildStdout>>,
    session_id: Option<String>,
    state: ProcessState,
}

pub enum ProcessState {
    Spawning,
    Ready,
    Running,  // 正在处理一个 turn
    Dead,
}

impl AgentProcess {
    pub async fn spawn(
        cli_path: &str,
        protocol: &dyn AgentProtocol,
        opts: &SpawnOptions,
    ) -> Result<Self> {
        let args = protocol.build_args(opts);
        let mut child = tokio::process::Command::new(cli_path)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();

        Ok(Self { child, stdin, stdout_lines: stdout, session_id: None, state: ProcessState::Spawning })
    }

    pub async fn send(&mut self, text: &str) -> Result<AgentResponse> {
        // 写入消息 → 读取事件流 → 收集完整响应
    }
}
```

**ACP WebSocket 消息协议（浏览器 ↔ 服务端）：**

| 方向 | Type | 说明 |
|---|---|---|
| Client→Server | `prompt` | `{"type":"prompt","text":"写一个hello world"}` |
| Client→Server | `approve` | `{"type":"approve","tool_call_id":"xxx"}` |
| Client→Server | `cancel` | `{"type":"cancel"}` 中断当前 turn |
| Server→Client | `chunk` | `{"type":"chunk","text":"正在..."}` 流式文本 |
| Server→Client | `tool_call` | `{"type":"tool_call","id":"xxx","name":"bash","input":{...}}` |
| Server→Client | `result` | `{"type":"result","text":"...","tokens":{"input":1234,"output":567}}` |
| Server→Client | `error` | `{"type":"error","message":"..."}` |

### 5.4 配置 (`config.toml`)

```toml
[server]
host = "0.0.0.0"
port = 8080

[auth]
# 留空则启动时自动生成并打印到 stdout
api_token = ""

[terminal]
# PTY 启动命令：可以是 tmux、直接 claude、或任意 shell
command = "tmux"
args = ["new-session", "-A", "-s", "cc-main"]
# 也可以直接: command = "claude"

[acp]
# Claude CLI 或 Kiro CLI 路径
cli_path = "claude"
# 协议: "stream-json" (Claude) 或 "acp" (Kiro)
protocol = "stream-json"
model = ""
# 超时
no_output_timeout_secs = 120
total_turn_timeout_secs = 300

[acp.env]
# 透传给 CLI 子进程的环境变量
ANTHROPIC_API_KEY = "${ANTHROPIC_API_KEY}"
```

### 5.5 前端 (`frontend/`) — ~500 行 JS

```html
<!-- index.html — 双面板布局 -->
<!DOCTYPE html>
<html>
<head>
    <title>CC-Cloud</title>
    <link rel="stylesheet" href="/vendor/xterm.css">
    <link rel="stylesheet" href="/style.css">
</head>
<body>
    <nav id="topbar">
        <span class="logo">⚡ CC-Cloud</span>
        <span class="status" id="status">disconnected</span>
    </nav>

    <div id="workspace">
        <!-- 左侧: 终端 -->
        <div id="terminal-panel">
            <div class="panel-header">
                Terminal
                <button onclick="toggleFullscreen('terminal')">⛶</button>
            </div>
            <div id="terminal-container"></div>
        </div>

        <!-- 右侧: ACP 控制 -->
        <div id="acp-panel">
            <div class="panel-header">
                ACP Control
                <button onclick="toggleFullscreen('acp')">⛶</button>
            </div>
            <div id="messages"></div>
            <div id="tool-approvals"></div>
            <div id="input-bar">
                <textarea id="prompt-input" placeholder="Send a message..."></textarea>
                <button id="send-btn" onclick="sendPrompt()">Send</button>
            </div>
        </div>
    </div>

    <script src="/vendor/xterm.js"></script>
    <script src="/vendor/xterm-addon-fit.js"></script>
    <script src="/vendor/xterm-addon-webgl.js"></script>
    <script src="/terminal.js"></script>
    <script src="/acp.js"></script>
</body>
</html>
```

---

## 六、API 路由总览

| 方法 | 路径 | Auth | 说明 |
|---|---|---|---|
| GET | `/` | ✅ | 主页面 (index.html) |
| WS | `/ws/term` | ✅ | 终端 WebSocket |
| WS | `/ws/acp` | ✅ | ACP 控制 WebSocket |
| GET | `/api/status` | ✅ | 服务状态 (进程状态、session info) |
| POST | `/api/terminal/resize` | ✅ | 备用 resize 端点 |
| POST | `/api/acp/spawn` | ✅ | 手动启动/重启 ACP 进程 |
| POST | `/api/acp/kill` | ✅ | 终止 ACP 进程 |
| GET | `/api/config` | ✅ | 读取当前配置 |
| GET | `/assets/*` | ❌ | 静态资源 (JS/CSS) |

---

## 七、关键技术点

### 7.1 portable-pty 是同步的 — 如何接入 tokio？

`portable-pty` 的 `read()`/`write()` 是阻塞调用，需要 `spawn_blocking`：

```rust
// 读取循环在 blocking 线程中运行
let (tx, rx) = mpsc::channel(256);
tokio::task::spawn_blocking(move || {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
});
// rx 在 async 上下文中消费，推给 WebSocket
```

### 7.2 PTY Resize

浏览器 xterm.js `fitAddon` 检测容器尺寸变化 → 发送 `resize` WS 消息 → 后端调用 `pty.resize(cols, rows)`。

### 7.3 ACP 进程看门狗

双超时看门狗：

```rust
tokio::select! {
    line = self.stdout_lines.next_line() => { /* 处理事件 */ }
    _ = tokio::time::sleep(no_output_timeout) => {
        // 2分钟无输出 → kill
        self.kill().await;
    }
}
```

### 7.4 安全：`--dangerously-skip-permissions`

MVP 阶段 hardcode 跳过权限。后续版本可以通过 ACP 通道的 `tool_call` 消息在前端做交互式审批。

---

## 八、MVP 开发计划

| 阶段 | 内容 | 预估时间 |
|---|---|---|
| **P0: 骨架** | cargo init + axum + config + auth middleware + rust-embed | 0.5 天 |
| **P1: Terminal** | portable-pty spawn + WS 桥接 + xterm.js 前端 | 1.5 天 |
| **P2: ACP** | Claude stream-json 协议 + WS handler + 消息面板 | 2 天 |
| **P3: 整合** | 双面板 UI + 状态同步 + 进程管理 API | 1 天 |
| **P4: 打磨** | 错误处理 + 重连 + 配置热加载 + Dockerfile | 1 天 |
| **合计** | | **~6 天** |

### MVP 交付物

- [x] 单二进制，`./cc-cloud` 启动即用
- [x] 浏览器打开看到双面板：左边终端（tmux/claude TUI），右边 ACP 结构化控制
- [x] Bearer Token 认证
- [x] Claude stream-json 协议完整实现
- [x] 终端 resize + WebGL 渲染
- [ ] ~~多用户~~ (后续)
- [ ] ~~ACP Kiro 协议~~ (后续)
- [ ] ~~TLS~~ (反代或后续)

---

## 九、后续演进路线

```
MVP (v0.1)                    v0.2                      v0.3
┌─────────────┐         ┌─────────────┐          ┌─────────────┐
│ 单用户 Token │    →    │ 多用户 OIDC  │    →     │ 团队协作     │
│ 单 Session   │         │ 多 Session   │          │ 共享 Session │
│ Claude only  │         │ + Kiro ACP   │          │ + Codex      │
│ 双面板 UI    │         │ + tmux 感知   │          │ + 文件浏览器  │
│ HTTP only    │         │ + TLS        │          │ + Git 集成   │
└─────────────┘         └─────────────┘          └─────────────┘
```

---

## 十、参考项目

| 项目 | 借鉴点 |
|---|---|
| **WebTmux** (chrismccord) | PTY↔WebSocket 桥接、xterm.js 前端、tmux 感知 |
| **Companion** (2.3K⭐) | `--sdk-url` WebSocket 桥接、工具审批 UI |
| **CloudCLI** (9.4K⭐) | 完整 Web IDE 参考、MCP 管理 UI |
| **WezTerm** | portable-pty crate 的上游项目 |

---

## 附录 A: 快速验证命令

```bash
# 验证 Claude CLI stream-json 协议
echo '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}' | \
  claude -p --output-format stream-json --input-format stream-json --verbose

# 验证 portable-pty 能跑 tmux
cargo run --example pty-basic -- tmux new-session -A -s test

# 验证 WebSocket + xterm.js
# 用 websocat 测试: websocat ws://localhost:8080/ws/term
```

---

## 附录 B: Claude Code 两种 I/O 模式互斥说明

```
┌──────────────────────────────────────────────────────┐
│              Claude Code CLI I/O 模式                 │
│                                                      │
│  模式 A: 交互模式 (默认)                               │
│  ├─ 需要: TTY (isatty=true)                          │
│  ├─ 输出: 终端 escape sequences (Ink TUI)             │
│  ├─ 输入: 键盘事件                                    │
│  └─ 适合: WebTmux 终端面板                             │
│                                                      │
│  模式 B: Stream-JSON (headless)                      │
│  ├─ 需要: -p --output-format stream-json             │
│  ├─ 输出: NDJSON (结构化事件流)                        │
│  ├─ 输入: JSON 消息 via stdin                         │
│  └─ 适合: ACP 控制面板                                │
│                                                      │
│  ⚠️ 同一进程只能选一种，不可同时                         │
│  本方案用两个独立进程解决此限制                           │
└──────────────────────────────────────────────────────┘
```
