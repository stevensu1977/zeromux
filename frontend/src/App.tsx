import { useState, useEffect, useCallback } from 'react'
import type { SessionInfo, SessionType, UserInfo } from './lib/api'
import { listSessions, createSession, deleteSession, checkAuth, legacyLogin, clearAuth } from './lib/api'
import { useTheme } from './lib/theme'
import Sidebar from './components/Sidebar'
import TerminalView from './components/TerminalView'
import AcpChatView from './components/AcpChatView'
import LoginPage from './components/LoginPage'
import WaitingPage from './components/WaitingPage'
import SessionInfoBar from './components/SessionInfoBar'
import MarkdownViewer from './components/MarkdownViewer'

type AuthState = 'loading' | 'unauthenticated' | 'pending' | 'active'

export default function App() {
  const [authState, setAuthState] = useState<AuthState>('loading')
  const [user, setUser] = useState<UserInfo | null>(null)
  const [sessions, setSessions] = useState<SessionInfo[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [showFiles, setShowFiles] = useState<Record<string, boolean>>({})
  const themeCtx = useTheme()

  const initAuth = useCallback(async () => {
    const me = await checkAuth()
    if (me) {
      setUser(me)
      if (me.status === 'active') {
        setAuthState('active')
        loadSessions()
      } else {
        setAuthState('pending')
      }
    } else {
      setAuthState('unauthenticated')
    }
  }, [])

  useEffect(() => { initAuth() }, [initAuth])

  const loadSessions = useCallback(async () => {
    try {
      const list = await listSessions()
      setSessions(list)
      if (list.length > 0) {
        setActiveId(prev => prev && list.some(s => s.id === prev) ? prev : list[0].id)
      }
    } catch {
      setAuthState('unauthenticated')
    }
  }, [])

  const handleLegacyLogin = useCallback(async (password: string) => {
    const userInfo = await legacyLogin(password)
    setUser(userInfo)
    setAuthState('active')
    const list = await listSessions()
    setSessions(list)
    if (list.length === 0) {
      const s = await createSession('tmux')
      setSessions([s])
      setActiveId(s.id)
    } else {
      setActiveId(list[0].id)
    }
  }, [])

  const handleCreate = useCallback(async (type: SessionType, workDir?: string) => {
    const s = await createSession(type, undefined, workDir)
    setSessions(prev => [...prev, s])
    setActiveId(s.id)
  }, [])

  const handleLogout = useCallback(() => {
    clearAuth()
    setAuthState('unauthenticated')
    setUser(null)
    setSessions([])
    setActiveId(null)
  }, [])

  const handleDelete = useCallback(async (id: string) => {
    await deleteSession(id)
    setSessions(prev => {
      const next = prev.filter(s => s.id !== id)
      if (activeId === id) {
        setActiveId(next.length > 0 ? next[0].id : null)
      }
      return next
    })
  }, [activeId])

  const handleApproved = useCallback(() => {
    setAuthState('active')
    if (user) setUser({ ...user, status: 'active' })
    loadSessions()
  }, [user, loadSessions])

  const handleSessionUpdate = useCallback((id: string, updated: Partial<SessionInfo>) => {
    setSessions(prev => prev.map(s => s.id === id ? { ...s, ...updated } : s))
  }, [])

  const toggleFiles = useCallback((id: string) => {
    setShowFiles(prev => ({ ...prev, [id]: !prev[id] }))
  }, [])

  if (authState === 'loading') {
    return <div className="h-full bg-[var(--bg-primary)]" />
  }

  if (authState === 'unauthenticated') {
    return <LoginPage onLegacyLogin={handleLegacyLogin} />
  }

  if (authState === 'pending' && user) {
    return <WaitingPage user={user} onStatusChange={handleApproved} onLogout={handleLogout} />
  }

  const activeSession = sessions.find(s => s.id === activeId)

  return (
    <div className="h-full flex bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        onSelect={setActiveId}
        onCreate={handleCreate}
        onDelete={handleDelete}
        onLogout={handleLogout}
        theme={themeCtx.theme}
        onToggleTheme={themeCtx.toggle}
        user={user}
      />
      <main className="flex-1 min-w-0 flex flex-col">
        {/* Info bar for active session */}
        {activeSession && (
          <SessionInfoBar
            key={activeSession.id}
            session={activeSession}
            onUpdate={(updated) => handleSessionUpdate(activeSession.id, updated)}
            onToggleFiles={() => toggleFiles(activeSession.id)}
            showFiles={!!showFiles[activeSession.id]}
          />
        )}

        {/* Main content area */}
        <div className="flex-1 min-h-0 relative">
          {sessions.map(s => (
            <div key={s.id} className={`absolute inset-0 ${s.id === activeId ? '' : 'hidden'}`}>
              {/* Always keep terminal/chat mounted, hide with CSS when file viewer is active */}
              <div className={`h-full ${showFiles[s.id] ? 'hidden' : ''}`}>
                {s.type === 'tmux' ? (
                  <TerminalView sessionId={s.id} active={s.id === activeId && !showFiles[s.id]} theme={themeCtx.theme} />
                ) : (
                  <AcpChatView sessionId={s.id} active={s.id === activeId && !showFiles[s.id]} agentType={s.type} />
                )}
              </div>
              {showFiles[s.id] && <MarkdownViewer sessionId={s.id} />}
            </div>
          ))}
          {sessions.length === 0 && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)] text-sm">
              Create a session to get started
            </div>
          )}
        </div>
      </main>
    </div>
  )
}
