import { useState, useEffect, useCallback, useRef } from 'react'
import { listTmuxSessions, killTmuxSession, renameTmuxSession } from '../lib/api'
import type { TmuxSession } from '../lib/api'
import { X, RefreshCw, Terminal, Trash2, Pencil, Check, Link } from 'lucide-react'

interface Props {
  onClose: () => void
  onAttach?: (tmuxTarget: string) => void
}

export default function TmuxManager({ onClose, onAttach }: Props) {
  const [sessions, setSessions] = useState<TmuxSession[]>([])
  const [loading, setLoading] = useState(true)
  const [renamingName, setRenamingName] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const renameRef = useRef<HTMLInputElement>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const data = await listTmuxSessions()
      setSessions(data)
    } catch { /* ignore */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  useEffect(() => {
    if (renamingName && renameRef.current) {
      renameRef.current.focus()
      renameRef.current.select()
    }
  }, [renamingName])

  const handleKill = async (name: string) => {
    if (!confirm(`Kill tmux session "${name}"? This will terminate all processes inside.`)) return
    try {
      await killTmuxSession(name)
      setSessions(prev => prev.filter(s => s.name !== name))
    } catch (e: any) {
      alert(`Kill failed: ${e.message}`)
    }
  }

  const handleRenameStart = (name: string) => {
    setRenamingName(name)
    setRenameValue(name)
  }

  const handleRenameSubmit = async () => {
    if (!renamingName || !renameValue.trim() || renameValue.trim() === renamingName) {
      setRenamingName(null)
      return
    }
    try {
      await renameTmuxSession(renamingName, renameValue.trim())
      setSessions(prev => prev.map(s => s.name === renamingName ? { ...s, name: renameValue.trim() } : s))
    } catch (e: any) {
      alert(`Rename failed: ${e.message}`)
    }
    setRenamingName(null)
  }

  const formatCreated = (ts: number) => {
    if (!ts) return ''
    const d = new Date(ts * 1000)
    const mo = String(d.getMonth() + 1).padStart(2, '0')
    const day = String(d.getDate()).padStart(2, '0')
    const h = String(d.getHours()).padStart(2, '0')
    const m = String(d.getMinutes()).padStart(2, '0')
    return `${mo}-${day} ${h}:${m}`
  }

  return (
    <div className="fixed inset-0 z-50 bg-[var(--bg-primary)]/95 backdrop-blur-sm flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-6 h-12 border-b border-[var(--border)] shrink-0">
        <span className="text-sm font-semibold text-[var(--text-primary)]">Tmux Sessions</span>
        <div className="flex items-center gap-1">
          <button
            onClick={load}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Refresh"
          >
            <RefreshCw size={12} />
          </button>
          <button
            onClick={onClose}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
            title="Close"
          >
            <X size={14} />
          </button>
        </div>
      </div>

      {/* Session list - card grid */}
      <div className="flex-1 overflow-y-auto p-4">
        {loading ? (
          <div className="py-4 text-center text-xs text-[var(--text-muted)]">Loading...</div>
        ) : sessions.length === 0 ? (
          <div className="py-4 text-center text-xs text-[var(--text-muted)]">No tmux sessions</div>
        ) : (
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 gap-2">
            {sessions.map(s => (
              <div
                key={s.name}
                className="border border-[var(--border)] rounded-lg p-3 bg-[var(--bg-secondary)] hover:border-[var(--accent-blue)]/50 hover:bg-[var(--bg-tertiary)] transition-colors group"
              >
                {renamingName === s.name ? (
                  <div className="flex items-center gap-1">
                    <input
                      ref={renameRef}
                      value={renameValue}
                      onChange={e => setRenameValue(e.target.value)}
                      onKeyDown={e => {
                        if (e.key === 'Enter') handleRenameSubmit()
                        if (e.key === 'Escape') setRenamingName(null)
                      }}
                      onBlur={handleRenameSubmit}
                      className="flex-1 text-xs bg-[var(--bg-secondary)] border border-[var(--accent-blue)] rounded px-1.5 py-0.5 text-[var(--text-primary)] outline-none"
                    />
                    <button onClick={handleRenameSubmit} className="p-0.5 text-[var(--accent-green-text)]">
                      <Check size={12} />
                    </button>
                  </div>
                ) : (
                  <>
                    <Terminal size={16} className="text-[var(--text-muted)] mb-1.5" />
                    <div className="text-[11px] font-medium text-[var(--text-primary)] truncate mb-1">{s.name}</div>
                    <div className="flex items-center gap-1 text-[9px] text-[var(--text-muted)] mb-1.5">
                      <span className={`inline-block w-1.5 h-1.5 rounded-full ${s.attached > 0 ? 'bg-green-400' : 'bg-gray-400'}`} />
                      <span>{s.attached > 0 ? 'attached' : 'detached'}</span>
                    </div>
                    <div className="text-[9px] text-[var(--text-muted)]">{formatCreated(s.created)}</div>
                    <div className="flex items-center gap-1 mt-2 md:opacity-0 md:group-hover:opacity-100 transition-opacity">
                      {onAttach && (
                        <button
                          onClick={() => { onAttach(s.name); onClose() }}
                          className="px-1.5 py-0.5 text-[9px] text-[var(--text-secondary)] hover:text-[var(--accent-green-text)] border border-[var(--border)] rounded transition-colors"
                          title="Attach"
                        >
                          <Link size={9} className="inline mr-0.5" />attach
                        </button>
                      )}
                      <button
                        onClick={() => handleRenameStart(s.name)}
                        className="px-1.5 py-0.5 text-[9px] text-[var(--text-secondary)] hover:text-[var(--accent-blue)] border border-[var(--border)] rounded transition-colors"
                        title="Rename"
                      >
                        <Pencil size={9} className="inline mr-0.5" />rename
                      </button>
                      <button
                        onClick={() => handleKill(s.name)}
                        className="px-1.5 py-0.5 text-[9px] text-[var(--text-secondary)] hover:text-[var(--accent-red)] border border-[var(--border)] rounded transition-colors"
                        title="Kill"
                      >
                        <Trash2 size={9} className="inline mr-0.5" />kill
                      </button>
                    </div>
                  </>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Footer */}
      <div className="px-6 py-2 border-t border-[var(--border)] text-[10px] text-[var(--text-muted)]">
        {sessions.length} session{sessions.length !== 1 ? 's' : ''} · Sidebar X = detach · Kill here = terminate
      </div>
    </div>
  )
}
