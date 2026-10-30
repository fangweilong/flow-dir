import { useMemo, useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  text: string | null | undefined
  name: string
  t: Translate
}

function renderInline(text: string): React.ReactNode[] {
  const parts: React.ReactNode[] = []
  const tokenRegex = /(\*\*[^*]+\*\*|__[^_]+__|`[^`]+`|\*[^*]+\*|_[^_]+_|~~[^~]+~~|\[[^\]]+\]\([^)]+\))/g
  let lastIndex = 0
  let match: RegExpExecArray | null

  while ((match = tokenRegex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(text.slice(lastIndex, match.index))
    }
    const token = match[0]
    if (token.startsWith('**') && token.endsWith('**')) {
      parts.push(
        <strong key={match.index} className="font-semibold text-fg-strong">
          {token.slice(2, -2)}
        </strong>
      )
    } else if (token.startsWith('__') && token.endsWith('__')) {
      parts.push(
        <strong key={match.index} className="font-semibold text-fg-strong">
          {token.slice(2, -2)}
        </strong>
      )
    } else if (token.startsWith('`') && token.endsWith('`')) {
      parts.push(
        <code
          key={match.index}
          className="rounded bg-surface-3 px-1 py-0.5 font-mono text-[0.6875rem] text-fg-strong"
        >
          {token.slice(1, -1)}
        </code>
      )
    } else if (token.startsWith('~~') && token.endsWith('~~')) {
      parts.push(
        <del key={match.index} className="text-muted line-through">
          {token.slice(2, -2)}
        </del>
      )
    } else if ((token.startsWith('*') && token.endsWith('*')) || (token.startsWith('_') && token.endsWith('_'))) {
      parts.push(
        <em key={match.index} className="italic text-fg">
          {token.slice(1, -1)}
        </em>
      )
    } else if (token.startsWith('[')) {
      const linkMatch = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(token)
      if (linkMatch) {
        parts.push(
          <span
            key={match.index}
            className="text-blue-500 underline underline-offset-2"
            title={linkMatch[2]}
          >
            {linkMatch[1]}
          </span>
        )
      } else {
        parts.push(token)
      }
    }
    lastIndex = tokenRegex.lastIndex
  }
  if (lastIndex < text.length) {
    parts.push(text.slice(lastIndex))
  }
  return parts.length ? parts : [text]
}

export function MarkdownPreview({ text, t }: Props) {
  const [mode, setMode] = useState<'preview' | 'source'>('preview')
  const [copied, setCopied] = useState(false)

  const elements = useMemo(() => {
    if (!text) return null
    const lines = text.split(/\r?\n/)
    const nodes: React.ReactNode[] = []
    let inCodeBlock = false
    let codeBlockLang = ''
    let codeBlockContent: string[] = []
    let inTable = false
    let tableRows: string[][] = []

    const flushTable = () => {
      if (!tableRows.length) return
      const [header, , ...body] = tableRows
      if (header) {
        nodes.push(
          <div key={`table-${nodes.length}`} className="my-2 overflow-x-auto rounded border border-line bg-surface-2/40">
            <table className="w-full border-collapse font-mono text-[0.6875rem]">
              <thead className="bg-surface-2 text-fg-strong border-b border-line">
                <tr>
                  {header.map((col, idx) => (
                    <th key={idx} className="border-l first:border-l-0 border-line px-2 py-1 text-left font-semibold">
                      {col.trim()}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {body.map((row, rIdx) => (
                  <tr key={rIdx} className="border-b last:border-b-0 border-line/60 even:bg-surface/60">
                    {header.map((_, cIdx) => (
                      <td key={cIdx} className="border-l first:border-l-0 border-line/60 px-2 py-0.5 text-fg">
                        {renderInline(row[cIdx]?.trim() ?? '')}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )
      }
      tableRows = []
      inTable = false
    }

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i]
      if (line.startsWith('```')) {
        if (inTable) flushTable()
        if (inCodeBlock) {
          const code = codeBlockContent.join('\n')
          nodes.push(
            <div key={`code-${i}`} className="my-2 overflow-hidden rounded border border-line bg-surface-2">
              {codeBlockLang && (
                <div className="border-b border-line px-2 py-0.5 text-[0.625rem] font-mono text-muted">
                  {codeBlockLang}
                </div>
              )}
              <pre className="overflow-x-auto p-2 font-mono text-[0.6875rem] leading-relaxed text-fg">
                {code}
              </pre>
            </div>
          )
          inCodeBlock = false
          codeBlockContent = []
        } else {
          inCodeBlock = true
          codeBlockLang = line.slice(3).trim()
        }
        continue
      }

      if (inCodeBlock) {
        codeBlockContent.push(line)
        continue
      }

      if (line.trim().startsWith('|') && line.trim().endsWith('|')) {
        inTable = true
        const cols = line.trim().slice(1, -1).split('|')
        tableRows.push(cols)
        continue
      } else if (inTable) {
        flushTable()
      }

      if (line.startsWith('# ')) {
        nodes.push(
          <h1 key={i} className="mt-3 mb-1 border-b border-line pb-1 text-[0.9375rem] font-bold text-fg-strong">
            {renderInline(line.slice(2))}
          </h1>
        )
      } else if (line.startsWith('## ')) {
        nodes.push(
          <h2 key={i} className="mt-2.5 mb-1 text-[0.8125rem] font-semibold text-fg-strong">
            {renderInline(line.slice(3))}
          </h2>
        )
      } else if (line.startsWith('### ')) {
        nodes.push(
          <h3 key={i} className="mt-2 mb-0.5 text-[0.75rem] font-semibold text-fg-strong">
            {renderInline(line.slice(4))}
          </h3>
        )
      } else if (line.startsWith('#### ')) {
        nodes.push(
          <h4 key={i} className="mt-1.5 mb-0.5 text-[0.6875rem] font-medium text-fg-strong">
            {renderInline(line.slice(5))}
          </h4>
        )
      } else if (line.startsWith('> ')) {
        nodes.push(
          <blockquote key={i} className="my-1 border-l-2 border-blue-500 pl-2 text-[0.6875rem] italic text-muted">
            {renderInline(line.slice(2))}
          </blockquote>
        )
      } else if (line.startsWith('---') || line.startsWith('***')) {
        nodes.push(<hr key={i} className="my-2 border-line" />)
      } else if (/^[-*]\s+\[[ x]\]\s+/i.test(line)) {
        const checked = /^[-*]\s+\[x\]\s+/i.test(line)
        const label = line.replace(/^[-*]\s+\[[ x]\]\s+/i, '')
        nodes.push(
          <div key={i} className="flex items-center gap-1.5 py-0.5 text-[0.6875rem] text-fg">
            <input type="checkbox" readOnly checked={checked} className="h-3 w-3 rounded accent-blue-500" />
            <span className={checked ? 'line-through text-muted' : ''}>{renderInline(label)}</span>
          </div>
        )
      } else if (line.startsWith('- ') || line.startsWith('* ')) {
        nodes.push(
          <li key={i} className="ml-4 list-disc text-[0.6875rem] leading-relaxed text-fg">
            {renderInline(line.slice(2))}
          </li>
        )
      } else if (/^\d+\.\s+/.test(line)) {
        const content = line.replace(/^\d+\.\s+/, '')
        nodes.push(
          <li key={i} className="ml-4 list-decimal text-[0.6875rem] leading-relaxed text-fg">
            {renderInline(content)}
          </li>
        )
      } else if (line.trim() === '') {
        nodes.push(<div key={i} className="h-1.5" />)
      } else {
        nodes.push(
          <p key={i} className="text-[0.6875rem] leading-relaxed text-fg">
            {renderInline(line)}
          </p>
        )
      }
    }
    if (inTable) flushTable()
    return nodes
  }, [text])

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5">
        <div className="flex items-center gap-1">
          {text && (
            <button
              type="button"
              onClick={() => {
                void navigator.clipboard.writeText(text)
                setCopied(true)
                setTimeout(() => setCopied(false), 1500)
              }}
              className="rounded px-2 py-0.5 text-[0.625rem] text-muted hover:bg-surface-3 hover:text-fg"
            >
              {copied ? t('preview.copied') : t('preview.copy')}
            </button>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => setMode('preview')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'preview' ? 'bg-sel font-medium text-fg-strong' : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.htmlRender')}
          </button>
          <button
            type="button"
            onClick={() => setMode('source')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'source' ? 'bg-sel font-medium text-fg-strong' : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.htmlSource')}
          </button>
        </div>
      </div>

      {mode === 'preview' ? (
        <div className="min-h-0 flex-1 overflow-auto rounded border border-line bg-surface p-3 space-y-1">
          {elements}
        </div>
      ) : (
        <pre
          data-selectable
          className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words font-mono text-[0.6875rem] leading-relaxed text-fg"
        >
          {text}
        </pre>
      )}
    </div>
  )
}
