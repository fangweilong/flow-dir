import { describe, expect, it } from 'vitest'
import {
  adoptIds,
  buildPreset,
  createWorkspace,
  paneIds,
  pushHistory,
  removePane,
  replacePane,
  samePath,
  setRatio,
  splitPane,
  stepHistory,
  uid,
} from './workspace'
import type { Node, TabState, Workspace } from './workspace'

const tab = (path: string, history: string[], histIdx: number): TabState => ({
  id: 't1',
  path,
  history,
  histIdx,
})

describe('samePath', () => {
  it('folds case, separators and trailing separators', () => {
    expect(samePath('C:\\Work', 'c:/work/')).toBe(true)
    expect(samePath('C:\\Work', 'C:\\Workbench')).toBe(false)
  })

  it('keeps a drive root distinct from a drive-relative path', () => {
    expect(samePath('C:\\', 'c:')).toBe(true)
    expect(samePath('C:\\', 'C:\\Work')).toBe(false)
  })
})

describe('history', () => {
  it('moves the cursor without touching the stack', () => {
    const current = tab('C:\\b', ['C:\\', 'C:\\a', 'C:\\b'], 2)
    expect(stepHistory(current, -1)).toMatchObject({ path: 'C:\\a', histIdx: 1 })
    expect(stepHistory(current, 1)).toBeNull()
    expect(stepHistory(current, 0)).toBeNull()
  })

  it('refuses to step past either edge', () => {
    const first = tab('C:\\', ['C:\\'], 0)
    expect(stepHistory(first, -1)).toBeNull()
    expect(stepHistory(first, 1)).toBeNull()
  })

  it('appends a new navigation', () => {
    const current = tab('C:\\a', ['C:\\', 'C:\\a'], 1)
    expect(pushHistory(current, 'C:\\b')).toMatchObject({
      path: 'C:\\b',
      history: ['C:\\', 'C:\\a', 'C:\\b'],
      histIdx: 2,
    })
  })

  it('truncates the forward tail after going back', () => {
    const current = tab('C:\\a', ['C:\\', 'C:\\a', 'C:\\b'], 1)
    expect(pushHistory(current, 'C:\\c')).toMatchObject({
      path: 'C:\\c',
      history: ['C:\\', 'C:\\a', 'C:\\c'],
      histIdx: 2,
    })
  })

  it('is a no-op for a rescan of the same folder', () => {
    const current = tab('C:\\a', ['C:\\', 'C:\\a'], 1)
    expect(pushHistory(current, 'c:/A/')).toBe(current)
  })
})

describe('pane tree', () => {
  const root: Node = {
    t: 'split',
    id: 's1',
    dir: 'row',
    ratio: 0.5,
    a: { t: 'pane', id: 'p1' },
    b: { t: 'pane', id: 'p2' },
  }

  it('lists panes left to right', () => {
    expect(paneIds(root)).toEqual(['p1', 'p2'])
    expect(paneIds({ t: 'pane', id: 'p9' })).toEqual(['p9'])
  })

  it('replaces only the targeted pane', () => {
    const next = replacePane(root, 'p2', { t: 'pane', id: 'p3' })
    expect(paneIds(next)).toEqual(['p1', 'p3'])
    expect(replacePane(root, 'missing', { t: 'pane', id: 'p9' })).toEqual(root)
  })

  it('collapses a split when one side is removed', () => {
    expect(removePane(root, 'p2')).toEqual({ t: 'pane', id: 'p1' })
    expect(removePane(root, 'p1')).toEqual({ t: 'pane', id: 'p2' })
    expect(removePane({ t: 'pane', id: 'p1' }, 'p1')).toBeNull()
  })

  it('updates only the named split ratio', () => {
    const next = setRatio(root, 's1', 0.7)
    expect(next).toMatchObject({ ratio: 0.7 })
    expect(setRatio(root, 'nope', 0.7)).toEqual(root)
  })

  it('splits a pane before or after the target', () => {
    const after = splitPane(root, 'p1', 'col', 'after', 'p9')
    expect(after).toMatchObject({
      id: 's1',
      dir: 'row',
      a: {
        t: 'split',
        dir: 'col',
        ratio: 0.5,
        a: { t: 'pane', id: 'p1' },
        b: { t: 'pane', id: 'p9' },
      },
      b: { t: 'pane', id: 'p2' },
    })
    const before = splitPane(root, 'p1', 'row', 'before', 'p9')
    expect(before).toMatchObject({
      a: { t: 'split', a: { t: 'pane', id: 'p9' }, b: { t: 'pane', id: 'p1' } },
      b: { t: 'pane', id: 'p2' },
    })
    expect(splitPane(root, 'p2', 'row', 'after', 'p9')).toMatchObject({
      a: { t: 'pane', id: 'p1' },
      b: { t: 'split', a: { t: 'pane', id: 'p2' }, b: { t: 'pane', id: 'p9' } },
    })
  })

  it('builds each layout preset', () => {
    const ids = ['p1', 'p2', 'p3', 'p4']
    expect(paneIds(buildPreset(ids, 1, 'h'))).toEqual(['p1'])
    expect(paneIds(buildPreset(ids, 2, 'v'))).toEqual(['p1', 'p2'])
    expect(paneIds(buildPreset(ids, 3, 'h'))).toEqual(['p1', 'p2', 'p3'])
    expect(paneIds(buildPreset(ids, 4, 'h'))).toEqual(['p1', 'p2', 'p3', 'p4'])
    expect(buildPreset(ids, 2, 'v')).toMatchObject({ t: 'split', dir: 'col' })
    expect(buildPreset(ids, 2, 'h')).toMatchObject({ t: 'split', dir: 'row' })
  })
})

describe('createWorkspace / adoptIds', () => {
  it('creates one pane with one tab at the home folder', () => {
    const ws = createWorkspace('C:\\Users\\me')
    expect(ws.root).toEqual({ t: 'pane', id: ws.activePaneId })
    const pane = ws.panes[ws.activePaneId]
    expect(pane.tabIds).toHaveLength(1)
    expect(ws.tabs[pane.activeTabId]).toMatchObject({
      path: 'C:\\Users\\me',
      history: ['C:\\Users\\me'],
      histIdx: 0,
    })
    expect(pane.hidden).toBe(false)
  })

  it('advances the id counter past restored ids', () => {
    const ws: Workspace = {
      root: {
        t: 'split',
        id: 's5',
        dir: 'row',
        ratio: 0.5,
        a: { t: 'pane', id: 'p3' },
        b: { t: 'pane', id: 'p9' },
      },
      panes: {
        p3: { id: 'p3', tabIds: ['t7'], activeTabId: 't7', sort: { key: 'name', dir: 'asc' }, hidden: false },
        p9: { id: 'p9', tabIds: ['t900'], activeTabId: 't900', sort: { key: 'name', dir: 'asc' }, hidden: false },
      },
      tabs: {
        t7: { id: 't7', path: 'C:\\', history: ['C:\\'], histIdx: 0 },
        t900: { id: 't900', path: 'C:\\', history: ['C:\\'], histIdx: 0 },
      },
      activePaneId: 'p3',
    }
    adoptIds(ws)
    expect(uid('p')).toBe('p901')
    expect(uid('t')).toBe('t902')
  })
})
