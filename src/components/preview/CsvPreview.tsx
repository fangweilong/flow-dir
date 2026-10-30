import { useMemo, useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  text: string | null | undefined
  name: string
  t: Translate
}

function parseDelimited(text: string, delimiter: string, maxRows = 200): string[][] {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let inQuotes = false

  for (let i = 0; i < text.length; i++) {
    const ch = text[i]
    if (inQuotes) {
      if (ch === '"') {
        if (i + 1 < text.length && text[i + 1] === '"') {
          cell += '"'
          i++
        } else {
          inQuotes = false
        }
      } else {
        cell += ch
      }
    } else {
      if (ch === '"') {
        inQuotes = true
      } else if (ch === delimiter) {
        row.push(cell)
        cell = ''
      } else if (ch === '\n' || ch === '\r') {
        if (ch === '\r' && i + 1 < text.length && text[i + 1] === '\n') {
          i++
        }
        row.push(cell)
        cell = ''
        if (row.length > 1 || row[0] !== '') {
          rows.push(row)
        }
        row = []
        if (rows.length >= maxRows) break
      } else {
        cell += ch
      }
    }
  }
  if (cell !== '' || row.length > 0) {
    row.push(cell)
    rows.push(row)
  }
  return rows
}

export function CsvPreview({ text, name, t }: Props) {
  const [mode, setMode] = useState<'table' | 'source'>('table')
  const delimiter = name.toLowerCase().endsWith('.tsv') ? '\t' : ','
  const rows = useMemo(() => (text ? parseDelimited(text, delimiter) : []), [text, delimiter])
  const header = rows[0] ?? []
  const dataRows = rows.slice(1)

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5">
        <span className="text-[0.625rem] text-muted-2">
          {rows.length > 0 && t('preview.csvSummary', { rows: rows.length, cols: header.length })}
        </span>
        <div className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => setMode('table')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'table'
                ? 'bg-sel font-medium text-fg-strong'
                : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.table')}
          </button>
          <button
            type="button"
            onClick={() => setMode('source')}
            className={`rounded px-2 py-0.5 text-[0.625rem] ${
              mode === 'source'
                ? 'bg-sel font-medium text-fg-strong'
                : 'text-muted hover:bg-surface-3'
            }`}
          >
            {t('preview.htmlSource')}
          </button>
        </div>
      </div>

      {mode === 'table' && rows.length > 0 ? (
        <div className="min-h-0 flex-1 overflow-auto rounded border border-line bg-surface-2/40">
          <table className="w-full border-collapse font-mono text-[0.6875rem]">
            <thead className="sticky top-0 z-10 bg-surface-2 text-muted shadow-sm">
              <tr className="border-b border-line">
                <th className="w-10 px-2 py-1 text-right text-[0.625rem] font-semibold text-muted-2">#</th>
                {header.map((col, idx) => (
                  <th
                    key={idx}
                    className="border-l border-line px-2 py-1 text-left font-semibold text-fg-strong whitespace-nowrap"
                  >
                    {col}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {dataRows.map((r, rowIdx) => (
                <tr
                  key={rowIdx}
                  className="border-b border-line/60 hover:bg-surface-3/50 even:bg-surface/60"
                >
                  <td className="w-10 px-2 py-0.5 text-right text-[0.625rem] text-muted-2 select-none">
                    {rowIdx + 1}
                  </td>
                  {header.map((_, colIdx) => (
                    <td
                      key={colIdx}
                      className="max-w-xs truncate border-l border-line/60 px-2 py-0.5 text-fg"
                      title={r[colIdx] ?? ''}
                    >
                      {r[colIdx] ?? ''}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
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
