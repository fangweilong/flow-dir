import React from 'react'
import type { GitStatus } from '../types'
import type { Translate } from '../i18n'
import type { GitBadges as GitBadgesData } from '../git'

export function GitBranchIcon({ className }: { className: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className={className}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
    >
      <circle cx="4.5" cy="3.5" r="1.75" />
      <circle cx="4.5" cy="12.5" r="1.75" />
      <circle cx="11.5" cy="6" r="1.75" />
      <path d="M4.5 5.25v5.5M11.5 7.75c0 2.5-2.1 3.6-4.6 4" />
    </svg>
  )
}

/** Branch, uncommitted state and upstream distance; visible even when the tree is clean. */
export function GitChip({ git, t }: { git: GitStatus; t: Translate }) {
  const title = [
    git.branch || t('git.detached'),
    git.dirty ? t('git.dirty') : t('git.clean'),
    git.upstream ? '' : t('git.upstreamMissing'),
    git.ahead > 0 ? t('git.ahead', { count: git.ahead }) : '',
    git.behind > 0 ? t('git.behind', { count: git.behind }) : '',
  ]
    .filter(Boolean)
    .join(' · ')
  return (
    <span className="flex min-w-0 max-w-[11.25rem] shrink-0 items-center gap-1" title={title}>
      <GitBranchIcon className="h-3 w-3 shrink-0 text-blue-500" />
      <span className="truncate font-mono text-[0.625rem] text-fg-strong">{git.branch || t('git.detached')}</span>
      {!git.upstream && <span className="shrink-0 text-muted-2">{t('git.upstreamMissingShort')}</span>}
      {git.dirty && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" />}
      {git.ahead > 0 && <span className="shrink-0 text-emerald-500">↑{git.ahead}</span>}
      {git.behind > 0 && <span className="shrink-0 text-red-500">↓{git.behind}</span>}
    </span>
  )
}

function CheckGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-[65%] w-[65%]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M3.4 8.6 6.4 11.6 12.6 4.8" />
    </svg>
  )
}

function PencilGlyph() {
  return (
    <svg viewBox="0 0 16 16" className="h-[65%] w-[65%]" fill="currentColor" aria-hidden>
      <path d="M4 12.4l.9-3.3 5.9-5.9a1.3 1.3 0 0 1 1.8 0l.2.2a1.3 1.3 0 0 1 0 1.8L6.9 11.1 2.9 12.4Z" />
    </svg>
  )
}

function PlusGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-[65%] w-[65%]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      aria-hidden
    >
      <path d="M8 3v10M3 8h10" />
    </svg>
  )
}

function MinusGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-[65%] w-[65%]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      aria-hidden
    >
      <path d="M3 8h10" />
    </svg>
  )
}

function ArrowRightGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-[65%] w-[65%]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M3 8h9M8.5 4.5 12 8l-3.5 3.5" />
    </svg>
  )
}

function ArrowGlyph({ up = false }: { up?: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-[65%] w-[65%]"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={up ? 'M8 12.5V4M4.6 7.4 8 4l3.4 3.4' : 'M8 3.5V12M4.6 8.6 8 12l3.4-3.4'} />
    </svg>
  )
}

function BadgeWrap({
  title,
  tone,
  iconSize,
  children,
}: {
  title: string
  tone: string
  iconSize: number
  children: React.ReactNode
}) {
  const isSmall = iconSize <= 18
  const isLarge = iconSize > 24
  const sizeClass = isSmall
    ? 'h-3 w-3 -bottom-0.5 -right-0.5 text-[0.5rem]'
    : isLarge
      ? 'h-4 w-4 -bottom-1 -right-1 text-[0.625rem]'
      : 'h-3.5 w-3.5 -bottom-0.5 -right-1 text-[0.5625rem]'

  return (
    <span
      title={title}
      className={`absolute grid place-items-center rounded-full text-white ring-1 ring-surface shadow-xs ${sizeClass} ${tone}`}
    >
      {children}
    </span>
  )
}

/**
 * Corner badge on a file or folder icon in a git work tree or child repository:
 * conflict, modified, added, untracked, deleted, renamed, ahead/behind, or clean.
 */
