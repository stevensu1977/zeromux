# File Browser Redesign — Tree View with Full Features

## Overview

Replace the current flat-grouped file list with a recursive tree file browser, similar to VS Code's explorer sidebar. All operations use inline hover icons (no right-click menus since this runs in a browser).

## Current State

- File list is flat, grouped by first-level directory
- Only supports `*.md`, `*.image`, or `*` via mode toggle
- No file download
- No nested directory expand/collapse
- Upload works but only for current base dir

## New Design

### Backend API Changes

#### New: `GET /api/sessions/{id}/tree`

Returns direct children of a directory (lazy loading).

**Query params:**
- `path` — relative path to list (default: `.` = root)
- `base_dir` — override base directory (for tmux sessions)

**Response:**
```json
{
  "path": "src",
  "entries": [
    { "name": "main.rs", "path": "src/main.rs", "type": "file", "size": 4096, "modified": 1717000000 },
    { "name": "lib", "path": "src/lib", "type": "dir" },
    { "name": "utils.rs", "path": "src/utils.rs", "type": "file", "size": 1024, "modified": 1717000000 }
  ]
}
```

Entries sorted: directories first (alphabetical), then files (alphabetical).

#### New: `GET /api/sessions/{id}/file/download`

Returns raw binary file for browser download.

**Query params:**
- `path` — relative file path
- `base_dir` — override base directory

**Response:** Raw bytes with `Content-Disposition: attachment; filename="xxx"` header.

#### Keep Existing
- `GET /api/sessions/{id}/file` — read file for preview (text or base64 image)
- `POST /api/sessions/{id}/file` — write/create file
- `DELETE /api/sessions/{id}/file` — delete file
- `POST /api/sessions/{id}/file/rename` — rename
- `POST /api/sessions/{id}/upload` — upload (base64)
- `POST /api/sessions/{id}/dir` — create directory
- `DELETE /api/sessions/{id}/dir` — delete directory

#### Remove
- `GET /api/sessions/{id}/files` — old flat listing (replaced by `/tree`)

### Frontend Component: `FileBrowser.tsx`

Replaces `MarkdownViewer.tsx`.

#### Layout

```
┌─────────────────────────────────────────────────────────┐
│ [Toolbar: New File | New Dir | Upload | Refresh]        │
├──────────────────────┬──────────────────────────────────┤
│  File Tree (w-56)    │  Preview Pane                    │
│                      │                                  │
│  ▼ src/              │  (Markdown / Image / Text /      │
│    ├─ main.rs        │   Binary placeholder)            │
│    ├─ lib/           │                                  │
│    │  └─ utils.rs    │                                  │
│    └─ web.rs         │                                  │
│  ▶ docs/             │                                  │
│  ▶ frontend/         │                                  │
│    package.json      │                                  │
│    README.md         │                                  │
│                      │                                  │
├──────────────────────┴──────────────────────────────────┤
│ [Path breadcrumb: / > src > main.rs]  (optional)        │
└─────────────────────────────────────────────────────────┘
```

#### TreeNode Component (recursive)

```tsx
interface TreeNode {
  name: string
  path: string
  type: 'file' | 'dir'
  size?: number
  modified?: number
}
```

**Directory node:**
- Click → toggle expand/collapse
- Expand triggers lazy load of children via `/tree?path=xxx`
- Shows ▶ (collapsed) or ▼ (expanded) chevron
- Hover shows: [+ New File] [📁 New Dir] [🗑 Delete]

**File node:**
- Click → load preview in right pane
- Hover shows: [⬇ Download] [✏ Rename] [🗑 Delete]
- Icon based on extension (image icon for images, file icon for others)

**Indent:** Each depth level adds `pl-4` (16px)

#### State Management

```tsx
// Expanded directories
const [expanded, setExpanded] = useState<Set<string>>(new Set(['.']))

// Loaded children per directory path
const [children, setChildren] = useState<Record<string, TreeNode[]>>({})

// Currently selected file
const [selectedPath, setSelectedPath] = useState<string | null>(null)

// Inline rename state
const [renamingPath, setRenamingPath] = useState<string | null>(null)
```

#### File Preview (right pane)

Same as current behavior:
- `.md` files → rendered Markdown
- Image files (png/jpg/gif/webp/svg) → `<img>` preview
- Other text files → monospace code view (no editing initially)
- Binary files → "Binary file — cannot preview" + download button

#### File Actions (hover icons, inline)

| Target | Actions shown on hover |
|--------|----------------------|
| File   | Download ⬇, Rename ✏, Delete 🗑 |
| Dir    | New File +, New Dir 📁, Delete 🗑 |

Icons are small (10-12px), appear right-aligned in the row.

#### Upload

- Toolbar upload button (same as now)
- Drag & drop onto the tree area
- Files upload to currently expanded/selected directory
- Clipboard paste via existing ClipboardUpload component

#### Download

Triggered by hover icon on file rows:
```tsx
const handleDownload = (path: string) => {
  const url = `/api/sessions/${sessionId}/file/download?path=${encodeURIComponent(path)}&token=...`
  const a = document.createElement('a')
  a.href = url
  a.download = path.split('/').pop() || 'file'
  a.click()
}
```

### Hidden/Filtered Items

Skip from display (same as current backend):
- `.git`, `node_modules`, `target`, `__pycache__`
- Hidden files (`.` prefix) — optionally toggle-able

### Per-Session State

- Expanded directories: in-memory only (reset on page refresh)
- Base directory: per-session localStorage (existing `zeromux_docs_basedir_{sessionId}`)

### Removed

- `md` / `image` / `all` mode toggle — no longer needed, show everything
- `groupByDir` flat grouping logic
- `*.md` pattern-based file listing

## Implementation Plan

### Phase 1: Backend
1. Add `GET /api/sessions/{id}/tree` handler — list directory entries
2. Add `GET /api/sessions/{id}/file/download` handler — raw binary response
3. Keep old `/files` endpoint temporarily for backward compat

### Phase 2: Frontend
1. Create `FileBrowser.tsx` (new component, replaces `MarkdownViewer.tsx`)
2. Recursive `TreeNode` rendering with lazy-load expand
3. File preview pane (reuse existing markdown/image rendering)
4. Inline hover actions (download, rename, delete)
5. Upload: toolbar button + drag-and-drop zone
6. Wire up in `App.tsx` (replace `MarkdownViewer` import)

### Phase 3: Polish
1. File type icons (different icons for .ts, .rs, .json, images, etc.)
2. Breadcrumb path display in preview header
3. Keyboard navigation (optional)
4. Drag-and-drop reorder/move (future)

## Tradeoffs

- **Lazy load vs full tree**: Lazy load avoids scanning huge repos. Each expand = 1 API call. Tradeoff: slightly slower expand for deep trees but safe for large projects.
- **No file editing**: Preview-only for now (except markdown which already has edit). Full editor (Monaco) is out of scope.
- **No right-click**: All operations via hover icons. Mobile-friendly, no ambiguity with browser context menu.
