import type { DocBlock, DocRun, OfficeDoc } from '../../types'
import type { Translate } from '../../i18n'

const HEADING_CLASS: Record<number, string> = {
  1: 'text-[1.125rem] leading-snug font-bold',
  2: 'text-[1rem] leading-snug font-semibold',
  3: 'text-[0.875rem] leading-snug font-semibold',
  4: 'text-[0.8125rem] leading-snug font-semibold',
}

/** Renders runs with their bold/italic flags; unknown spacing is preserved. */
function Runs({ runs, fallback }: { runs: DocRun[]; fallback: string }) {
  if (!runs.length) return <>{fallback}</>
  return (
    <>
      {runs.map((run, index) => (
        <span
          key={index}
          className={`${run.bold ? 'font-semibold' : ''} ${run.italic ? 'italic' : ''}`}
        >
          {run.text}
        </span>
      ))}
    </>
  )
}

function Block({ block }: { block: DocBlock }) {
  if (block.kind === 'table') {
    const header = block.header && block.rows.length > 0 ? block.rows[0] : null
    const body = header ? block.rows.slice(1) : block.rows
    return (
      <div className="my-2 overflow-x-auto">
        <table className="w-full border-collapse text-[0.71875rem]">
          {header && (
            <thead>
              <tr>
                {header.map((cell, index) => (
                  <th
                    key={index}
                    className="border border-line bg-surface-2 px-2 py-1 text-left font-semibold text-fg-strong"
                  >
                    {cell}
                  </th>
                ))}
              </tr>
            </thead>
          )}
          <tbody>
            {body.map((row, rowIndex) => (
              <tr key={rowIndex}>
                {row.map((cell, cellIndex) => (
                  <td
                    key={cellIndex}
                    className="border border-line px-2 py-1 align-top whitespace-pre-wrap text-fg"
                  >
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    )
  }

  if (block.kind === 'heading') {
    const level = Math.min(Math.max(block.level ?? 1, 1), 4)
    return (
      <div className={`mt-3 mb-1 text-fg-strong ${HEADING_CLASS[level]}`}>
        <Runs runs={block.runs} fallback={block.text} />
      </div>
    )
  }

  if (block.kind === 'list') {
    const indent = Math.min(Math.max((block.level ?? 1) - 1, 0), 6) * 1.25
    return (
      <div className="flex gap-2 text-[0.8125rem] leading-relaxed text-fg" style={{ paddingLeft: `${indent}rem` }}>
        <span className="shrink-0 text-muted-2">•</span>
        <span className="min-w-0 whitespace-pre-wrap break-words">
          <Runs runs={block.runs} fallback={block.text} />
        </span>
      </div>
    )
  }

  return (
    <p className="my-1 whitespace-pre-wrap break-words text-[0.8125rem] leading-relaxed text-fg">
      <Runs runs={block.runs} fallback={block.text} />
    </p>
  )
}

/** Read-only Word preview: headings, styled runs, lists and tables. */
export function DocxPreview({ doc, t }: { doc: OfficeDoc | null | undefined; t: Translate }) {
  if (!doc || doc.blocks.length === 0) {
    return <div className="grid h-full place-items-center text-[0.71875rem] text-muted-2">{t('preview.officeEmpty')}</div>
  }
  return (
    <div className="mx-auto max-w-[52rem] px-2 pb-4">
      {doc.blocks.map((block, index) => (
        <Block key={index} block={block} />
      ))}
      {doc.truncated && (
        <div className="mt-3 rounded bg-surface-2 px-2 py-1 text-[0.625rem] text-muted-2">
          {t('preview.officeTruncated')}
        </div>
      )}
    </div>
  )
}
