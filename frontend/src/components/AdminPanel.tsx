import { useState, useEffect, useCallback } from 'react'
import { Users, Check, Trash2, X, Shield, Clock } from 'lucide-react'
import type { AdminUser } from '../lib/api'
import { listUsers, approveUser, removeUser } from '../lib/api'

interface Props {
  onClose: () => void
}

export default function AdminPanel({ onClose }: Props) {
  const [users, setUsers] = useState<AdminUser[]>([])
  const [loading, setLoading] = useState(true)

  const load = useCallback(async () => {
    try {
      const data = await listUsers()
      setUsers(data)
    } catch { /* ignore */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  const handleApprove = async (id: string) => {
    try {
      await approveUser(id)
      load()
    } catch { /* ignore */ }
  }

  const handleRemove = async (id: string) => {
    try {
      await removeUser(id)
      load()
    } catch { /* ignore */ }
  }

  const pending = users.filter(u => u.status === 'pending')
  const active = users.filter(u => u.status === 'active')

  return (
    <div className="absolute inset-0 bg-[var(--bg-primary)] z-50 flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 h-10 border-b border-[var(--border)] bg-[var(--bg-secondary)]">
        <div className="flex items-center gap-2 text-xs font-bold text-[var(--text-primary)]">
          <Users size={14} />
          User Management
        </div>
        <button
          onClick={onClose}
          className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
        >
          <X size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {loading ? (
          <div className="text-sm text-[var(--text-muted)]">Loading...</div>
        ) : (
          <>
            {/* Pending users */}
            {pending.length > 0 && (
              <div>
                <h3 className="text-xs font-semibold text-[var(--accent-yellow)] uppercase tracking-wider mb-2 flex items-center gap-1.5">
                  <Clock size={12} />
                  Pending Approval ({pending.length})
                </h3>
                <div className="space-y-1">
                  {pending.map(u => (
                    <UserRow key={u.id} user={u} onApprove={handleApprove} onRemove={handleRemove} />
                  ))}
                </div>
              </div>
            )}

            {/* Active users */}
            <div>
              <h3 className="text-xs font-semibold text-[var(--accent-green-text)] uppercase tracking-wider mb-2 flex items-center gap-1.5">
                <Shield size={12} />
                Active Users ({active.length})
              </h3>
              <div className="space-y-1">
                {active.map(u => (
                  <UserRow key={u.id} user={u} onRemove={handleRemove} />
                ))}
              </div>
            </div>
          </>
        )}
      </div>
    </div>
  )
}

function UserRow({ user, onApprove, onRemove }: {
  user: AdminUser
  onApprove?: (id: string) => void
  onRemove: (id: string) => void
}) {
  return (
    <div className="flex items-center gap-3 px-3 py-2 bg-[var(--bg-secondary)] rounded-lg border border-[var(--border)]">
      {user.avatar_url ? (
        <img src={user.avatar_url} alt="" className="w-7 h-7 rounded-full shrink-0" />
      ) : (
        <div className="w-7 h-7 rounded-full bg-[var(--bg-tertiary)] shrink-0" />
      )}
      <div className="flex-1 min-w-0">
        <div className="text-xs font-medium text-[var(--text-primary)] truncate">
          {user.github_login}
          {user.role === 'admin' && (
            <span className="ml-1.5 text-[10px] text-[var(--accent-purple)] font-normal">admin</span>
          )}
        </div>
        {user.display_name && (
          <div className="text-[10px] text-[var(--text-muted)] truncate">{user.display_name}</div>
        )}
      </div>
      <div className="flex items-center gap-1 shrink-0">
        {onApprove && user.status === 'pending' && (
          <button
            onClick={() => onApprove(user.id)}
            className="p-1 text-[var(--accent-green-text)] hover:bg-[var(--bg-tertiary)] rounded transition-colors"
            title="Approve"
          >
            <Check size={14} />
          </button>
        )}
        {user.role !== 'admin' && (
          <button
            onClick={() => onRemove(user.id)}
            className="p-1 text-[var(--text-secondary)] hover:text-[var(--accent-red)] hover:bg-[var(--bg-tertiary)] rounded transition-colors"
            title="Remove"
          >
            <Trash2 size={12} />
          </button>
        )}
      </div>
    </div>
  )
}
