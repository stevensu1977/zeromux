import { useState, useEffect, useCallback, useRef, useMemo } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { markdownComponents } from './markdownStyles'
import hljs from 'highlight.js/lib/core'
import rust from 'highlight.js/lib/languages/rust'
import typescript from 'highlight.js/lib/languages/typescript'
import javascript from 'highlight.js/lib/languages/javascript'
import python from 'highlight.js/lib/languages/python'
import go from 'highlight.js/lib/languages/go'
import java from 'highlight.js/lib/languages/java'
import cpp from 'highlight.js/lib/languages/cpp'
import c from 'highlight.js/lib/languages/c'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import yaml from 'highlight.js/lib/languages/yaml'
import css from 'highlight.js/lib/languages/css'
import xml from 'highlight.js/lib/languages/xml'
import sql from 'highlight.js/lib/languages/sql'
import ruby from 'highlight.js/lib/languages/ruby'
import php from 'highlight.js/lib/languages/php'
import lua from 'highlight.js/lib/languages/lua'
import dockerfile from 'highlight.js/lib/languages/dockerfile'
import markdown from 'highlight.js/lib/languages/markdown'
import 'highlight.js/styles/github-dark.css'

hljs.registerLanguage('rust', rust)
hljs.registerLanguage('typescript', typescript)
hljs.registerLanguage('javascript', javascript)
hljs.registerLanguage('python', python)
hljs.registerLanguage('go', go)
hljs.registerLanguage('java', java)
hljs.registerLanguage('cpp', cpp)
hljs.registerLanguage('c', c)
hljs.registerLanguage('bash', bash)
hljs.registerLanguage('json', json)
hljs.registerLanguage('yaml', yaml)
hljs.registerLanguage('css', css)
hljs.registerLanguage('xml', xml)
hljs.registerLanguage('sql', sql)
hljs.registerLanguage('ruby', ruby)
hljs.registerLanguage('php', php)
hljs.registerLanguage('lua', lua)
hljs.registerLanguage('dockerfile', dockerfile)
hljs.registerLanguage('markdown', markdown)
import {
  listSessionTree, getSessionFile, writeSessionFile, deleteSessionFile,
  renameSessionFile, uploadSessionFile, createSessionDir, deleteSessionDir,
  downloadFileUrl
} from '../lib/api'
import type { TreeEntry, SessionType } from '../lib/api'
import {
  FileText, RefreshCw, ChevronRight, ChevronDown, Folder, Plus, Trash2, Pencil,
  Upload, FolderPlus, Save, X, Check, Eye, Edit3, Settings, Image, Download, File
} from 'lucide-react'

interface Props {
  sessionId: string
  sessionType?: SessionType
}

const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico'])
const CODE_EXTENSIONS = new Set(['rs', 'ts', 'tsx', 'js', 'jsx', 'py', 'go', 'java', 'c', 'cpp', 'h', 'sh', 'bash', 'zsh', 'toml', 'yaml', 'yml', 'json', 'css', 'html', 'sql', 'lua', 'rb', 'php', 'dockerfile'])

const EXT_TO_LANG: Record<string, string> = {
  rs: 'rust', ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript',
  py: 'python', go: 'go', java: 'java', c: 'c', cpp: 'cpp', h: 'c',
  sh: 'bash', bash: 'bash', zsh: 'bash',
  toml: 'yaml', yaml: 'yaml', yml: 'yaml',
  json: 'json', css: 'css', html: 'xml', xml: 'xml',
  sql: 'sql', lua: 'lua', rb: 'ruby', php: 'php',
  dockerfile: 'dockerfile', md: 'markdown',
}

function getLangForFile(path: string): string | undefined {
  const name = path.split('/').pop()?.toLowerCase() || ''
  if (name === 'dockerfile') return 'dockerfile'
  const ext = name.split('.').pop() || ''
  return EXT_TO_LANG[ext]
}

function isImageFile(name: string): boolean {
  const ext = name.split('.').pop()?.toLowerCase() || ''
  return IMAGE_EXTENSIONS.has(ext)
}

function getFileIcon(name: string) {
  if (isImageFile(name)) return Image
  const ext = name.split('.').pop()?.toLowerCase() || ''
  if (CODE_EXTENSIONS.has(ext)) return FileText
  return File
}

