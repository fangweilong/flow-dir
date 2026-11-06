import { useEffect, useState } from 'react'
import type { SheetPreview } from '../../types'
import type { Translate } from '../../i18n'

/** Rows added to the DOM per "show more" click; the backend parses up to 2000. */
const SHEET_PAGE_SIZE = 200

/**
 * Read-only worksheet view. Column widths are content-based because the
 * backend does not carry the Excel width metadata; the row gutter keeps large
 * sheets navigable while the container scrolls in both directions. Rows are
 * paged so a large parsed sheet does not put every cell in the DOM at once.
 */
export function XlsxPreview({ sheets, t }: { sheets: SheetPreview[] | null | undefined; t: Translate }) {
  const [requested, setRequested] = useState(0)
  const [visible, setVisible] = useState(SHEET_PAGE_SIZE)
  const active = sheets && sheets.length > 0 ? Math.min(requested, sheets.length - 1) : 0
  useEffect(() => {
    setVisible(SHEET_PAGE_SIZE)
  }, [active, sheets])
  if (!sheets || sheets.length === 0) {
    return <div className="grid h-full place-items-center text-[0.71875rem] text-muted-2">{t('preview.officeEmpty')}</div>
  }
  const sheet = sheets[active]
  const rows = sheet.rows.slice(0, visible)
  return (
    <div className="flex h-full min-h-0 flex-col">
      {sheets.length > 1 && (
        <div className="flex shrink-0 gap-1 overflow-x-auto pb-1.5">
          {sheets.map((item, index) => (
            <button
              key={`${item.name}-${index}`}
              type="button"
              onClick={() => setRequested(index)}
              className={`shrink-0 rounded px-2 py-0.5 text-[0.625rem] ${
                index === active ? 'bg-sel font-medium text-fg-strong' : 'text-muted hover:bg-surface-3'
              }`}
            >
              {item.name}
            </button>
          ))}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto rounded border border-line">
        <table className="border-collapse text-[0.71875rem]">
          <tbody>
            {rows.map((row, rowIndex) => (
              <tr key={rowIndex} className="even:bg-surface-2/40">
                <th className="sticky left-0 z-[1] border-r border-b border-line bg-surface-2 px-1.5 py-0.5 text-right font-mono text-[0.625rem] font-normal text-muted-2">
                  {rowIndex + 1}
                </th>
                {row.map((cell, cellIndex) => (
                  <td
                    key={cellIndex}
                    className="border-r border-b border-line px-2 py-0.5 align-top whitespace-pre-wrap text-fg"
                  >
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
        {sheet.rows.length === 0 && (
          <div className="px-2 py-3 text-[0.6875rem] text-muted-2">{t('preview.officeEmpty')}</div>
        )}
      </div>

      {sheet.rows.length > 0 && (
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-x-3 gap-y-1 pt-1 text-[0.625rem] text-muted-2">
          <span>
            {t('preview.rowsShown', {
              shown: Math.min(visible, sheet.rows.length),
              total: sheet.rows.length,
            })}
          </span>
          <div className="flex items-center gap-2">
            {sheet.truncated && <span>{t('preview.officeTruncated')}</span>}
            {visible < sheet.rows.length && (
              <button
                type="button"
                onClick={() => setVisible((prev) => prev + SHEET_PAGE_SIZE)}
                className="rounded bg-surface-2 px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
              >
                {t('preview.loadMore')}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
