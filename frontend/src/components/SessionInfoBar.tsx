import { useState, useCallback } from 'react'
import type { SessionInfo, SessionMetaStatus } from '../lib/api'
import { updateSession } from '../lib/api'
import { ChevronDown, ChevronRight, FileText, StickyNote } from 'lucide-react'

interface Props {
  session: SessionInfo
  onUpdate: (updated: Partial<SessionInfo>) => void
  onToggleFiles: () => void
  showFiles: boolean
}

const STATUS_OPTIONS: { value: SessionMetaStatus; label: string; color: string }[] = [
  { value: 'running', label: 'Running', color: 'bg-green-500' },
  { value: 'done', label: 'Done', color: 'bg-blue-500' },
  { value: 'blocked', label: 'Blocked', color: 'bg-yellow-500' },
  { value: 'idle', label: 'Idle', color: 'bg-gray-400' },
]

export function StatusDot({ status }: { status: SessionMetaStatus }) {
  const opt = STATUS_OPTIONS.find(o => o.value === status)
  return <span className={`inline-block w-2 h-2 rounded-full ${opt?.color || 'bg-gray-400'} shrink-0`} />
}

export default function SessionInfoBar({ session, onUpdate, onToggleFiles, showFiles }: Props) {
  const [expanded, setExpanded] = useState(false)
  const [desc, setDesc] = useState(session.description)
  const [notes, setNotes] = useState(session.notes)

  const save = useCallback(async (data: { description?: string; status?: SessionMetaStatus; notes?: string }) => {
    try {
      await updateSession(session.id, data)
      onUpdate(data)
    } catch { /* ignore */ }
  }, [session.id, onUpdate])

  const handleDescBlur = () => {
    if (desc !== session.description) {
      save({ description: desc })
    }
  }

  const handleNotesBlur = () => {
    if (notes !== session.notes) {
      save({ notes })
    }
  }

  const handleStatusChange = (status: SessionMetaStatus) => {
    save({ status })
  }

  // Sync from props when session changes
  if (desc !== session.description && document.activeElement?.tagName !== 'INPUT') {
    setDesc(session.description)
  }
  if (notes !== session.notes && document.activeElement?.tagName !== 'TEXTAREA') {
    setNotes(session.notes)
  }

  return (
    <div className="border-b border-[var(--border)] bg-[var(--bg-secondary)]">
      {/* Collapsed bar */}
      <div className="flex items-center gap-2 px-3 h-9">
        <button
          onClick={() => setExpanded(!expanded)}
          className="p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-primary)] transition-colors"
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>

        <StatusDot status={session.status} />

        {!expanded ? (
          <span className="text-xs text-[var(--text-secondary)] truncate flex-1">
            {session.description || session.name}
          </span>
        ) : (
          <input
            value={desc}
            onChange={e => setDesc(e.target.value)}
            onBlur={handleDescBlur}
            placeholder="What is this session doing?"
            className="text-xs text-[var(--text-primary)] bg-transparent flex-1 outline-none placeholder-[var(--text-muted)]"
          />
        )}

        <div className="flex items-center gap-1 shrink-0">
          <button
            onClick={onToggleFiles}
            className={`p-1 rounded transition-colors ${
              showFiles
                ? 'text-[var(--accent-blue)] bg-[var(--bg-primary)]'
                : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'
            }`}
            title="Browse files"
          >
            <FileText size={14} />
          </button>
        </div>
      </div>

      {/* Expanded panel */}
      {expanded && (
        <div className="px-3 pb-2 space-y-2">
          {/* Status selector */}
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-[var(--text-muted)] uppercase w-12">Status</span>
            <div className="flex gap-1">
              {STATUS_OPTIONS.map(opt => (
                <button
                  key={opt.value}
                  onClick={() => handleStatusChange(opt.value)}
                  className={`px-2 py-0.5 text-[10px] rounded-full border transition-colors ${
                    session.status === opt.value
                      ? 'border-[var(--accent-blue)] text-[var(--accent-blue)] bg-[var(--bg-primary)]'
                      : 'border-[var(--border)] text-[var(--text-secondary)] hover:border-[var(--text-muted)]'
                  }`}
                >
                  {opt.label}
                </button>
              ))}
            </div>
          </div>

          {/* Notes */}
          <div>
            <div className="flex items-center gap-1 mb-1">
              <StickyNote size={10} className="text-[var(--text-muted)]" />
              <span className="text-[10px] text-[var(--text-muted)] uppercase">Notes</span>
            </div>
            <textarea
              value={notes}
              onChange={e => setNotes(e.target.value)}
              onBlur={handleNotesBlur}
              placeholder="Add notes for yourself..."
              rows={2}
              className="w-full text-xs bg-[var(--bg-primary)] border border-[var(--border)] rounded-md px-2 py-1.5 text-[var(--text-primary)] outline-none focus:border-[var(--accent-blue)] placeholder-[var(--text-muted)] resize-y"
            />
          </div>
        </div>
      )}
    </div>
  )
}
