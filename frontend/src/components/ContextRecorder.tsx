import { useState, useEffect, useCallback } from 'react'
import { Circle, Square, FileText, RefreshCw, Download, X, ArrowLeft, Trash2 } from 'lucide-react'

interface Props {
  sessionId: string
}

interface ContextFile {
  name: string
  path: string
  size: number
  modified: number
}

async function api(path: string, opts: RequestInit = {}): Promise<Response> {
  const token = localStorage.getItem('zeromux_token') || ''
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    ...(opts.headers as Record<string, string> || {}),
  }
  if (token) headers['Authorization'] = `Bearer ${token}`
  return fetch(path, { ...opts, headers, credentials: 'same-origin' })
}

export default function ContextRecorder({ sessionId }: Props) {
  const [recording, setRecording] = useState(false)
  const [recordingFile, setRecordingFile] = useState<string | null>(null)
  const [files, setFiles] = useState<ContextFile[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [viewing, setViewing] = useState<{ name: string; content: string } | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [showDeleteModal, setShowDeleteModal] = useState(false)
  const [deleting, setDeleting] = useState(false)

  const loadStatus = useCallback(async () => {
    try {
      const res = await api(`/api/sessions/${sessionId}/record/status`)
      if (res.ok) {
        const data = await res.json()
        setRecording(data.recording)
        setRecordingFile(data.file || null)
      }
    } catch { /* ignore */ }
  }, [sessionId])

  const loadFiles = useCallback(async () => {
    setLoading(true)
    try {
      const res = await api(`/api/sessions/${sessionId}/context/files`)
      if (res.ok) {
        const data = await res.json()
        setFiles(data.files || [])
      }
    } catch { /* ignore */ }
    setLoading(false)
  }, [sessionId])

  useEffect(() => {
    loadStatus()
    loadFiles()
  }, [loadStatus, loadFiles])

  const handleStart = async () => {
    setError(null)
    try {
      const res = await api(`/api/sessions/${sessionId}/record/start`, { method: 'POST' })
      if (!res.ok) {
        const text = await res.text()
        setError(text)
        return
      }
      const data = await res.json()
      setRecording(true)
      setRecordingFile(data.file || null)
    } catch (e: any) {
      setError(e.message)
    }
  }

  const handleStop = async () => {
    setError(null)
    try {
      const res = await api(`/api/sessions/${sessionId}/record/stop`, { method: 'POST' })
      if (!res.ok) {
        const text = await res.text()
        setError(text)
        return
      }
      setRecording(false)
      setRecordingFile(null)
      loadFiles()
    } catch (e: any) {
      setError(e.message)
    }
  }

  const handleView = async (name: string) => {
    try {
      const res = await api(`/api/sessions/${sessionId}/context/file?name=${encodeURIComponent(name)}`)
      if (res.ok) {
        const data = await res.json()
        setViewing({ name: data.name, content: data.content })
      }
    } catch { /* ignore */ }
  }

  const handleDownload = (name: string) => {
    const token = localStorage.getItem('zeromux_token') || ''
    const url = `/api/sessions/${sessionId}/context/file/download?name=${encodeURIComponent(name)}&token=${encodeURIComponent(token)}`
    window.open(url, '_blank')
  }

  const toggleSelect = (name: string) => {
    setSelected(prev => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  const toggleSelectAll = () => {
    if (selected.size === files.length) {
      setSelected(new Set())
    } else {
      setSelected(new Set(files.map(f => f.name)))
    }
  }

  const handleDeleteConfirm = async () => {
    if (selected.size === 0) return
    setDeleting(true)
    try {
      const res = await api(`/api/sessions/${sessionId}/context/files/delete`, {
        method: 'POST',
        body: JSON.stringify({ names: [...selected] }),
      })
      if (res.ok) {
        setFiles(prev => prev.filter(f => !selected.has(f.name)))
        setSelected(new Set())
        setShowDeleteModal(false)
      }
    } catch { /* ignore */ }
    setDeleting(false)
  }

  const formatSize = (bytes: number): string => {
    if (bytes < 1024) return `${bytes} B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  const formatTime = (ts: number): string => {
    if (!ts) return ''
    const d = new Date(ts * 1000)
    const mo = String(d.getMonth() + 1).padStart(2, '0')
    const day = String(d.getDate()).padStart(2, '0')
    const h = String(d.getHours()).padStart(2, '0')
    const m = String(d.getMinutes()).padStart(2, '0')
    return `${mo}-${day} ${h}:${m}`
  }

  // File viewer mode
  if (viewing) {
    return (
      <div className="flex flex-col h-full">
        <div className="flex items-center gap-2 px-3 h-9 border-b border-[var(--border)] bg-[var(--bg-secondary)] shrink-0">
          <button
            onClick={() => setViewing(null)}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Back"
          >
            <ArrowLeft size={12} />
          </button>
          <span className="text-[10px] font-mono text-[var(--text-primary)] truncate flex-1">
            {viewing.name}
          </span>
          <button
            onClick={() => handleDownload(viewing.name)}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Download"
          >
            <Download size={12} />
          </button>
          <button
            onClick={() => setViewing(null)}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Close"
          >
            <X size={12} />
          </button>
        </div>
        <div className="flex-1 overflow-auto p-3">
          <pre className="text-[11px] text-[var(--text-primary)] font-mono whitespace-pre-wrap break-words leading-relaxed">
            {viewing.content || '(empty)'}
          </pre>
        </div>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-3 h-9 border-b border-[var(--border)] bg-[var(--bg-secondary)] shrink-0">
        <span className="text-[10px] font-semibold text-[var(--text-muted)] uppercase tracking-wider">
          Context Recorder
        </span>
        <div className="flex items-center gap-1">
          {selected.size > 0 && (
            <button
              onClick={() => setShowDeleteModal(true)}
              className="p-1 text-red-400 hover:text-red-300 rounded transition-colors"
              title={`Delete ${selected.size} file(s)`}
            >
              <Trash2 size={12} />
            </button>
          )}
          <button
            onClick={loadFiles}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Refresh"
          >
            <RefreshCw size={12} />
          </button>
        </div>
      </div>

      {/* Record controls */}
      <div className="px-3 py-3 border-b border-[var(--border)]">
        {!recording ? (
          <button
            onClick={handleStart}
            className="flex items-center gap-2 px-3 py-1.5 bg-red-500/15 hover:bg-red-500/25 text-red-400 rounded-md text-xs font-medium transition-colors w-full justify-center"
          >
            <Circle size={12} fill="currentColor" />
            Start Recording
          </button>
        ) : (
          <div className="space-y-2">
            <button
              onClick={handleStop}
              className="flex items-center gap-2 px-3 py-1.5 bg-[var(--bg-tertiary)] hover:bg-[var(--border)] text-[var(--text-primary)] rounded-md text-xs font-medium transition-colors w-full justify-center"
            >
              <Square size={12} fill="currentColor" />
              Stop Recording
            </button>
            <div className="flex items-center gap-1.5">
              <span className="w-2 h-2 rounded-full bg-red-500 animate-pulse" />
              <span className="text-[10px] text-red-400">Recording...</span>
            </div>
            {recordingFile && (
              <p className="text-[9px] text-[var(--text-muted)] font-mono truncate">
                {recordingFile.split('/').pop()}
              </p>
            )}
          </div>
        )}
        {error && (
          <p className="text-[10px] text-red-400 mt-1">{error}</p>
        )}
      </div>

      {/* Context files list */}
      <div className="flex-1 overflow-y-auto">
        {loading && files.length === 0 ? (
          <div className="p-4 text-center text-[10px] text-[var(--text-muted)]">Loading...</div>
        ) : files.length === 0 ? (
          <div className="p-6 text-center">
            <FileText size={24} className="mx-auto text-[var(--text-muted)] mb-2" />
            <p className="text-[11px] text-[var(--text-muted)]">No context files yet</p>
            <p className="text-[9px] text-[var(--text-muted)] mt-1">
              Press Record to capture terminal output
            </p>
          </div>
        ) : (
          <>
            {/* Select all */}
            {files.length > 1 && (
              <div className="flex items-center gap-2 px-3 py-1 border-b border-[var(--border)] bg-[var(--bg-primary)]">
                <input
                  type="checkbox"
                  checked={selected.size === files.length}
                  onChange={toggleSelectAll}
                  className="w-3 h-3 rounded accent-[var(--accent-blue)]"
                />
                <span className="text-[9px] text-[var(--text-muted)]">
                  {selected.size > 0 ? `${selected.size} selected` : 'Select all'}
                </span>
              </div>
            )}
            <div className="divide-y divide-[var(--border)]">
              {files.map(file => (
                <div
                  key={file.path}
                  className="flex items-center gap-2 px-3 py-2 hover:bg-[var(--bg-tertiary)] transition-colors group cursor-pointer"
                  onDoubleClick={() => handleView(file.name)}
                >
                  <input
                    type="checkbox"
                    checked={selected.has(file.name)}
                    onChange={() => toggleSelect(file.name)}
                    onClick={e => e.stopPropagation()}
                    className="w-3 h-3 rounded accent-[var(--accent-blue)] shrink-0"
                  />
                  <FileText size={12} className="text-[var(--text-muted)] shrink-0" />
                  <div className="flex-1 min-w-0">
                    <p className="text-[11px] text-[var(--text-primary)] font-mono truncate">
                      {file.name}
                    </p>
                    <p className="text-[9px] text-[var(--text-muted)]">
                      {formatSize(file.size)} · {formatTime(file.modified)}
                    </p>
                  </div>
                  <div className="flex items-center gap-0.5 shrink-0 opacity-0 group-hover:opacity-100 md:opacity-0 md:group-hover:opacity-100 transition-opacity">
                    <button
                      onClick={(e) => { e.stopPropagation(); handleDownload(file.name) }}
                      className="p-1 text-[var(--text-secondary)] hover:text-[var(--accent-green-text)] rounded transition-colors"
                      title="Download"
                    >
                      <Download size={12} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </>
        )}
      </div>

      {/* Footer hint */}
      <div className="px-3 py-1.5 border-t border-[var(--border)] bg-[var(--bg-secondary)]">
        <span className="text-[9px] text-[var(--text-muted)]">
          Tell agent: read .zeromux/context/&lt;filename&gt;
        </span>
      </div>

      {/* Delete confirmation modal */}
      {showDeleteModal && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-lg p-4 mx-4 max-w-sm w-full shadow-xl">
            <h3 className="text-sm font-medium text-[var(--text-primary)] mb-2">
              Delete {selected.size} file{selected.size > 1 ? 's' : ''}?
            </h3>
            <div className="max-h-32 overflow-y-auto mb-3">
              {[...selected].map(name => (
                <p key={name} className="text-[10px] text-[var(--text-secondary)] font-mono truncate py-0.5">
                  {name}
                </p>
              ))}
            </div>
            <p className="text-[10px] text-[var(--text-muted)] mb-3">
              This action cannot be undone.
            </p>
            <div className="flex items-center justify-end gap-2">
              <button
                onClick={() => setShowDeleteModal(false)}
                disabled={deleting}
                className="px-3 py-1.5 text-xs text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded-md transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleDeleteConfirm}
                disabled={deleting}
                className="px-3 py-1.5 text-xs bg-red-500/15 hover:bg-red-500/25 text-red-400 rounded-md font-medium transition-colors"
              >
                {deleting ? 'Deleting...' : 'Delete'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
