import { useEffect, useRef, useCallback, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { WebglAddon } from '@xterm/addon-webgl'
import { ClipboardAddon } from '@xterm/addon-clipboard'
import { wsUrl, getSessionStatus, tmuxAction, getSessionPorts, exposePort } from '../lib/api'
import type { SessionStatus, TmuxAction, SessionPort } from '../lib/api'
import type { Theme } from '../lib/theme'
import { b64encode, b64decode } from '../lib/base64'
import {
  GitBranch, Folder, Circle,
  Plus, ChevronLeft, ChevronRight, Columns2, Rows2, LayoutGrid, X,
  ChevronsUp, ChevronsDown, Globe,
} from 'lucide-react'

// A hidden (display:none) terminal makes FitAddon propose a collapsed size.
// Treat anything below a sane minimum as "not laid out yet" and skip the resize
// so we never shrink the shared tmux window and truncate other clients.
const MIN_COLS = 20
const MIN_ROWS = 5
function isValidDims(cols?: number, rows?: number): boolean {
  return !!cols && !!rows && cols >= MIN_COLS && rows >= MIN_ROWS
}

const THEMES = {
  dark: {
    background: '#0d1117',
    foreground: '#c9d1d9',
    cursor: '#58a6ff',
    selectionBackground: '#264f78',
    black: '#484f58',
    red: '#ff7b72',
    green: '#3fb950',
    yellow: '#d29922',
    blue: '#58a6ff',
    magenta: '#bc8cff',
    cyan: '#39c5cf',
    white: '#b1bac4',
    brightBlack: '#6e7681',
    brightRed: '#ffa198',
    brightGreen: '#56d364',
    brightYellow: '#e3b341',
    brightBlue: '#79c0ff',
    brightMagenta: '#d2a8ff',
    brightCyan: '#56d4dd',
    brightWhite: '#f0f6fc',
  },
  light: {
    background: '#ffffff',
    foreground: '#1f2328',
    cursor: '#0969da',
    selectionBackground: '#b6d4fe',
    black: '#24292f',
    red: '#cf222e',
    green: '#1a7f37',
    yellow: '#9a6700',
    blue: '#0969da',
    magenta: '#8250df',
    cyan: '#1b7c83',
    white: '#6e7781',
    brightBlack: '#57606a',
    brightRed: '#a40e26',
    brightGreen: '#116329',
    brightYellow: '#7d4e00',
    brightBlue: '#0550ae',
    brightMagenta: '#6639ba',
    brightCyan: '#136061',
    brightWhite: '#8c959f',
  },
}

interface Props {
  sessionId: string
  active: boolean
  theme: Theme
}

const TMUX_BUTTONS: Array<{ action: TmuxAction; title: string; Icon: typeof Plus }> = [
  { action: 'page-up', title: 'Scroll history up', Icon: ChevronsUp },
  { action: 'page-down', title: 'Scroll history down', Icon: ChevronsDown },
  { action: 'new-window', title: 'New window', Icon: Plus },
  { action: 'prev-window', title: 'Previous window', Icon: ChevronLeft },
  { action: 'next-window', title: 'Next window', Icon: ChevronRight },
  { action: 'split-h', title: 'Split left/right', Icon: Columns2 },
  { action: 'split-v', title: 'Split top/bottom', Icon: Rows2 },
  { action: 'next-pane', title: 'Focus next pane', Icon: LayoutGrid },
  { action: 'kill-pane', title: 'Close pane', Icon: X },
]

export default function TerminalView({ sessionId, active, theme }: Props) {
  const containerRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const initRef = useRef(false)
  const [status, setStatus] = useState<SessionStatus | null>(null)
  const [tmuxError, setTmuxError] = useState<string | null>(null)
  const [ports, setPorts] = useState<SessionPort[]>([])

  const runTmuxAction = useCallback((action: TmuxAction) => {
    tmuxAction(sessionId, action)
      .then(() => setTmuxError(null))
      .catch(e => setTmuxError(String(e.message || e)))
    // Keep keystrokes flowing to the terminal after a button tap
    termRef.current?.focus()
  }, [sessionId])

  // Fetch status + listening ports
  useEffect(() => {
    let cancelled = false
    const fetchStatus = () => {
      getSessionStatus(sessionId).then(s => {
        if (!cancelled) setStatus(s)
      }).catch(() => {})
      getSessionPorts(sessionId).then(p => {
        if (!cancelled) setPorts(p)
      }).catch(() => {})
    }
    fetchStatus()
    const interval = setInterval(fetchStatus, 10000)
    return () => { cancelled = true; clearInterval(interval) }
  }, [sessionId])

  // Initialize terminal once
  useEffect(() => {
    if (initRef.current || !containerRef.current) return
    initRef.current = true

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', Menlo, monospace",
      theme: THEMES[theme],
      allowProposedApi: true,
      scrollback: 10000,
    })

    const fit = new FitAddon()
    term.loadAddon(fit)
    // OSC 52 support: tmux copy-mode (mouse drag) pushes the selection here,
    // which the addon writes to the browser clipboard. tmux emits an empty
    // selection type (ESC]52;;...), which the addon's default provider drops
    // (it only accepts 'c'), so use a provider that accepts any type.
    // TUIs (claude code etc.) render with NBSP instead of spaces; normalize so
    // pasted text doesn't carry invisible 0xA0 characters.
    term.loadAddon(new ClipboardAddon(undefined, {
      readText: () => navigator.clipboard.readText().catch(() => ''),
      writeText: (_sel: string, text: string) =>
        navigator.clipboard.writeText(text.replace(/\u00a0/g, ' ')).catch(() => {}),
    }))
    term.open(containerRef.current)

    try {
      term.loadAddon(new WebglAddon())
    } catch {
      // fallback to canvas
    }

    fit.fit()
    termRef.current = term
    fitRef.current = fit

    term.onData(data => {
      const ws = wsRef.current
      if (ws?.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'input', data: b64encode(new TextEncoder().encode(data)) }))
      }
    })

    term.onBinary(data => {
      const ws = wsRef.current
      if (ws?.readyState === WebSocket.OPEN) {
        const bytes = new Uint8Array(data.length)
        for (let i = 0; i < data.length; i++) bytes[i] = data.charCodeAt(i)
        ws.send(JSON.stringify({ type: 'input', data: b64encode(bytes) }))
      }
    })

    return () => {
      wsRef.current?.close()
      term.dispose()
    }
  }, [sessionId])

  // Update terminal theme when it changes
  useEffect(() => {
    if (termRef.current) {
      termRef.current.options.theme = THEMES[theme]
    }
  }, [theme])

  // Connect WebSocket
  useEffect(() => {
    if (!termRef.current) return
    if (wsRef.current) return

    const ws = new WebSocket(wsUrl(`/ws/term/${sessionId}`))
    wsRef.current = ws

    ws.onopen = () => {
      const fit = fitRef.current
      if (fit) {
        const dims = fit.proposeDimensions()
        // Only send a resize when the container is actually laid out. A hidden
        // (display:none) terminal proposes a tiny size (~10x6) that would shrink
        // the shared tmux window and truncate other clients. The resize will be
        // sent later by the `active` effect once this terminal becomes visible.
        if (dims && isValidDims(dims.cols, dims.rows)) {
          ws.send(JSON.stringify({ type: 'resize', cols: dims.cols, rows: dims.rows }))
        }
      }
    }

    ws.onmessage = (evt) => {
      try {
        const msg = JSON.parse(evt.data)
        if (msg.type === 'output') {
          termRef.current?.write(b64decode(msg.data))
        }
      } catch { /* ignore */ }
    }

    ws.onclose = () => { wsRef.current = null }
    ws.onerror = () => { ws.close() }

    return () => { ws.close() }
  }, [sessionId])

  const handleResize = useCallback(() => {
    const fit = fitRef.current
    const term = termRef.current
    const ws = wsRef.current
    if (!fit || !term) return
    // Skip while the container is hidden/unlaid-out: FitAddon would collapse the
    // terminal to a tiny size and we'd push that to the shared tmux window.
    const dims = fit.proposeDimensions()
    if (!dims || !isValidDims(dims.cols, dims.rows)) return
    fit.fit()
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: 'resize', cols: term.cols, rows: term.rows }))
    }
  }, [])

  useEffect(() => {
    if (active) {
      const t = setTimeout(() => {
        handleResize()
        termRef.current?.focus()
      }, 50)
      return () => clearTimeout(t)
    }
  }, [active, handleResize])

  useEffect(() => {
    window.addEventListener('resize', handleResize)
    return () => window.removeEventListener('resize', handleResize)
  }, [handleResize])

  return (
    <div className="flex flex-col h-full">
      <div ref={containerRef} className="xterm-container w-full flex-1 min-h-0" />
      <div className="flex items-center gap-3 px-4 py-3 border-t border-[var(--border)] bg-[var(--bg-secondary)] min-h-[40px]">
        {status ? (
          <>
            <div className="flex items-center gap-1.5 text-xs text-[var(--text-secondary)]">
              <Folder size={13} className="shrink-0" />
              <span className="truncate max-w-[200px]" title={status.work_dir}>{status.work_dir}</span>
            </div>
            {status.is_git && (
              <>
                <div className="flex items-center gap-1.5 text-xs text-[var(--accent-purple)]">
                  <GitBranch size={13} className="shrink-0" />
                  <span>{status.git_branch}</span>
                </div>
                {status.git_dirty > 0 && (
                  <div className="flex items-center gap-1 text-xs text-[var(--accent-yellow)]">
                    <Circle size={8} className="fill-current shrink-0" />
                    <span>{status.git_dirty} changed</span>
                  </div>
                )}
              </>
            )}
          </>
        ) : (
          <span className="text-xs text-[var(--text-muted)]">Loading...</span>
        )}
        {ports.map(p => (
          <button
            key={p.port}
            title={`Open localhost:${p.port} preview`}
            className="flex items-center gap-1 text-xs text-[var(--accent-green-text,var(--accent-green))] hover:underline"
            onClick={async () => {
              try {
                // Exposing is idempotent: returns the existing slug URL if
                // this port was exposed before.
                const url = p.url ?? (await exposePort(sessionId, p.port)).url
                window.open(url, '_blank', 'noreferrer')
              } catch (e) {
                setTmuxError(String((e as Error).message || e))
              }
            }}
          >
            <Globe size={13} className="shrink-0" />
            <span>:{p.port}</span>
          </button>
        ))}
        <div className="flex items-center gap-1 ml-auto">
          {tmuxError && (
            <span className="text-xs text-[var(--accent-red)] truncate max-w-[240px]" title={tmuxError}>
              {tmuxError}
            </span>
          )}
          {TMUX_BUTTONS.map(({ action, title, Icon }) => (
            <button
              key={action}
              title={title}
              onClick={() => runTmuxAction(action)}
              className="p-1.5 rounded text-[var(--text-secondary)] hover:text-[var(--text-bright)] hover:bg-[var(--bg-hover)] transition-colors"
            >
              <Icon size={14} />
            </button>
          ))}
        </div>
      </div>
    </div>
  )
}
