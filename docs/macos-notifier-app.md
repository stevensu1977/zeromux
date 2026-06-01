# ZeroMux Notifier — macOS Native Agent Activity App

## Overview

A lightweight macOS menu bar app (Swift/SwiftUI) that provides ambient awareness of AI agent activity. Shows real-time status from ZeroMux's event API and/or local agent hooks, displayed as a menu bar icon + floating "Dynamic Island" style panel.

Supports multiple agents: Claude Code, Codex, Kiro — any tool that pushes events to ZeroMux or outputs local hook data.

## Goals

- **Ambient awareness** — know at a glance if agents are working, idle, or stuck
- **Non-intrusive** — menu bar icon + optional floating panel, never modal
- **Multi-agent** — unified view regardless of which agent is working
- **Dual mode** — works with remote ZeroMux server OR local-only (no server needed)

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Data Sources                                         │
├──────────────────────────────────────────────────────┤
│  1. ZeroMux API (remote)                             │
│     GET /api/events?since=xxx&limit=20               │
│     Polling every 5s                                 │
│                                                      │
│  2. Local Hook (no server needed)                    │
│     ~/.zeromux/events.jsonl                           │
│     Hook scripts append events to this file           │
│     App watches with FSEvents / DispatchSource        │
└──────────────────┬───────────────────────────────────┘
                   │
                   ▼
┌──────────────────────────────────────────────────────┐
│  EventStore (in-memory)                              │
│  - Deduplicates by event ID                          │
│  - Keeps last 100 events                             │
│  - Derives current status: idle/working/error        │
└──────────────────┬───────────────────────────────────┘
                   │
                   ▼
┌──────────────────────────────────────────────────────┐
│  UI Layer                                            │
├──────────────────────────────────────────────────────┤
│  1. Menu Bar Icon (NSStatusItem)                     │
│     - Color: green=working, gray=idle, red=error     │
│     - Badge: number of active agents                 │
│                                                      │
│  2. Island Panel (NSPanel, floating)                 │
│     - Click menu bar icon to toggle                  │
│     - Shows: active agents, recent task summaries    │
│     - Auto-hides after 5s of no new events           │
│                                                      │
│  3. Native Notifications                             │
│     - task_done: brief notification with summary     │
│     - error: alert-style notification                │
│     - Configurable: which events trigger notifs      │
└──────────────────────────────────────────────────────┘
```

## Data Model

### Event (shared with ZeroMux)

```swift
struct AgentEvent: Codable, Identifiable {
    let id: String
    let agent: String        // "claude-code" | "codex" | "kiro"
    let event: String        // "task_start" | "task_done" | "error"
    let summary: String
    let workDir: String?
    let metadata: [String: AnyCodable]?
    let timestamp: String    // ISO 8601
}
```

### AgentStatus (derived)

```swift
enum AgentState {
    case idle
    case working(since: Date)
    case error(message: String)
}

struct AgentStatus {
    let agent: String
    let state: AgentState
    let lastEvent: AgentEvent?
}
```

## UI Design

### Menu Bar Icon

```
┌─────┐
│  ◉  │  ← colored circle (SF Symbol: circle.fill)
└─────┘
   │
   │ click
   ▼
┌─────────────────────────────────┐
│  ZeroMux Agents                 │
├─────────────────────────────────┤
│  ● claude-code  working  2m    │
│  ○ codex        idle           │
│  ○ kiro         idle           │
├─────────────────────────────────┤
│  Recent:                        │
│  ✓ Added login page    12:30   │
│  ✓ Fixed scroll bug    12:25   │
│  ✓ Updated config      12:20   │
├─────────────────────────────────┤
│  Settings...                    │
│  Quit                           │
└─────────────────────────────────┘
```

### Island Panel (floating notification)

Appears briefly when task_done fires, stays on screen for ~5s:

```
╭─────────────────────────────────────╮
│ ● claude-code                       │
│ ✓ Added user authentication         │
│   /home/ubuntu/zeromux · 2m ago     │
╰─────────────────────────────────────╯
```

- Anchored to top-center of screen (near notch area on newer Macs)
- `NSPanel` with `.nonactivating` + `.floating` level
- `NSVisualEffectView` with `.hudWindow` material (blur/vibrancy)
- Rounded corners, compact size (~300x80px)
- Click to dismiss, or auto-dismiss after 5s

## Data Sources

### Mode 1: Remote ZeroMux Server

```swift
class ZeroMuxPoller: ObservableObject {
    var baseURL: String  // e.g. "https://zeromux.awscode.dev"
    var token: String
    var pollInterval: TimeInterval = 5.0
    
    func poll() async {
        let url = "\(baseURL)/api/events?since=\(lastTimestamp)&limit=20&token=\(token)"
        // URLSession.shared.data(from: url)
        // Parse JSON → [AgentEvent]
        // Merge into EventStore
    }
}
```

### Mode 2: Local File Watch

For when agents run locally without ZeroMux server:

```swift
class LocalEventWatcher: ObservableObject {
    let filePath = "~/.zeromux/events.jsonl"
    
