import type { Translate } from '../i18n'
import { sameVolume } from '../paths'
import { FileIcon } from '../icons'
import type { OsDragState, RowDragState } from '../hooks/useDragAndDrop'

/**
 * Live feedback for a file drag: a blue outline over the folder/tab that will receive the
 * drop and, for in-app drags, a pill that follows the pointer with the pending effect.
 * Without this the list gives no hint that rows are draggable at all.
 */
export function DragOverlay({
  rowDrag,
  osDrag,
  t,
}: {
  rowDrag: RowDragState | null
  osDrag: OsDragState | null
  t: Translate
}) {
  const target = rowDrag?.target ?? osDrag?.target ?? null
  if (!rowDrag && !target) return null
  let hint = ''
  if (rowDrag) {
    const count = rowDrag.names.length
    if (target) {
      const copy = rowDrag.copy || (!rowDrag.move && !sameVolume(rowDrag.sourceDir, target.dir))
      hint = t(copy ? 'drag.copyCount' : 'drag.moveCount', { count })
    } else if (count === 1) {
      hint = rowDrag.names[0] ?? ''
    } else {
      hint = `${rowDrag.names[0] ?? ''} +${count - 1}`
    }
  }
  return (
    <>
      {target && (
        <div
          data-drag-target
          className="pointer-events-none fixed z-[59] rounded-[3px] bg-blue-400/10 ring-2 ring-inset ring-blue-400/70"
          style={{
            left: target.left,
            top: target.top,
            width: target.width,
            height: target.height,
          }}
        />
      )}
      {rowDrag && (
        <div
          data-drag-ghost
          className="pointer-events-none fixed z-[60] flex max-w-[20rem] items-center gap-1.5 rounded-md border border-line bg-surface-2 px-2 py-1 text-[0.75rem] text-fg-strong shadow-lg"
          style={{ left: rowDrag.x + 14, top: rowDrag.y + 14 }}
        >
          <FileIcon name={rowDrag.names[0] ?? ''} size={16} />
          <span className="truncate">{hint}</span>
        </div>
      )}
    </>
  )
}
