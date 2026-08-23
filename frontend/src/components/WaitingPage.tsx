import { useEffect } from 'react'
import { LogOut } from 'lucide-react'
import type { UserInfo } from '../lib/api'

interface Props {
  user: UserInfo
  onStatusChange: () => void
  onLogout: () => void
}

// Shown to signed-in but not-yet-approved users. Deliberately styled as a
// "coming soon" teaser rather than an approval queue — access is
// invite-only and we don't advertise the approval mechanics. Keeps polling
// so an admin approval still unlocks the app without a manual refresh.
export default function WaitingPage({ user, onStatusChange, onLogout }: Props) {
  useEffect(() => {
    const interval = setInterval(async () => {
      try {
        const res = await fetch('/api/me', { credentials: 'same-origin' })
        if (res.ok) {
          const data = await res.json()
          if (data.status === 'active') {
            onStatusChange()
          }
        }
      } catch { /* ignore */ }
    }, 5000)
    return () => clearInterval(interval)
  }, [onStatusChange])

  return (
    <div className="h-full bg-[var(--bg-primary)] flex items-center justify-center">
      <div className="text-center space-y-6 px-8">
        <div className="text-5xl font-bold tracking-tight text-[var(--text-primary)]">
          ZeroMux
        </div>
        <div className="space-y-2">
          <p className="text-xl text-[var(--accent-blue)] font-medium tracking-widest uppercase">
            Coming Soon
          </p>
          <p className="text-sm text-[var(--text-muted)] max-w-xs mx-auto leading-relaxed">
            ZeroMux is currently in private preview.
          </p>
        </div>
        <button
          onClick={onLogout}
          title={user.login}
          className="inline-flex items-center gap-1.5 text-xs text-[var(--text-muted)] hover:text-[var(--text-secondary)] transition-colors"
        >
          <LogOut size={12} />
          Sign out
        </button>
      </div>
    </div>
  )
}