    // Use DispatchSource.makeFileSystemObjectSource to watch for writes
    // On change: read new lines, parse as JSON, merge into EventStore
}
```

Local hook script (modified `zeromux-hook`):
```bash
#!/bin/bash
# Append to local file instead of (or in addition to) posting to server
EVENT_JSON=$(jq -n --arg agent "claude-code" --arg event "$EVENT" --arg summary "$SUMMARY" ...)
echo "$EVENT_JSON" >> ~/.zeromux/events.jsonl
```

### Mode 3: Both (default)

Poll remote server AND watch local file. Deduplicate by event ID.

## Settings

```swift
struct AppSettings: Codable {
    var remoteURL: String?           // nil = local only
    var token: String?
    var pollInterval: TimeInterval   // default 5s
    var showNotifications: Bool      // default true
    var notifyEvents: [String]       // default: ["task_done", "error"]
    var islandDuration: TimeInterval // default 5s
    var enableLocalWatch: Bool       // default true
}
```

Settings stored in `UserDefaults` or `~/.zeromux/notifier.json`.

UI: Standard SwiftUI Settings window (`Settings` scene in SwiftUI lifecycle).

## Project Structure

```
zeromux-notifier/
├── Package.swift (or .xcodeproj)
├── Sources/
│   ├── ZeroMuxNotifierApp.swift    // @main, MenuBarExtra
│   ├── Models/
│   │   ├── AgentEvent.swift
│   │   └── AgentStatus.swift
│   ├── Services/
│   │   ├── EventStore.swift         // In-memory store + dedup
│   │   ├── ZeroMuxPoller.swift      // Remote API polling
│   │   └── LocalEventWatcher.swift  // File system watcher
│   ├── Views/
│   │   ├── MenuBarView.swift        // Menu content (SwiftUI)
│   │   ├── IslandPanel.swift        // Floating notification panel
│   │   └── SettingsView.swift       // Preferences window
│   └── Utilities/
│       └── AnyCodable.swift         // For metadata dict
└── Resources/
    └── Assets.xcassets              // App icon, SF Symbols
```

## Key Implementation Details

### Menu Bar App (no dock icon)

```swift
@main
struct ZeroMuxNotifierApp: App {
    @StateObject private var eventStore = EventStore()
    
    var body: some Scene {
        MenuBarExtra {
            MenuBarView(store: eventStore)
        } label: {
            Image(systemName: eventStore.statusIcon)
                .foregroundColor(eventStore.statusColor)
        }
        
        Settings {
            SettingsView()
        }
    }
}
```

### Floating Panel

```swift
class IslandPanelController {
    private var panel: NSPanel?
    
    func show(event: AgentEvent) {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 320, height: 80),
            styleMask: [.nonactivatingPanel, .fullSizeContentView],
            backing: .buffered, defer: true
        )
        panel.level = .floating
        panel.isMovableByWindowBackground = true
        panel.backgroundColor = .clear
        
        // Position near top center (notch area)
        if let screen = NSScreen.main {
            let x = (screen.frame.width - 320) / 2
            let y = screen.frame.height - 100
            panel.setFrameOrigin(NSPoint(x: x, y: y))
        }
        
        // SwiftUI content
        panel.contentView = NSHostingView(rootView: IslandView(event: event))
        panel.orderFront(nil)
        
        // Auto dismiss
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
            panel.close()
        }
    }
}
```

### Local Agent Support (without ZeroMux)

The app should work standalone. If no remote URL is configured, it watches `~/.zeromux/events.jsonl` only. This means:

- Claude Code hook: appends to `~/.zeromux/events.jsonl`
- Codex: could write to same file via a wrapper script
- Kiro: same pattern

Modified hook script supports both modes:

```bash
#!/bin/bash
# Post to ZeroMux server (if configured)
if [ -n "$ZEROMUX_URL" ] && [ -n "$ZEROMUX_TOKEN" ]; then
    curl -s -X POST "${ZEROMUX_URL}/api/events?token=${ZEROMUX_TOKEN}" ... &
fi

# Always append to local file for desktop app
mkdir -p ~/.zeromux
echo "$EVENT_JSON" >> ~/.zeromux/events.jsonl
```

## Distribution

- **Direct download**: `.app` bundle in GitHub releases
- **Homebrew cask**: `brew install --cask zeromux-notifier`
- **App Store**: possible (no private APIs used), but adds review friction

Minimum macOS version: 13.0 (Ventura) — for `MenuBarExtra` SwiftUI API.

## Future Ideas

- **Siri Shortcuts integration** — "Hey Siri, what are my agents doing?"
- **Widgets** — macOS desktop widget showing agent status
- **Sound effects** — subtle chime on task_done (configurable)
- **Focus mode integration** — suppress notifications during Focus
- **Pet mode** — animated character that reacts to agent events (optional overlay layer using same data source)
- **iPhone companion** — if using CloudKit sync or just polling same remote URL
