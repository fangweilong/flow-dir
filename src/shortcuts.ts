import type { Key } from './i18n'

/** Where a binding is resolved: the window (App) or the focused pane (PaneView). */
export type ShortcutScope = 'app' | 'pane'

export type ShortcutId =
  | 'search'
  | 'togglePreview'
  | 'externalPreview'
  | 'toggleSidebar'
  | 'newTab'
  | 'reopenTab'
  | 'closeTab'
  | 'openSettings'
  | 'undo'
  | 'redo'
  | 'open'
  | 'rename'
  | 'delete'
  | 'refresh'
  | 'properties'
  | 'back'
  | 'forward'
  | 'up'
  | 'addressBar'
  | 'selectAll'
  | 'copy'
  | 'cut'
  | 'paste'
  | 'contextMenu'

/** A rebound combination persisted in settings.toml; only changed ids are stored. */
export type ShortcutOverride = { id: string; keys: string }

type Shortcut = { id: ShortcutId; scope: ShortcutScope; label: Key }

export const DEFAULT_BINDINGS: Record<ShortcutId, string> = {
  search: 'ctrl+f',
  togglePreview: 'ctrl+p',
  externalPreview: 'f3',
  toggleSidebar: 'ctrl+b',
  newTab: 'ctrl+t',
  reopenTab: 'ctrl+shift+t',
  closeTab: 'ctrl+w',
  openSettings: 'ctrl+,',
  undo: 'ctrl+z',
  redo: 'ctrl+y',
  open: 'enter',
  rename: 'f2',
  delete: 'delete',
  refresh: 'f5',
  properties: 'alt+enter',
  back: 'alt+left',
  forward: 'alt+right',
  up: 'backspace',
  addressBar: 'ctrl+l',
  selectAll: 'ctrl+a',
  copy: 'ctrl+c',
  cut: 'ctrl+x',
  paste: 'ctrl+v',
  contextMenu: 'shift+f10',
}

export const SHORTCUTS: Shortcut[] = [
  { id: 'search', scope: 'app', label: 'shortcut.search' },
  { id: 'togglePreview', scope: 'app', label: 'shortcut.togglePreview' },
  { id: 'externalPreview', scope: 'app', label: 'shortcut.externalPreview' },
  { id: 'toggleSidebar', scope: 'app', label: 'shortcut.toggleSidebar' },
  { id: 'newTab', scope: 'app', label: 'shortcut.newTab' },
  { id: 'reopenTab', scope: 'app', label: 'shortcut.reopenTab' },
  { id: 'closeTab', scope: 'app', label: 'shortcut.closeTab' },
  { id: 'openSettings', scope: 'app', label: 'shortcut.openSettings' },
  { id: 'undo', scope: 'app', label: 'shortcut.undo' },
  { id: 'redo', scope: 'app', label: 'shortcut.redo' },
  { id: 'open', scope: 'pane', label: 'shortcut.open' },
  { id: 'rename', scope: 'pane', label: 'shortcut.rename' },
  { id: 'delete', scope: 'pane', label: 'shortcut.delete' },
  { id: 'refresh', scope: 'pane', label: 'shortcut.refresh' },
  { id: 'properties', scope: 'pane', label: 'shortcut.properties' },
  { id: 'back', scope: 'pane', label: 'shortcut.back' },
  { id: 'forward', scope: 'pane', label: 'shortcut.forward' },
  { id: 'up', scope: 'pane', label: 'shortcut.up' },
  { id: 'addressBar', scope: 'pane', label: 'shortcut.addressBar' },
  { id: 'selectAll', scope: 'pane', label: 'shortcut.selectAll' },
  { id: 'copy', scope: 'pane', label: 'shortcut.copy' },
  { id: 'cut', scope: 'pane', label: 'shortcut.cut' },
  { id: 'paste', scope: 'pane', label: 'shortcut.paste' },
  { id: 'contextMenu', scope: 'pane', label: 'shortcut.contextMenu' },
]


/** `event.key` spellings that differ from the name users type and see. */
const KEY_ALIASES: Record<string, string> = {
  ' ': 'space',
  spacebar: 'space',
  arrowup: 'up',
  arrowdown: 'down',
  arrowleft: 'left',
  arrowright: 'right',
  escape: 'esc',
  del: 'delete',
  return: 'enter',
  ',': 'comma',
}

const KEY_DISPLAY: Record<string, string> = {
  space: 'Space',
  up: 'Up',
  down: 'Down',
  left: 'Left',
  right: 'Right',
  esc: 'Esc',
  delete: 'Delete',
  backspace: 'Backspace',
  enter: 'Enter',
  tab: 'Tab',
  comma: ',',
}

const canonicalKey = (key: string) => KEY_ALIASES[key.toLowerCase()] ?? key.toLowerCase()

type ParsedBinding = { ctrl: boolean; alt: boolean; shift: boolean; key: string }

