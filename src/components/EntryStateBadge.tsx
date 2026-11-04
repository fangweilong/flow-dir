import type { Translate } from '../i18n'

const ATTR_HIDDEN = 0x2
const ATTR_SYSTEM = 0x4
const ATTR_REPARSE_POINT = 0x400
/** OneDrive placeholders carry the reparse bit too; they are files, not links. */
const ATTR_CLOUD = 0x1000 | 0x40000 | 0x400000

/** Matches the backend's hidden filter: both HIDDEN and SYSTEM are shown or hidden together. */
export const isHiddenEntry = (attrs: number) => (attrs & (ATTR_HIDDEN | ATTR_SYSTEM)) !== 0

/** `.lnk` shortcuts plus symlinks/junctions; cloud placeholders are excluded. */
export const isLinkEntry = (name: string, attrs: number) =>
  /\.lnk$/i.test(name) || ((attrs & ATTR_REPARSE_POINT) !== 0 && (attrs & ATTR_CLOUD) === 0)

export const entryStateTitle = (name: string, attrs: number, t: Translate): string =>
  [isHiddenEntry(attrs) ? t('list.hiddenItem') : '', isLinkEntry(name, attrs) ? t('list.linkItem') : '']
    .filter(Boolean)
    .join(' · ')

function LinkGlyph() {
  return (
    <svg viewBox="0 0 16 16" className="h-[70%] w-[70%]" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
      <path d="M6.2 9.8a2.6 2.6 0 0 0 3.7 0l2.2-2.2a2.6 2.6 0 0 0-3.7-3.7l-1 1" />
      <path d="M9.8 6.2a2.6 2.6 0 0 0-3.7 0L3.9 8.4a2.6 2.6 0 0 0 3.7 3.7l1-1" />
    </svg>
  )
}

function HiddenGlyph() {
  return (
    <svg viewBox="0 0 16 16" className="h-[70%] w-[70%]" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden>
      <path d="M2 8s2.2-3.5 6-3.5S14 8 14 8s-2.2 3.5-6 3.5S2 8 2 8Z" />
      <circle cx="8" cy="8" r="1.6" />
      <path d="M3 3l10 10" />
    </svg>
  )
}

function StateBadge({ kind, title }: { kind: 'hidden' | 'link'; title: string }) {
  return (
    <span
      title={title}
      className={`absolute grid h-3.5 w-3.5 place-items-center rounded-full text-white ring-1 ring-surface shadow-xs ${
        kind === 'link' ? '-top-1 -left-1 bg-blue-500' : '-top-1 -right-1 bg-slate-400'
      }`}
    >
      {kind === 'link' ? <LinkGlyph /> : <HiddenGlyph />}
    </span>
  )
}

/** Overlay for the icon wrapper, which must be `relative`. */
export function EntryStateBadges({ name, attrs, t }: { name: string; attrs: number; t: Translate }) {
  const hidden = isHiddenEntry(attrs)
  const link = isLinkEntry(name, attrs)
  return (
    <>
      {hidden && <StateBadge kind="hidden" title={t('list.hiddenItem')} />}
      {link && <StateBadge kind="link" title={t('list.linkItem')} />}
    </>
  )
}

/** Name color shared by the file list, search overlay and folder comparison. */
export const entryStateTone = (name: string, attrs: number): string => {
  const hidden = isHiddenEntry(attrs)
  if (isLinkEntry(name, attrs)) return hidden ? 'text-blue-500/70' : 'text-blue-500'
  return hidden ? 'text-muted-2' : ''
}