export default function FileBrowser({ sessionId, sessionType }: Props) {
  const docsBaseDirKey = `zeromux_docs_basedir_${sessionId}`

  // Tree state
  const [expanded, setExpanded] = useState<Set<string>>(new Set(['.']))
  const [children, setChildren] = useState<Record<string, TreeEntry[]>>({})
  const [loadingDirs, setLoadingDirs] = useState<Set<string>>(new Set())
  const [treeError, setTreeError] = useState<string | null>(null)

  // Selection & preview
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [content, setContent] = useState<string>('')
  const [isBinary, setIsBinary] = useState(false)
  const [loadingContent, setLoadingContent] = useState(false)

  // Editing
  const [editing, setEditing] = useState(false)
  const [editContent, setEditContent] = useState('')
  const [saving, setSaving] = useState(false)

  // Inline rename
  const [renamingPath, setRenamingPath] = useState<string | null>(null)
  const [renameValue, setRenameValue] = useState('')
  const renameRef = useRef<HTMLInputElement>(null)

  // New file/dir
  const [creating, setCreating] = useState<{ type: 'file' | 'dir'; parentPath: string } | null>(null)
  const [createName, setCreateName] = useState('')
  const createRef = useRef<HTMLInputElement>(null)

  // Upload
  const uploadRef = useRef<HTMLInputElement>(null)

  // Base directory
  const [docsBaseDir, setDocsBaseDir] = useState(() => localStorage.getItem(docsBaseDirKey) || '')
  const [showBaseConfig, setShowBaseConfig] = useState(false)
  const [baseDirInput, setBaseDirInput] = useState(docsBaseDir)
  const isTmux = sessionType === 'tmux'
  const effectiveBaseDir = isTmux && docsBaseDir ? docsBaseDir : undefined

  useEffect(() => {
    setDocsBaseDir(localStorage.getItem(docsBaseDirKey) || '')
  }, [sessionId, docsBaseDirKey])

  const applyBaseDir = () => {
    const val = baseDirInput.trim()
    setDocsBaseDir(val)
    if (val) localStorage.setItem(docsBaseDirKey, val)
    else localStorage.removeItem(docsBaseDirKey)
    setShowBaseConfig(false)
    setExpanded(new Set(['.']))
    setChildren({})
    setSelectedPath(null)
    setContent('')
  }

  // Load directory children
  const loadDir = useCallback(async (dirPath: string) => {
    setLoadingDirs(prev => new Set(prev).add(dirPath))
    try {
      const data = await listSessionTree(sessionId, dirPath === '.' ? undefined : dirPath, effectiveBaseDir)
      setChildren(prev => ({ ...prev, [dirPath]: data.entries }))
      if (dirPath === '.') setTreeError(null)
    } catch (e: any) {
      if (dirPath === '.') setTreeError(e.message || 'Failed to load')
    }
    setLoadingDirs(prev => { const n = new Set(prev); n.delete(dirPath); return n })
  }, [sessionId, effectiveBaseDir])

  // Load root on mount
  useEffect(() => {
    loadDir('.')
  }, [loadDir])

  const toggleDir = (dirPath: string) => {
    setExpanded(prev => {
      const next = new Set(prev)
      if (next.has(dirPath)) {
        next.delete(dirPath)
      } else {
        next.add(dirPath)
        if (!children[dirPath]) loadDir(dirPath)
      }
      return next
    })
  }

  const selectFile = async (path: string) => {
    setSelectedPath(path)
    setEditing(false)
    setLoadingContent(true)
    try {
      const result = await getSessionFile(sessionId, path, effectiveBaseDir)
      setContent(result.content)
      setIsBinary(result.binary)
    } catch (e: any) {
      setContent(`Error: ${e.message}`)
      setIsBinary(false)
    }
    setLoadingContent(false)
  }

  const handleSave = async () => {
    if (!selectedPath) return
    setSaving(true)
    try {
      await writeSessionFile(sessionId, selectedPath, editContent)
      setContent(editContent)
      setEditing(false)
    } catch (e: any) { alert(`Save failed: ${e.message}`) }
    setSaving(false)
  }

  const handleDelete = async (path: string, type: 'file' | 'dir') => {
    if (!confirm(`Delete ${type} "${path}"?`)) return
    try {
      if (type === 'file') {
        await deleteSessionFile(sessionId, path)
        if (selectedPath === path) { setSelectedPath(null); setContent('') }
      } else {
        await deleteSessionDir(sessionId, path)
      }
      // Refresh parent
      const parent = path.includes('/') ? path.split('/').slice(0, -1).join('/') : '.'
      loadDir(parent)
    } catch (e: any) { alert(`Delete failed: ${e.message}`) }
  }

  const handleRenameStart = (path: string) => {
    setRenamingPath(path)
    setRenameValue(path.split('/').pop() || '')
  }

  const handleRenameSubmit = async () => {
    if (!renamingPath || !renameValue.trim()) { setRenamingPath(null); return }
    const parts = renamingPath.split('/')
    parts[parts.length - 1] = renameValue.trim()
    const newPath = parts.join('/')
    if (newPath === renamingPath) { setRenamingPath(null); return }
    try {
      await renameSessionFile(sessionId, renamingPath, newPath)
      if (selectedPath === renamingPath) setSelectedPath(newPath)
      const parent = renamingPath.includes('/') ? renamingPath.split('/').slice(0, -1).join('/') : '.'
      loadDir(parent)
    } catch (e: any) { alert(`Rename failed: ${e.message}`) }
    setRenamingPath(null)
  }

  const handleCreateStart = (parentPath: string, type: 'file' | 'dir') => {
    setCreating({ type, parentPath })
    setCreateName('')
    if (!expanded.has(parentPath)) {
      setExpanded(prev => new Set(prev).add(parentPath))
      if (!children[parentPath]) loadDir(parentPath)
    }
  }

  const handleCreateSubmit = async () => {
    if (!creating || !createName.trim()) { setCreating(null); return }
    const fullPath = creating.parentPath === '.' ? createName.trim() : `${creating.parentPath}/${createName.trim()}`
    try {
      if (creating.type === 'file') {
        await writeSessionFile(sessionId, fullPath, '')
        loadDir(creating.parentPath)
        setTimeout(() => selectFile(fullPath), 300)
      } else {
        await createSessionDir(sessionId, fullPath)
        loadDir(creating.parentPath)
      }
    } catch (e: any) { alert(`Create failed: ${e.message}`) }
    setCreating(null)
    setCreateName('')
  }

  const handleUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const fileList = e.target.files
    if (!fileList || fileList.length === 0) return
    try {
      for (let i = 0; i < fileList.length; i++) {
        const file = fileList[i]
        const reader = new FileReader()
        await new Promise<void>((resolve, reject) => {
          reader.onload = async () => {
            try {
              const base64 = (reader.result as string).split(',')[1]
              await uploadSessionFile(sessionId, file.name, base64, effectiveBaseDir)
              resolve()
            } catch (err) { reject(err) }
          }
          reader.onerror = () => reject(reader.error)
          reader.readAsDataURL(file)
        })
      }
      loadDir('.')
    } catch (err: any) { alert(`Upload failed: ${err.message}`) }
    if (uploadRef.current) uploadRef.current.value = ''
  }

  const handleDownload = (path: string) => {
    const url = downloadFileUrl(sessionId, path, effectiveBaseDir)
    const a = document.createElement('a')
    a.href = url
    a.download = path.split('/').pop() || 'file'
    a.click()
  }

  // Focus refs
  useEffect(() => { if (renamingPath && renameRef.current) { renameRef.current.focus(); renameRef.current.select() } }, [renamingPath])
  useEffect(() => { if (creating && createRef.current) createRef.current.focus() }, [creating])

  const refreshAll = () => {
    setChildren({})
    loadDir('.')
    expanded.forEach(dir => { if (dir !== '.') loadDir(dir) })
  }

  return (
    <div className="flex h-full">
      {/* Tree sidebar */}
      <div className="w-56 border-r border-[var(--border)] flex flex-col bg-[var(--bg-secondary)] shrink-0">
        {/* Toolbar */}
        <div className="flex items-center justify-between px-3 h-9 border-b border-[var(--border)]">
          <span className="text-[10px] font-semibold text-[var(--text-muted)] uppercase tracking-wider">Files</span>
          <div className="flex items-center gap-0.5">
            {isTmux && (
              <button onClick={() => { setShowBaseConfig(!showBaseConfig); setBaseDirInput(docsBaseDir) }}
                className={`p-1 rounded transition-colors ${docsBaseDir ? 'text-[var(--accent-blue)]' : 'text-[var(--text-secondary)] hover:text-[var(--text-primary)]'}`}
                title={docsBaseDir || 'Set base directory'}
              ><Settings size={12} /></button>
            )}
            <button onClick={() => handleCreateStart('.', 'file')} className="p-1 text-[var(--text-secondary)] hover:text-[var(--accent-green-text)] rounded transition-colors" title="New file"><Plus size={12} /></button>
            <button onClick={() => handleCreateStart('.', 'dir')} className="p-1 text-[var(--text-secondary)] hover:text-[var(--accent-green-text)] rounded transition-colors" title="New directory"><FolderPlus size={12} /></button>
            <button onClick={() => uploadRef.current?.click()} className="p-1 text-[var(--text-secondary)] hover:text-[var(--accent-blue)] rounded transition-colors" title="Upload"><Upload size={12} /></button>
            <input ref={uploadRef} type="file" multiple className="hidden" onChange={handleUpload} />
            <button onClick={refreshAll} className="p-1 text-[var(--text-secondary)] hover:text-[var(--text-primary)] rounded transition-colors" title="Refresh"><RefreshCw size={12} /></button>
          </div>
        </div>

        {/* Base dir config */}
        {showBaseConfig && (
          <div className="px-2 py-1.5 border-b border-[var(--border)] bg-[var(--bg-tertiary)]">
            <div className="text-[10px] text-[var(--text-muted)] mb-1">Base directory</div>
            <div className="flex items-center gap-1">
              <input value={baseDirInput} onChange={e => setBaseDirInput(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') applyBaseDir(); if (e.key === 'Escape') setShowBaseConfig(false) }}
                placeholder="Leave empty for session default" autoFocus
                className="flex-1 text-[11px] bg-[var(--bg-primary)] border border-[var(--border)] rounded px-1.5 py-0.5 text-[var(--text-primary)] outline-none focus:border-[var(--accent-blue)] placeholder-[var(--text-muted)]"
              />
              <button onClick={applyBaseDir} className="p-0.5 text-[var(--accent-green-text)]"><Check size={12} /></button>
              <button onClick={() => setShowBaseConfig(false)} className="p-0.5 text-[var(--text-secondary)]"><X size={12} /></button>
            </div>
          </div>
        )}

        {/* Base dir breadcrumb */}
        {docsBaseDir && (
          <div className="flex items-center gap-1 px-2 py-1 border-b border-[var(--border)] bg-[var(--bg-tertiary)]">
            <button onClick={() => {
              const parts = docsBaseDir.split('/')
              if (parts.length > 3) {
                const parent = parts.slice(0, -1).join('/')
                setDocsBaseDir(parent); setBaseDirInput(parent); localStorage.setItem(docsBaseDirKey, parent)
              } else {
                setDocsBaseDir(''); setBaseDirInput(''); localStorage.removeItem(docsBaseDirKey)
              }
              setExpanded(new Set(['.'])); setChildren({}); setSelectedPath(null); setContent('')
            }} className="p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-primary)] shrink-0" title="Go up">
              <ChevronRight size={10} className="rotate-180" />
            </button>
            <span className="text-[9px] text-[var(--text-muted)] truncate font-mono" title={docsBaseDir}>
              {docsBaseDir.split('/').slice(-2).join('/')}
            </span>
          </div>
        )}

        {/* Tree */}
        <div className="flex-1 overflow-y-auto py-1">
          {treeError ? (
            <div className="px-3 py-4 text-center">
              <p className="text-[10px] text-[var(--accent-red)] mb-1">{treeError}</p>
              <button onClick={() => loadDir('.')} className="text-[10px] text-[var(--accent-blue)] hover:underline">Retry</button>
            </div>
          ) : children['.'] ? (
            <TreeLevel
              entries={children['.']}
              depth={0}
              expanded={expanded}
              children_map={children}
              loadingDirs={loadingDirs}
              selectedPath={selectedPath}
              renamingPath={renamingPath}
              renameValue={renameValue}
              renameRef={renameRef}
              creating={creating}
              createName={createName}
              createRef={createRef}
              onToggleDir={toggleDir}
              onSelectFile={selectFile}
              onDoubleClickDir={(dirPath) => {
                const val = effectiveBaseDir ? `${effectiveBaseDir}/${dirPath}` : dirPath
                setDocsBaseDir(val); setBaseDirInput(val); localStorage.setItem(docsBaseDirKey, val)
                setExpanded(new Set(['.'])); setChildren({}); setSelectedPath(null); setContent('')
              }}
              onDelete={handleDelete}
              onRenameStart={handleRenameStart}
              onRenameChange={setRenameValue}
              onRenameSubmit={handleRenameSubmit}
              onRenameCancel={() => setRenamingPath(null)}
              onCreateStart={handleCreateStart}
              onCreateChange={setCreateName}
              onCreateSubmit={handleCreateSubmit}
              onCreateCancel={() => { setCreating(null); setCreateName('') }}
              onDownload={handleDownload}
            />
          ) : (
            <div className="px-3 py-2 text-[10px] text-[var(--text-muted)]">Loading...</div>
          )}
        </div>
      </div>

      {/* Preview pane */}
      <div className="flex-1 flex flex-col min-w-0">
        {selectedPath && (
          <div className="flex items-center justify-between px-4 h-9 border-b border-[var(--border)] bg-[var(--bg-secondary)] shrink-0">
            <span className="text-[10px] text-[var(--text-muted)] font-mono truncate">{selectedPath}</span>
            {!isBinary && (
              <div className="flex items-center gap-1 shrink-0">
                {editing ? (
                  <>
                    <button onClick={handleSave} disabled={saving}
                      className="flex items-center gap-1 px-2 py-0.5 text-[10px] font-medium bg-[var(--accent-blue)] hover:bg-[var(--accent-blue-hover)] text-white rounded transition-colors disabled:opacity-50">
                      <Save size={10} />{saving ? 'Saving...' : 'Save'}
                    </button>
                    <button onClick={() => setEditing(false)}
                      className="flex items-center gap-1 px-2 py-0.5 text-[10px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] border border-[var(--border)] rounded transition-colors">
                      <Eye size={10} />Preview
                    </button>
                  </>
                ) : (
                  <button onClick={() => { setEditContent(content); setEditing(true) }}
                    className="flex items-center gap-1 px-2 py-0.5 text-[10px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] border border-[var(--border)] rounded transition-colors">
                    <Edit3 size={10} />Edit
                  </button>
                )}
              </div>
            )}
          </div>
        )}

        <div className="flex-1 overflow-y-auto">
          {loadingContent ? (
            <div className="p-6 text-sm text-[var(--text-muted)]">Loading...</div>
          ) : selectedPath ? (
            isBinary ? (
              content.startsWith('data:image/') ? (
                <div className="p-6 flex items-center justify-center h-full">
                  <img src={content} alt={selectedPath} className="max-w-full max-h-full object-contain rounded" />
                </div>
              ) : (
                <div className="flex flex-col items-center justify-center h-full gap-2 text-sm text-[var(--text-muted)]">
                  <span>Binary file</span>
                  <button onClick={() => handleDownload(selectedPath)}
                    className="flex items-center gap-1 px-3 py-1 text-xs bg-[var(--accent-blue)] text-white rounded hover:bg-[var(--accent-blue-hover)]">
                    <Download size={12} />Download
                  </button>
                </div>
              )
            ) : editing ? (
              <textarea value={editContent} onChange={e => setEditContent(e.target.value)}
                className="w-full h-full p-6 text-sm font-mono bg-[var(--bg-primary)] text-[var(--text-primary)] outline-none resize-none leading-relaxed" spellCheck={false} />
            ) : selectedPath.endsWith('.md') ? (
              <div className="p-6 max-w-3xl mx-auto">
                <article className="text-sm text-[var(--text-primary)] leading-relaxed">
                  <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>{content}</ReactMarkdown>
                </article>
              </div>
            ) : (
              <CodeBlock content={content} path={selectedPath} />
            )
          ) : (
            <div className="flex items-center justify-center h-full text-sm text-[var(--text-muted)]">
              Select a file to view
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

// ── Code Block with Syntax Highlighting ──

function CodeBlock({ content, path }: { content: string; path: string }) {
  const highlighted = useMemo(() => {
    const lang = getLangForFile(path)
    if (lang && hljs.getLanguage(lang)) {
      try {
        return hljs.highlight(content, { language: lang }).value
      } catch { /* fallback */ }
    }
    return null
  }, [content, path])

  if (highlighted) {
    return (
      <pre className="p-6 text-sm leading-relaxed overflow-x-auto">
        <code className="hljs" dangerouslySetInnerHTML={{ __html: highlighted }} />
      </pre>
    )
  }
  return (
    <pre className="p-6 text-sm font-mono text-[var(--text-primary)] whitespace-pre-wrap break-words leading-relaxed">{content}</pre>
  )
}

// ── Recursive Tree Rendering ──

interface TreeLevelProps {
  entries: TreeEntry[]
  depth: number
  expanded: Set<string>
  children_map: Record<string, TreeEntry[]>
  loadingDirs: Set<string>
  selectedPath: string | null
  renamingPath: string | null
  renameValue: string
  renameRef: React.RefObject<HTMLInputElement | null>
  creating: { type: 'file' | 'dir'; parentPath: string } | null
  createName: string
  createRef: React.RefObject<HTMLInputElement | null>
  onToggleDir: (path: string) => void
  onSelectFile: (path: string) => void
  onDoubleClickDir: (path: string) => void
  onDelete: (path: string, type: 'file' | 'dir') => void
  onRenameStart: (path: string) => void
  onRenameChange: (val: string) => void
  onRenameSubmit: () => void
  onRenameCancel: () => void
  onCreateStart: (parentPath: string, type: 'file' | 'dir') => void
  onCreateChange: (val: string) => void
  onCreateSubmit: () => void
  onCreateCancel: () => void
  onDownload: (path: string) => void
}

function TreeLevel(props: TreeLevelProps) {
  const { entries, depth, expanded, children_map, loadingDirs, selectedPath,
    renamingPath, renameValue, renameRef, creating, createName, createRef,
    onToggleDir, onSelectFile, onDoubleClickDir, onDelete,
    onRenameStart, onRenameChange, onRenameSubmit, onRenameCancel,
    onCreateStart, onCreateChange, onCreateSubmit, onCreateCancel, onDownload } = props

  const indent = depth * 16

  return (
    <>
      {entries.map(entry => {
        if (entry.type === 'dir') {
          const isExpanded = expanded.has(entry.path)
          const isLoading = loadingDirs.has(entry.path)
          const dirChildren = children_map[entry.path]

          return (
            <div key={entry.path}>
              {/* Directory row */}
              <div
                className="flex items-center h-[26px] pr-1 hover:bg-[var(--bg-tertiary)] group cursor-pointer"
                style={{ paddingLeft: `${indent + 4}px` }}
                onClick={() => onToggleDir(entry.path)}
                onDoubleClick={() => onDoubleClickDir(entry.path)}
              >
                {isExpanded ? <ChevronDown size={12} className="shrink-0 text-[var(--text-muted)]" /> : <ChevronRight size={12} className="shrink-0 text-[var(--text-muted)]" />}
                <Folder size={12} className="shrink-0 text-[var(--text-muted)] ml-0.5 mr-1.5" />
                <span className="text-[11px] text-[var(--text-primary)] truncate flex-1">{entry.name}</span>
                <div className="flex items-center gap-0 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                  <button onClick={e => { e.stopPropagation(); onCreateStart(entry.path, 'file') }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-green-text)]" title="New file"><Plus size={10} /></button>
                  <button onClick={e => { e.stopPropagation(); onCreateStart(entry.path, 'dir') }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-green-text)]" title="New folder"><FolderPlus size={10} /></button>
                  <button onClick={e => { e.stopPropagation(); onDelete(entry.path, 'dir') }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-red)]" title="Delete"><Trash2 size={10} /></button>
                </div>
              </div>

              {/* Expanded children */}
              {isExpanded && (
                isLoading ? (
                  <div style={{ paddingLeft: `${indent + 20}px` }} className="text-[9px] text-[var(--text-muted)] py-1">Loading...</div>
                ) : dirChildren ? (
                  <>
                    {/* Inline create for this dir */}
                    {creating && creating.parentPath === entry.path && (
                      <div className="flex items-center h-[26px] pr-1" style={{ paddingLeft: `${indent + 20}px` }}>
                        {creating.type === 'dir' ? <FolderPlus size={11} className="shrink-0 text-[var(--text-muted)] mr-1" /> : <Plus size={11} className="shrink-0 text-[var(--text-muted)] mr-1" />}
                        <input ref={createRef} value={createName} onChange={e => onCreateChange(e.target.value)}
                          onKeyDown={e => { if (e.key === 'Enter') onCreateSubmit(); if (e.key === 'Escape') onCreateCancel() }}
                          onBlur={onCreateCancel} placeholder={creating.type === 'file' ? 'filename' : 'dirname'}
                          className="flex-1 text-[11px] bg-[var(--bg-primary)] border border-[var(--accent-blue)] rounded px-1 py-0 text-[var(--text-primary)] outline-none min-w-0" />
                      </div>
                    )}
                    <TreeLevel {...props} entries={dirChildren} depth={depth + 1} />
                  </>
                ) : null
              )}
            </div>
          )
        }

        // File row
        const FileIcon = getFileIcon(entry.name)
        const isSelected = selectedPath === entry.path
        const isRenaming = renamingPath === entry.path

        if (isRenaming) {
          return (
            <div key={entry.path} className="flex items-center h-[26px] pr-1" style={{ paddingLeft: `${indent + 20}px` }}>
              <FileIcon size={12} className="shrink-0 text-[var(--text-muted)] mr-1.5" />
              <input ref={renameRef} value={renameValue} onChange={e => onRenameChange(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') onRenameSubmit(); if (e.key === 'Escape') onRenameCancel() }}
                onBlur={onRenameSubmit}
                className="flex-1 text-[11px] bg-[var(--bg-primary)] border border-[var(--accent-blue)] rounded px-1 py-0 text-[var(--text-primary)] outline-none min-w-0" />
            </div>
          )
        }

        return (
          <div
            key={entry.path}
            className={`flex items-center h-[26px] pr-1 cursor-pointer group transition-colors ${
              isSelected ? 'bg-[var(--bg-primary)] text-[var(--accent-blue)]' : 'hover:bg-[var(--bg-tertiary)] text-[var(--text-secondary)]'
            }`}
            style={{ paddingLeft: `${indent + 20}px` }}
            onClick={() => onSelectFile(entry.path)}
          >
            <FileIcon size={12} className="shrink-0 mr-1.5" />
            <span className="text-[11px] truncate flex-1">{entry.name}</span>
            <div className="flex items-center gap-0 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
              <button onClick={e => { e.stopPropagation(); onDownload(entry.path) }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-blue)]" title="Download"><Download size={10} /></button>
              <button onClick={e => { e.stopPropagation(); onRenameStart(entry.path) }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-blue)]" title="Rename"><Pencil size={10} /></button>
              <button onClick={e => { e.stopPropagation(); onDelete(entry.path, 'file') }} className="p-0.5 text-[var(--text-muted)] hover:text-[var(--accent-red)]" title="Delete"><Trash2 size={10} /></button>
            </div>
          </div>
        )
      })}

      {/* Inline create at root level */}
      {creating && creating.parentPath === '.' && depth === 0 && (
        <div className="flex items-center h-[26px] pr-1" style={{ paddingLeft: `${indent + 4}px` }}>
          {creating.type === 'dir' ? <FolderPlus size={11} className="shrink-0 text-[var(--text-muted)] mr-1" /> : <Plus size={11} className="shrink-0 text-[var(--text-muted)] mr-1" />}
          <input ref={createRef} value={createName} onChange={e => onCreateChange(e.target.value)}
            onKeyDown={e => { if (e.key === 'Enter') onCreateSubmit(); if (e.key === 'Escape') onCreateCancel() }}
            onBlur={onCreateCancel} placeholder={creating.type === 'file' ? 'filename' : 'dirname'}
            className="flex-1 text-[11px] bg-[var(--bg-primary)] border border-[var(--accent-blue)] rounded px-1 py-0 text-[var(--text-primary)] outline-none min-w-0" />
        </div>
      )}
    </>
  )
}