/** Parses `ctrl+shift+f`; returns null for text that is not a binding. */
export function parseBinding(binding: string): ParsedBinding | null {
  const tokens = binding
    .split('+')
    .map((token) => token.trim().toLowerCase())
    .filter(Boolean)
  const last = tokens.pop()
  if (!last) return null
  let ctrl = false
  let alt = false
  let shift = false
  for (const token of tokens) {
    if (token === 'ctrl' || token === 'control' || token === 'meta') ctrl = true
    else if (token === 'alt') alt = true
    else if (token === 'shift') shift = true
    else return null
  }
  return { ctrl, alt, shift, key: canonicalKey(last) }
}

/** Human form for the settings list: `Ctrl+Shift+F`. */
export function formatBinding(binding: string): string {
  return bindingLabel(binding, false)
}

/** Menu-hint form; Explorer writes `Del` where the settings list spells `Delete`. */
export function menuHint(binding: string): string {
  return bindingLabel(binding, true)
}

const MENU_KEYS: Record<string, string> = {
  delete: 'Del',
  backspace: 'Bksp',
  enter: 'Enter',
  escape: 'Esc',
  esc: 'Esc',
}

function bindingLabel(binding: string, compact: boolean): string {
  const parsed = parseBinding(binding)
  if (!parsed) return binding
  const parts: string[] = []
  if (parsed.ctrl) parts.push('Ctrl')
  if (parsed.alt) parts.push('Alt')
  if (parsed.shift) parts.push('Shift')
  const short = compact ? MENU_KEYS[parsed.key] : undefined
  parts.push(short ?? KEY_DISPLAY[parsed.key] ?? parsed.key.toUpperCase())
  return parts.join('+')
}

/** Canonical binding for a keydown, or null while only modifiers are held. */
export function bindingFromEvent(event: KeyboardEvent | React.KeyboardEvent): string | null {
  const key = canonicalKey(event.key)
  if (key === 'control' || key === 'alt' || key === 'shift' || key === 'meta') return null
  const parts: string[] = []
  if (event.ctrlKey || event.metaKey) parts.push('ctrl')
  if (event.altKey) parts.push('alt')
  if (event.shiftKey) parts.push('shift')
  parts.push(key)
  return parts.join('+')
}

/**
 * Exact-modifier match. `ignoreShift` lets `Delete` fire for `Shift+Delete` (permanent
 * delete), which shares the same handler; it never relaxes a binding that needs Shift.
 */
export function matchBinding(
  event: KeyboardEvent | React.KeyboardEvent,
  binding: string,
  options?: { ignoreShift?: boolean },
): boolean {
  const parsed = parseBinding(binding)
  if (!parsed) return false
  if ((event.ctrlKey || event.metaKey) !== parsed.ctrl) return false
  if (event.altKey !== parsed.alt) return false
  if (parsed.shift) {
    if (!event.shiftKey) return false
  } else if (event.shiftKey && !options?.ignoreShift) {
    return false
  }
  return canonicalKey(event.key) === parsed.key
}

export function sameBinding(a: string, b: string): boolean {
  const left = parseBinding(a)
  const right = parseBinding(b)
  if (!left || !right) return false
  return left.ctrl === right.ctrl && left.alt === right.alt && left.shift === right.shift && left.key === right.key
}

export const isShortcutId = (value: string): value is ShortcutId =>
  Object.prototype.hasOwnProperty.call(DEFAULT_BINDINGS, value)

/**
 * Combinations the app handles outside the rebindable table. A shortcut bound to one of
 * these would never fire, so the settings dialog refuses them instead of saving a
 * binding that silently does nothing.
 */
export const FIXED_BINDINGS: { keys: string; label: Key }[] = [
  { keys: 'ctrl+tab', label: 'shortcut.fixedTabCycle' },
  { keys: 'ctrl+shift+tab', label: 'shortcut.fixedTabCycle' },
  ...Array.from({ length: 9 }, (_, index) => ({
    keys: `ctrl+${index + 1}`,
    label: 'shortcut.fixedTabJump' as Key,
  })),
  { keys: 'alt+up', label: 'shortcut.fixedUpAlias' },
]

/** Human label of a fixed combination that `binding` would shadow, if any. */
export function fixedBindingName(binding: string, t: (key: Key) => string): string | null {
  const clash = FIXED_BINDINGS.find((entry) => sameBinding(entry.keys, binding))
  return clash ? t(clash.label) : null
}

/** Defaults merged with user overrides; unknown ids and blank keys are ignored. */
export function resolveShortcuts(overrides: ShortcutOverride[] = []): Record<ShortcutId, string> {
  const resolved = { ...DEFAULT_BINDINGS }
  for (const override of overrides) {
    if (isShortcutId(override.id) && parseBinding(override.keys)) resolved[override.id] = override.keys.trim()
  }
  return resolved
}

/** Text-input focus check, used to keep bare-key shortcuts out of text fields. */
export const isTextInput = (target: EventTarget | null): boolean =>
  target instanceof HTMLElement &&
  (target.isContentEditable || target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT')
