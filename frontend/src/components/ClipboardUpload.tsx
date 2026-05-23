import { useState, useRef, useEffect, useCallback } from 'react'
import { uploadSessionFile } from '../lib/api'
import { X, Upload, Clipboard } from 'lucide-react'

interface Props {
  sessionId: string
  onClose: () => void
  onUploaded?: (filename: string) => void
}

function generateFilename(): string {
  const now = new Date()
  const y = now.getFullYear()
  const mo = String(now.getMonth() + 1).padStart(2, '0')
  const d = String(now.getDate()).padStart(2, '0')
  const h = String(now.getHours()).padStart(2, '0')
  const m = String(now.getMinutes()).padStart(2, '0')
  const s = String(now.getSeconds()).padStart(2, '0')
  return `clipboard-${y}${mo}${d}-${h}${m}${s}.png`
}

export default function ClipboardUpload({ sessionId, onClose, onUploaded }: Props) {
  const [imageData, setImageData] = useState<string | null>(null)
  const [uploading, setUploading] = useState(false)
  const [error, setError] = useState('')
  const pasteRef = useRef<HTMLDivElement>(null)

  const handlePaste = useCallback((e: ClipboardEvent) => {
    const items = e.clipboardData?.items
    if (!items) return
    for (let i = 0; i < items.length; i++) {
      if (items[i].type.startsWith('image/')) {
        e.preventDefault()
        const blob = items[i].getAsFile()
        if (!blob) continue
        const reader = new FileReader()
        reader.onload = () => {
          setImageData(reader.result as string)
          setError('')
        }
        reader.readAsDataURL(blob)
        return
      }
    }
    setError('No image in clipboard')
  }, [])

  useEffect(() => {
    document.addEventListener('paste', handlePaste)
    return () => document.removeEventListener('paste', handlePaste)
  }, [handlePaste])

  useEffect(() => {
    pasteRef.current?.focus()
  }, [])

  const [uploadedName, setUploadedName] = useState<string | null>(null)

  const handleUpload = async () => {
    if (!imageData) return
    setUploading(true)
    try {
      const base64 = imageData.split(',')[1]
      const filename = generateFilename()
      await uploadSessionFile(sessionId, filename, base64)
      setUploadedName(filename)
      onUploaded?.(filename)
    } catch (err: any) {
      setError(`Upload failed: ${err.message}`)
    }
    setUploading(false)
  }

  return (
    <div className="absolute right-0 top-full mt-1 z-50 bg-[var(--bg-tertiary)] border border-[var(--border)] rounded-lg shadow-xl w-64">
      <div className="flex items-center justify-between px-3 py-2 border-b border-[var(--border)]">
        <span className="text-[11px] font-medium text-[var(--text-primary)]">Paste Image</span>
        <button onClick={onClose} className="p-0.5 text-[var(--text-secondary)] hover:text-[var(--text-primary)]">
          <X size={12} />
        </button>
      </div>

      <div
        ref={pasteRef}
        tabIndex={0}
        className="m-2 border-2 border-dashed border-[var(--border)] rounded-md flex items-center justify-center min-h-[120px] outline-none focus:border-[var(--accent-blue)] transition-colors cursor-pointer"
        onClick={async () => {
          try {
            const items = await navigator.clipboard.read()
            for (const item of items) {
              const imageType = item.types.find(t => t.startsWith('image/'))
              if (imageType) {
                const blob = await item.getType(imageType)
                const reader = new FileReader()
                reader.onload = () => {
                  setImageData(reader.result as string)
                  setError('')
                }
                reader.readAsDataURL(blob)
                return
              }
            }
            setError('No image in clipboard')
          } catch {
            setError('Ctrl+V to paste')
          }
        }}
      >
        {imageData ? (
          <img src={imageData} alt="Preview" className="max-w-full max-h-[140px] object-contain rounded" />
        ) : (
          <div className="text-center py-4">
            <Clipboard size={20} className="mx-auto text-[var(--text-muted)] mb-1" />
            <p className="text-[10px] text-[var(--text-muted)]">Click or Ctrl+V to paste</p>
          </div>
        )}
      </div>

      {error && (
        <p className="px-3 text-[10px] text-[var(--accent-red)]">{error}</p>
      )}

      {uploadedName ? (
        <div className="px-3 py-2">
          <p className="text-[10px] text-[var(--accent-green-text)] mb-1">Uploaded successfully!</p>
          <div className="flex items-center gap-1 bg-[var(--bg-primary)] border border-[var(--border)] rounded px-2 py-1">
            <span className="text-[11px] font-mono text-[var(--text-primary)] truncate flex-1">{uploadedName}</span>
            <button
              onClick={() => { navigator.clipboard.writeText(uploadedName) }}
              className="text-[10px] text-[var(--accent-blue)] hover:underline shrink-0"
            >
              Copy
            </button>
          </div>
          <div className="flex justify-end gap-2 mt-2">
            <button
              onClick={() => { setUploadedName(null); setImageData(null) }}
              className="text-[10px] text-[var(--accent-blue)] hover:underline"
            >
              Paste another
            </button>
            <button
              onClick={onClose}
              className="text-[10px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              Done
            </button>
          </div>
        </div>
      ) : (
        <div className="px-3 py-2 flex items-center justify-end gap-2">
          {imageData && (
            <button
              onClick={() => { setImageData(null); setError('') }}
              className="text-[10px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              Clear
            </button>
          )}
          <button
            onClick={handleUpload}
            disabled={!imageData || uploading}
            className="flex items-center gap-1 px-2.5 py-1 text-[10px] font-medium bg-[var(--accent-blue)] hover:bg-[var(--accent-blue-hover)] text-white rounded transition-colors disabled:opacity-40"
          >
            <Upload size={10} />
            {uploading ? 'Uploading...' : 'Upload'}
          </button>
        </div>
      )}
    </div>
  )
}
