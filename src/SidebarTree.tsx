import { memo } from 'react'
import { isFsPath, joinPath, pathKey } from './paths'
import type { Drive } from './types'

/** Indent per level, capped so a deep tree cannot consume the whole sidebar. */
const INDENT_STEP = 12
const INDENT_MAX_DEPTH = 16

/** One folder level: expanded state and fetched children keyed by {@link pathKey}. */
type TreeState = {
  /** open: key → display path, so a cached folder keeps its own casing. */
  open: Record<string, string>
  /** Children names per folder key; missing means "not fetched yet". */
  nodes: Record<string, string[] | undefined>
}

type Props = TreeState & {
  drives: Drive[]
  /** Folders whose child list hit the scan cap, keyed by {@link pathKey}. */
  truncated: Record<string, boolean>
  /** Row text marking a truncated folder; the sidebar owns no translations itself. */
  truncatedLabel: string
  /** Highlighted row: the selected folder, or the current one when nothing is selected. */
  selectedPath: string
  /** Folder the active tab is in, drawn bold so orientation survives a file selection. */
  currentPath: string
  onToggle: (path: string) => void
  onOpen: (path: string) => void
}

/**
 * Folder tree under the sidebar's "此电脑". Rows are pure presentation: the parent
 * owns caches and expansion so tab switches and filesystem refreshes never remount
 * the tree, and a cached folder paints on the first frame after expanding.
 */
export function SidebarTree({
  drives,
  open,
  nodes,
  truncated,
  truncatedLabel,
  selectedPath,
  currentPath,
  onToggle,
  onOpen,
}: Props) {
  const selectedKey = pathKey(selectedPath)
  const currentKey = pathKey(currentPath)
  return (
    <div data-sidebar-tree>
      {drives.map((drive) => (
        <TreeRow
          key={pathKey(drive.root)}
          path={drive.root}
          name={drive.name}
          depth={0}
          glyph={drive.kind === 'location' ? 'network' : 'drive'}
          open={open}
          nodes={nodes}
          truncated={truncated}
          truncatedLabel={truncatedLabel}
          selectedKey={selectedKey}
          currentKey={currentKey}
          onToggle={onToggle}
          onOpen={onOpen}
        />
      ))}
    </div>
  )
}

type RowProps = TreeState & {
  path: string
  name: string
  depth: number
  glyph: 'drive' | 'network' | 'folder'
  truncated: Record<string, boolean>
  truncatedLabel: string
  selectedKey: string
  currentKey: string
  onToggle: (path: string) => void
  onOpen: (path: string) => void
}

/**
 * Memoised because the sidebar re-renders with every list selection change while its
 * own inputs stay identical; only expansion, fetches and the active path reach rows.
 */
const TreeRow = memo(function TreeRow({
  path,
  name,
  depth,
  glyph,
  open,
  nodes,
  truncated,
  truncatedLabel,
  selectedKey,
  currentKey,
  onToggle,
  onOpen,
}: RowProps) {
  const key = pathKey(path)
  const expanded = key in open
  const selected = key === selectedKey
  const current = !selected && key === currentKey
  const expandable = isFsPath(path)
  const kids = expanded ? nodes[key] : undefined
  return (
    <>
      <div
        data-tree-key={key}
        data-drop-dir={expandable ? path : undefined}
        className={`flex h-[1.375rem] items-center rounded pr-1 ${
          selected
            ? 'bg-sel font-medium text-fg-strong'
            : current
              ? 'font-medium text-fg-strong hover:bg-surface-3'
              : 'text-fg hover:bg-surface-3'
        }`}
        style={{ paddingLeft: 2 + Math.min(depth, INDENT_MAX_DEPTH) * INDENT_STEP }}
      >
        {expandable ? (
          <button
            type="button"
            aria-expanded={expanded}
            onClick={() => onToggle(path)}
            className="grid h-4 w-4 shrink-0 place-items-center rounded text-muted-2 hover:text-fg"
          >
            <svg
              viewBox="0 0 16 16"
              className={`h-2.5 w-2.5 transition-transform ${expanded ? 'rotate-90' : ''}`}
              fill="none"
              stroke="currentColor"
              strokeWidth="1.8"
            >
              <path d="M6 4l4 4-4 4" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        ) : (
          <span className="h-4 w-4 shrink-0" />
        )}
        <button
          type="button"
          onClick={() => onOpen(path)}
          title={path}
          className="flex min-w-0 flex-1 items-center gap-1.5 py-[3px] text-left text-[0.75rem]"
        >
          <Glyph kind={glyph} active={selected} />
          <span className="truncate">{name}</span>
        </button>
      </div>
      {expanded &&
        kids?.map((kid) => {
          const child = joinPath(path, kid)
          return (
            <TreeRow
              key={pathKey(child)}
              path={child}
              name={kid}
              depth={depth + 1}
              glyph="folder"
              open={open}
              nodes={nodes}
              truncated={truncated}
              truncatedLabel={truncatedLabel}
              selectedKey={selectedKey}
              currentKey={currentKey}
              onToggle={onToggle}
              onOpen={onOpen}
            />
          )
        })}
      {expanded && truncated[key] && (
        <div
          data-tree-truncated={key}
          title={truncatedLabel}
          className="flex h-[1.375rem] items-center rounded pr-1 text-[0.6875rem] text-muted-2"
          style={{ paddingLeft: 2 + Math.min(depth + 1, INDENT_MAX_DEPTH) * INDENT_STEP }}
        >
          <span className="h-4 w-4 shrink-0" />
          <span className="truncate">{truncatedLabel}</span>
        </div>
      )}
    </>
  )
})

/** Static glyphs: the tree must not queue one shell-icon request per folder. */
function Glyph({ kind, active }: { kind: 'drive' | 'network' | 'folder'; active: boolean }) {
  const tone = active ? 'text-blue-500' : 'text-muted-2'
  if (kind === 'drive') {
    return (
      <svg viewBox="0 0 16 16" className={`h-4 w-4 shrink-0 ${tone}`} fill="currentColor">
        <path d="M2 5.5C2 4.7 2.7 4 3.5 4h9c.8 0 1.5.7 1.5 1.5v5c0 .8-.7 1.5-1.5 1.5h-9C2.7 12 2 11.3 2 10.5v-5Zm2 4.3h8v.7H4v-.7Z" />
      </svg>
    )
  }
  if (kind === 'network') {
    return (
      <svg viewBox="0 0 16 16" className={`h-4 w-4 shrink-0 ${tone}`} fill="none" stroke="currentColor" strokeWidth="1.3">
        <circle cx="8" cy="8" r="5.6" />
        <path d="M2.4 8h11.2M8 2.4c1.6 1.6 2.4 3.5 2.4 5.6S9.6 12 8 13.6C6.4 12 5.6 10.1 5.6 8S6.4 4 8 2.4Z" />
      </svg>
    )
  }
  return (
    <svg viewBox="0 0 16 16" className={`h-4 w-4 shrink-0 ${tone}`} fill="currentColor">
      <path d="M1.5 3.6c0-.6.5-1.1 1.1-1.1h3.2c.4 0 .7.15.95.42l.85.93h6.3c.6 0 1.1.5 1.1 1.1v7.4c0 .6-.5 1.1-1.1 1.1H2.6c-.6 0-1.1-.5-1.1-1.1V3.6Z" />
    </svg>
  )
}
