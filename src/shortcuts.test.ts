import { describe, expect, it } from 'vitest'
import {
  DEFAULT_BINDINGS,
  bindingFromEvent,
  fixedBindingName,
  formatBinding,
  isShortcutId,
  matchBinding,
  menuHint,
  parseBinding,
  resolveShortcuts,
  sameBinding,
} from './shortcuts'

type EventShape = Pick<KeyboardEvent, 'key' | 'ctrlKey' | 'altKey' | 'shiftKey' | 'metaKey'>
const event = (shape: Partial<EventShape> & { key: string }) =>
  ({ ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...shape }) as KeyboardEvent

describe('parseBinding', () => {
  it('parses modifiers and the key', () => {
    expect(parseBinding('ctrl+shift+f')).toEqual({ ctrl: true, alt: false, shift: true, key: 'f' })
    expect(parseBinding(' Alt + Enter ')).toEqual({ ctrl: false, alt: true, shift: false, key: 'enter' })
  })

  it('treats a trailing plus as the key itself', () => {
    expect(parseBinding('ctrl+shift++')).toEqual({ ctrl: true, alt: false, shift: true, key: '+' })
  })

  it('normalizes key aliases', () => {
    expect(parseBinding('del')?.key).toBe('delete')
    expect(parseBinding('arrowup')?.key).toBe('up')
    expect(parseBinding('spacebar')?.key).toBe('space')
    expect(parseBinding(',')?.key).toBe('comma')
  })

  it('rejects unknown modifiers and blank input', () => {
    expect(parseBinding('')).toBeNull()
    expect(parseBinding('   ')).toBeNull()
    expect(parseBinding('hyper+f')).toBeNull()
  })
})

describe('formatting', () => {
  it('renders the settings form and the compact menu form', () => {
    expect(formatBinding('ctrl+shift+f')).toBe('Ctrl+Shift+F')
    expect(formatBinding('delete')).toBe('Delete')
    expect(menuHint('delete')).toBe('Del')
    expect(menuHint('backspace')).toBe('Bksp')
    expect(menuHint('ctrl+comma')).toBe('Ctrl+,')
  })

  it('returns unparseable text unchanged', () => {
    expect(formatBinding('hyper+f')).toBe('hyper+f')
  })
})

describe('bindingFromEvent', () => {
  it('canonicalizes the pressed combination', () => {
    expect(bindingFromEvent(event({ key: 'F5' }))).toBe('f5')
    expect(bindingFromEvent(event({ key: 'ArrowUp', altKey: true }))).toBe('alt+up')
    expect(bindingFromEvent(event({ key: 'F', ctrlKey: true, shiftKey: true }))).toBe('ctrl+shift+f')
    expect(bindingFromEvent(event({ key: 'Delete' }))).toBe('delete')
  })

  it('treats Meta as Ctrl', () => {
    expect(bindingFromEvent(event({ key: 'c', metaKey: true }))).toBe('ctrl+c')
  })

  it('ignores modifier-only keydowns', () => {
    expect(bindingFromEvent(event({ key: 'Control', ctrlKey: true }))).toBeNull()
    expect(bindingFromEvent(event({ key: 'Shift', shiftKey: true }))).toBeNull()
  })
})

describe('matchBinding', () => {
  it('requires an exact modifier match', () => {
    expect(matchBinding(event({ key: 'f' }), 'f')).toBe(true)
    expect(matchBinding(event({ key: 'f', ctrlKey: true }), 'f')).toBe(false)
    expect(matchBinding(event({ key: 'f', ctrlKey: true }), 'ctrl+f')).toBe(true)
    expect(matchBinding(event({ key: 'f', ctrlKey: true, shiftKey: true }), 'ctrl+f')).toBe(false)
  })

  it('lets Delete match Shift+Delete only when asked', () => {
    expect(matchBinding(event({ key: 'Delete', shiftKey: true }), 'delete')).toBe(false)
    expect(matchBinding(event({ key: 'Delete', shiftKey: true }), 'delete', { ignoreShift: true })).toBe(true)
    // ignoreShift never relaxes a binding that itself requires Shift.
    expect(matchBinding(event({ key: 'Delete' }), 'shift+delete', { ignoreShift: true })).toBe(false)
  })
})

describe('resolveShortcuts', () => {
  it('merges valid overrides over the defaults', () => {
    const keys = resolveShortcuts([
      { id: 'search', keys: 'ctrl+shift+f' },
      { id: 'rename', keys: '   ' },
      { id: 'notARealAction', keys: 'f1' },
    ])
    expect(keys.search).toBe('ctrl+shift+f')
    expect(keys.rename).toBe(DEFAULT_BINDINGS.rename)
    expect(keys.open).toBe(DEFAULT_BINDINGS.open)
    expect('notARealAction' in keys).toBe(false)
  })

  it('ignores malformed bindings instead of losing the default', () => {
    const keys = resolveShortcuts([{ id: 'copy', keys: 'hyper+c' }])
    expect(keys.copy).toBe(DEFAULT_BINDINGS.copy)
  })
})

describe('identity helpers', () => {
  it('compares bindings by meaning, not spelling', () => {
    expect(sameBinding('Ctrl+Shift+F', 'shift+ctrl+f')).toBe(true)
    expect(sameBinding('ctrl+del', 'ctrl+delete')).toBe(true)
    expect(sameBinding('ctrl+f', 'ctrl+shift+f')).toBe(false)
    expect(sameBinding('nonsense', 'ctrl+f')).toBe(false)
  })

  it('detects fixed combinations that a rebind would shadow', () => {
    const translate = (key: string) => `t:${key}`
    expect(fixedBindingName('ctrl+tab', translate)).toBe('t:shortcut.fixedTabCycle')
    expect(fixedBindingName('alt+up', translate)).toBe('t:shortcut.fixedUpAlias')
    expect(fixedBindingName('ctrl+alt+up', translate)).toBeNull()
  })

  it('knows the rebindable ids', () => {
    expect(isShortcutId('search')).toBe(true)
    expect(isShortcutId('toString')).toBe(false)
  })
})
