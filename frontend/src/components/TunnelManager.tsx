import { useState, useEffect, useCallback } from 'react'
import { listTunnels, createTunnel, deleteTunnel, setTunnelShareable } from '../lib/api'
import type { Tunnel } from '../lib/api'
import { X, RefreshCw, Cable, Trash2, ExternalLink, Circle, Lock, LockOpen } from 'lucide-react'

interface Props {
  onClose: () => void
}

// ssh-tunnel-style port forwards: expose any local port (even services not
// started inside a ZeroMux session) at a stable slug URL.
export default function TunnelManager({ onClose }: Props) {
  const [tunnels, setTunnels] = useState<Tunnel[]>([])
  const [loading, setLoading] = useState(true)
  const [port, setPort] = useState('')
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      setTunnels(await listTunnels())
      setError(null)
    } catch { /* ignore */ }
    setLoading(false)
  }, [])

  useEffect(() => { load() }, [load])

  const handleCreate = async () => {
    const p = parseInt(port, 10)
    if (!p || p < 1024 || p > 65535) {
      setError('Port must be 1024-65535')
      return
    }
    try {
      await createTunnel(p, name.trim())
      setPort('')
      setName('')
      setError(null)
      load()
    } catch (e) {
      setError(String((e as Error).message || e))
    }
  }

  const handleDelete = async (t: Tunnel) => {
    if (!confirm(`Delete tunnel "${t.name || t.slug}"? The URL stops working immediately.`)) return
    try {
      await deleteTunnel(t.slug)
      setTunnels(prev => prev.filter(x => x.slug !== t.slug))
    } catch (e) {
      setError(String((e as Error).message || e))
    }
  }

  const handleToggleShare = async (t: Tunnel) => {
    if (!t.shareable && !confirm(
      `Make "${t.name || t.slug}" public?\n\nAnyone with the URL can access this service without logging in. The URL is unguessable, but treat it like a secret link.`
    )) return
    try {
      await setTunnelShareable(t.slug, !t.shareable)
      setTunnels(prev => prev.map(x => x.slug === t.slug ? { ...x, shareable: !t.shareable } : x))
      setError(null)
    } catch (e) {
      setError(String((e as Error).message || e))
    }
  }

  return (
    <div className="fixed inset-0 z-50 bg-[var(--bg-primary)]/95 backdrop-blur-sm flex flex-col">
      <div className="flex items-center justify-between px-6 h-12 border-b border-[var(--border)] shrink-0">
        <span className="text-sm font-semibold text-[var(--text-primary)]">Port Tunnels</span>
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

      <div className="flex-1 overflow-y-auto p-4 space-y-4 max-w-2xl mx-auto w-full">
        {/* Create form */}
        <div className="flex items-center gap-2">
          <input
            value={port}
            onChange={e => setPort(e.target.value.replace(/\D/g, ''))}
            onKeyDown={e => e.key === 'Enter' && handleCreate()}
            placeholder="Port"
            inputMode="numeric"
            className="w-24 text-sm bg-[var(--bg-secondary)] border border-[var(--border)] rounded px-2.5 py-1.5 text-[var(--text-primary)] outline-none focus:border-[var(--accent-blue)]"
          />
          <input
            value={name}
            onChange={e => setName(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && handleCreate()}
            placeholder="Name (optional)"
            className="flex-1 text-sm bg-[var(--bg-secondary)] border border-[var(--border)] rounded px-2.5 py-1.5 text-[var(--text-primary)] outline-none focus:border-[var(--accent-blue)]"
          />
          <button
            onClick={handleCreate}
            className="px-3 py-1.5 text-sm rounded bg-[var(--accent-blue)] text-white hover:bg-[var(--accent-blue-hover)] transition-colors"
          >
            Create
          </button>
        </div>
        {error && <div className="text-xs text-[var(--accent-red)]">{error}</div>}

        {/* Tunnel list */}
        {loading ? (
          <div className="py-4 text-center text-xs text-[var(--text-muted)]">Loading...</div>
        ) : tunnels.length === 0 ? (
          <div className="py-8 text-center text-xs text-[var(--text-muted)]">
            No tunnels. Create one to expose a local port at a public URL.
          </div>
        ) : (
          <div className="space-y-2">
            {tunnels.map(t => (
              <div
                key={t.slug}
                className="flex items-center gap-3 border border-[var(--border)] rounded-lg px-4 py-3 bg-[var(--bg-secondary)]"
              >
                <Cable size={16} className="text-[var(--text-muted)] shrink-0" />
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium text-[var(--text-primary)]">
                      {t.name || `port ${t.port}`}
                    </span>
                    <span className="text-xs text-[var(--text-muted)]">:{t.port}</span>
                    <Circle
                      size={8}
                      className={`shrink-0 fill-current ${t.listening ? 'text-[var(--accent-green)]' : 'text-[var(--text-muted)]'}`}
                    />
                    {!t.listening && (
                      <span className="text-xs text-[var(--text-muted)]">not listening</span>
                    )}
                    {t.shareable && (
                      <span className="text-xs text-[var(--accent-yellow)] font-medium">public link</span>
                    )}
                  </div>
                  <a
                    href={t.url}
                    target="_blank"
                    rel="noreferrer"
                    className="text-xs text-[var(--accent-blue)] hover:underline truncate block"
                  >
                    {t.url}
                  </a>
                </div>
                <button
                  onClick={() => handleToggleShare(t)}
                  title={t.shareable ? 'Public link — click to require login' : 'Login required — click to make a shareable public link'}
                  className={`p-1.5 rounded transition-colors ${t.shareable
                    ? 'text-[var(--accent-yellow)] hover:text-[var(--text-primary)]'
                    : 'text-[var(--text-secondary)] hover:text-[var(--accent-yellow)]'}`}
                >
                  {t.shareable ? <LockOpen size={14} /> : <Lock size={14} />}
                </button>
                <a
                  href={t.url}
                  target="_blank"
                  rel="noreferrer"
                  title="Open"
                  className="p-1.5 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors"
                >
                  <ExternalLink size={14} />
                </a>
                <button
                  onClick={() => handleDelete(t)}
                  title="Delete tunnel"
                  className="p-1.5 text-[var(--text-secondary)] hover:text-[var(--accent-red)] rounded transition-colors"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