export function GitItemBadge({
  name,
  dir,
  badges,
  iconSize = 20,
  t,
}: {
  name: string
  dir: boolean
  badges: GitBadgesData
  iconSize?: number
  t: Translate
}) {
  const key = name.toLowerCase()

  // 1. Child repository folder
  if (dir && badges.repos.has(key)) {
    const repo = badges.repos.get(key)!
    if (!repo.checked) {
      return <GitBranchIcon className="absolute -bottom-0.5 -right-1 h-3 w-3 text-blue-500 drop-shadow" />
    }
    const title = [
      repo.conflict ? t('git.conflict') : repo.dirty ? t('git.dirty') : t('git.clean'),
      repo.upstream ? '' : t('git.upstreamMissing'),
      repo.ahead > 0 ? t('git.ahead', { count: repo.ahead }) : '',
      repo.behind > 0 ? t('git.behind', { count: repo.behind }) : '',
    ]
      .filter(Boolean)
      .join(' · ')
    const tone = repo.conflict
      ? 'bg-red-500'
      : repo.dirty
        ? 'bg-amber-500'
        : repo.ahead > 0
          ? 'bg-blue-500'
          : repo.behind > 0
            ? 'bg-violet-500'
            : 'bg-emerald-500'
    return (
      <BadgeWrap title={title} tone={tone} iconSize={iconSize}>
        {repo.conflict ? (
          <span className="font-bold leading-none">!</span>
        ) : repo.dirty ? (
          <PencilGlyph />
        ) : repo.ahead > 0 ? (
          <ArrowGlyph up />
        ) : repo.behind > 0 ? (
          <ArrowGlyph />
        ) : (
          <CheckGlyph />
        )}
      </BadgeWrap>
    )
  }

  // 2. Directory inside work tree
  if (dir) {
    if (badges.dirtyDirs.has(key)) {
      return (
        <BadgeWrap title={t('git.dirty')} tone="bg-amber-500" iconSize={iconSize}>
          <PencilGlyph />
        </BadgeWrap>
      )
    }
    if (badges.cleanDirs.has(key)) {
      return (
        <BadgeWrap title={t('git.cleanCommitted')} tone="bg-emerald-500" iconSize={iconSize}>
          <CheckGlyph />
        </BadgeWrap>
      )
    }
    return null
  }

  // 3. File inside work tree
  const code = badges.changed.get(key)
  if (code) {
    if (code === '!' || code === 'UU' || code === 'AA') {
      return (
        <BadgeWrap title={`${t('git.conflict')} (${code})`} tone="bg-red-500" iconSize={iconSize}>
          <span className="font-bold leading-none">!</span>
        </BadgeWrap>
      )
    }
    if (code === 'M') {
      return (
        <BadgeWrap title={`${t('git.modified')} (${code})`} tone="bg-amber-500" iconSize={iconSize}>
          <PencilGlyph />
        </BadgeWrap>
      )
    }
    if (code === 'A') {
      return (
        <BadgeWrap title={`${t('git.added')} (${code})`} tone="bg-emerald-500" iconSize={iconSize}>
          <PlusGlyph />
        </BadgeWrap>
      )
    }
    if (code === 'D') {
      return (
        <BadgeWrap title={`${t('git.deleted')} (${code})`} tone="bg-rose-500" iconSize={iconSize}>
          <MinusGlyph />
        </BadgeWrap>
      )
    }
    if (code === 'R' || code === 'C') {
      return (
        <BadgeWrap title={`${t('git.renamed')} (${code})`} tone="bg-sky-500" iconSize={iconSize}>
          <ArrowRightGlyph />
        </BadgeWrap>
      )
    }
    if (code === '?' || code === 'U') {
      return (
        <BadgeWrap title={`${t('git.untracked')} (${code})`} tone="bg-sky-500" iconSize={iconSize}>
          <span className="font-bold leading-none">?</span>
        </BadgeWrap>
      )
    }
    return (
      <BadgeWrap title={code} tone="bg-amber-500" iconSize={iconSize}>
        <PencilGlyph />
      </BadgeWrap>
    )
  }

  // Tracked file with no uncommitted change
  if (badges.cleanFiles.has(key)) {
    return (
      <BadgeWrap title={t('git.cleanCommitted')} tone="bg-emerald-500" iconSize={iconSize}>
        <CheckGlyph />
      </BadgeWrap>
    )
  }

  return null
}
