import { useEffect, useMemo, useState } from 'react'
import type { Translate } from '../../i18n'
import { CSV_MAX_ROWS, CSV_PAGE_SIZE, parseDelimited } from './csv'

type Props = {
  text: string | null | undefined
  name: string
  t: Translate
}

export function CsvPreview({ text, name, t }: Props) {
  const [mode, setMode] = useState<'table' | 'source'>('table')
  const [visible, setVisible] = useState(CSV_PAGE_SIZE)
  const delimiter = name.toLowerCase().endsWith('.tsv') ? '\t' : ','
  const table = useMemo(
    () => (text ? parseDelimited(text, delimiter) : { rows: [], truncated: false }),
    [text, delimiter],
  )
  useEffect(() => {
    setVisible(CSV_PAGE_SIZE)
  }, [text, delimiter])

  const header = table.rows[0] ?? []
  const dataRows = table.rows.slice(1)
  const shownRows = dataRows.slice(0, visible)

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between pb-1.5">
        <span className="text-[0.625rem] text-muted-2">
          {dataRows.length > 0 && t('preview.csvSummary', { rows: dataRows.length, cols: header.length })}
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

      {mode === 'table' && table.rows.length > 0 ? (
        <div className="flex min-h-0 flex-1 flex-col">
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
                {shownRows.map((r, rowIdx) => (
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

          {dataRows.length > 0 && (
            <div className="flex shrink-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 pt-1 text-[0.625rem] text-muted-2">
              <span>
                {t('preview.rowsShown', {
                  shown: Math.min(visible, dataRows.length),
                  total: dataRows.length,
                })}
              </span>
              <div className="flex items-center gap-2">
                {table.truncated && <span>{t('preview.rowsTruncated', { count: CSV_MAX_ROWS })}</span>}
                {visible < dataRows.length && (
                  <button
                    type="button"
                    onClick={() => setVisible((prev) => prev + CSV_PAGE_SIZE)}
                    className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
                  >
                    {t('preview.loadMore')}
                  </button>
                )}
              </div>
            </div>
          )}
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
