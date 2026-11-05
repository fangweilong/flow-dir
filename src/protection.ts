import type { OpKind } from './types'

/**
 * A destructive operation the backend refused because its sources touch a protected
 * path. `block` paths can only be acknowledged; `confirm` paths can proceed after the
 * user types `word`, which is sent back with the retried request.
 */
export type ProtectPromptState = {
  mode: 'block' | 'confirm'
  paths: string[]
  /** Typed confirmation required in `confirm` mode; empty in `block` mode. */
  word: string
  onConfirm: () => void
  onCancel: () => void
}

const CONFIRM_WORDS: Partial<Record<OpKind, string>> = {
  delete: 'DELETE',
  move: 'MOVE',
  rename: 'RENAME',
}

export const protectWord = (kind: OpKind): string => CONFIRM_WORDS[kind] ?? 'CONFIRM'

/**
 * Turns an `op_start` rejection into the protected paths it names. Other error codes
 * return `null` so the caller can keep its ordinary failure handling.
 */
export function protectionError(
  err: unknown,
): { code: 'PROTECTED' | 'NEEDS_CONFIRM'; paths: string[] } | null {
  if (!err || typeof err !== 'object' || !('code' in err)) return null
  const code = (err as { code: unknown }).code
  if (code !== 'PROTECTED' && code !== 'NEEDS_CONFIRM') return null
  const detail = (err as { detail?: unknown }).detail
  const paths =
    typeof detail === 'string'
      ? detail
          .split('\n')
          .map((path) => path.trim())
          .filter(Boolean)
      : []
  return { code, paths }
}
