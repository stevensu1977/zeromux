import type { Components } from 'react-markdown'

export const markdownComponents: Components = {
  strong: ({ children }) => <strong className="font-bold text-[var(--text-bright)]">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  a: ({ href, children }) => (
    <a href={href} target="_blank" rel="noopener noreferrer" className="text-[var(--accent-blue)] underline hover:text-[var(--accent-blue-hover)]">
      {children}
    </a>
  ),
  code: ({ className, children, ...props }) => {
    const isBlock = className?.startsWith('language-')
    if (isBlock) {
      return <code className={`text-[12px] ${className ?? ''}`} {...props}>{children}</code>
    }
    return (
      <code className="px-1 py-0.5 bg-[var(--code-bg)] border border-[var(--border)] rounded text-[12px] text-[var(--text-bright)] font-mono" {...props}>
        {children}
      </code>
    )
  },
  p: ({ children }) => <p className="mb-2 last:mb-0">{children}</p>,
  pre: ({ children }) => (
    <pre className="bg-[var(--bg-secondary)] border border-[var(--border)] rounded-md p-3 my-2 overflow-x-auto text-[12px] text-[var(--text-bright)] font-mono">
      {children}
    </pre>
  ),
  ul: ({ children }) => <ul className="list-disc list-inside mb-2 space-y-0.5">{children}</ul>,
  ol: ({ children }) => <ol className="list-decimal list-inside mb-2 space-y-0.5">{children}</ol>,
  li: ({ children }) => <li>{children}</li>,
  h1: ({ children }) => <h1 className="text-lg font-bold text-[var(--text-bright)] mb-2 mt-3">{children}</h1>,
  h2: ({ children }) => <h2 className="text-base font-bold text-[var(--text-bright)] mb-1.5 mt-2">{children}</h2>,
  h3: ({ children }) => <h3 className="text-sm font-bold text-[var(--text-bright)] mb-1 mt-2">{children}</h3>,
  blockquote: ({ children }) => (
    <blockquote className="border-l-2 border-[var(--border)] pl-3 text-[var(--text-secondary)] italic my-2">{children}</blockquote>
  ),
  hr: () => <hr className="border-[var(--border)] my-3" />,
  table: ({ children }) => (
    <div className="overflow-x-auto my-2">
      <table className="text-xs border-collapse border border-[var(--border)]">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border border-[var(--border)] px-2 py-1 bg-[var(--bg-secondary)] text-left font-semibold">{children}</th>,
  td: ({ children }) => <td className="border border-[var(--border)] px-2 py-1">{children}</td>,
}
