import type { Translate } from '../i18n'
import type { CompareStatus } from '../types'

/** Human label for a row/filter chip. */
export function statusLabel(status: CompareStatus, t: Translate): string {
  switch (status) {
    case 'left':
      return t('compare.status.left')
    case 'right':
      return t('compare.status.right')
    case 'different':
      return t('compare.status.different')
    default:
      return t('compare.status.same')
  }
}

export function statusClass(status: CompareStatus): string {
  switch (status) {
    case 'left':
      return 'text-rose-600 dark:text-rose-400'
    case 'right':
      return 'text-emerald-600 dark:text-emerald-400'
    case 'different':
      return 'text-warning-strong dark:text-warning-soft'
    default:
      return 'text-muted-2'
  }
}

/** git-diff markers: `+` only on the right, `-` only on the left, `~` modified, `=` same. */
export const statusMark = (status: CompareStatus) =>
  status === 'right' ? '+' : status === 'left' ? '-' : status === 'different' ? '~' : '='
