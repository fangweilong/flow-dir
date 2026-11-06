import { describe, expect, it } from 'vitest'
import { buildCompareOptions, csvCell, decideAction, parentRel } from './plan'
import type { DecideInput } from './plan'
import type { CompareEntry, CompareMeta } from '../types'

const file = (mtime = 1_000, size = 10): CompareMeta => ({ size, mtime, isDir: false, attrs: 0 })
const dir = (mtime = 1_000): CompareMeta => ({ size: 0, mtime, isDir: true, attrs: 0x10 })

const entry = (patch: Partial<CompareEntry> & Pick<CompareEntry, 'left' | 'right'>): CompareEntry => ({
  rel: 'a.txt',
  name: 'a.txt',
  status: 'different',
  ...patch,
})

const decide = (patch: Partial<DecideInput> & Pick<DecideInput, 'entry'>) =>
  decideAction({
    leftRoot: 'C:\\Left',
    rightRoot: 'D:\\Right',
    strategy: 'l2r',
    deleteExtras: false,
    hashError: false,
    toleranceMs: 0,
    ...patch,
  })

describe('decideAction', () => {
  it('copies a left-only file to the right root', () => {
    expect(decide({ entry: entry({ rel: 'sub\\a.txt', left: file(), right: null }) })).toEqual({
      kind: 'copy',
      source: 'C:\\Left\\sub\\a.txt',
      targetDir: 'D:\\Right\\sub',
    })
  })

  it('copies a right-only file to the left root when syncing right-to-left', () => {
    expect(
      decide({ entry: entry({ left: null, right: file() }), strategy: 'r2l' }),
    ).toEqual({ kind: 'copy', source: 'D:\\Right\\a.txt', targetDir: 'C:\\Left\\' })
  })

  it('deletes extras only when asked', () => {
    const leftOnly = entry({ left: file(), right: null })
    const rightOnly = entry({ left: null, right: file() })
    expect(decide({ entry: leftOnly })).toMatchObject({ kind: 'copy' })
    expect(decide({ entry: leftOnly, strategy: 'r2l', deleteExtras: false })).toBeNull()
    expect(decide({ entry: leftOnly, strategy: 'r2l', deleteExtras: true })).toEqual({
      kind: 'delete',
      paths: ['C:\\Left\\a.txt'],
    })
    expect(decide({ entry: rightOnly, strategy: 'l2r', deleteExtras: true })).toEqual({
      kind: 'delete',
      paths: ['D:\\Right\\a.txt'],
    })
  })

  it('leaves matching folders alone and reports type mismatches', () => {
    expect(decide({ entry: entry({ left: dir(), right: dir() }) })).toBeNull()
    expect(decide({ entry: entry({ left: dir(), right: file() }) })).toBe('mismatch')
    expect(decide({ entry: entry({ left: file(), right: dir() }) })).toBe('mismatch')
  })

  it('copies folders that exist on one side', () => {
    expect(decide({ entry: entry({ rel: 'photos', left: dir(), right: null }) })).toEqual({
      kind: 'copy',
      source: 'C:\\Left\\photos',
      targetDir: 'D:\\Right\\',
    })
  })

  it('skips identical files and unreadable pairs', () => {
    expect(decide({ entry: entry({ status: 'same', left: file(), right: file() }) })).toBeNull()
    expect(
      decide({ entry: entry({ left: file(), right: file() }), hashError: true }),
    ).toBeNull()
  })

  it('never copies from a side the filters hid', () => {
    const pair = entry({ left: file(), right: file() })
    expect(decide({ entry: { ...pair, leftCompared: false } })).toBeNull()
    expect(decide({ entry: { ...pair, rightCompared: false }, strategy: 'r2l' })).toBeNull()
  })

  it('uses the newer side for a bidirectional sync', () => {
    const newerLeft = entry({ left: file(2_000), right: file(1_000) })
    const newerRight = entry({ left: file(1_000), right: file(2_000) })
    expect(decide({ entry: newerLeft, strategy: 'both' })).toEqual({
      kind: 'copy',
      source: 'C:\\Left\\a.txt',
      targetDir: 'D:\\Right\\',
    })
    expect(decide({ entry: newerRight, strategy: 'both' })).toEqual({
      kind: 'copy',
      source: 'D:\\Right\\a.txt',
      targetDir: 'C:\\Left\\',
    })
  })

  it('reports a conflict when timestamps differ within tolerance', () => {
    const pair = entry({ left: file(2_000), right: file(1_000) })
    expect(decide({ entry: pair, strategy: 'both', toleranceMs: 1_000 })).toBe('conflict')
    expect(decide({ entry: pair, strategy: 'both', toleranceMs: 999 })).toMatchObject({ kind: 'copy' })
  })
})

describe('buildCompareOptions', () => {
  const now = 1_700_000_000_000

  it('converts MB, days and seconds into backend units', () => {
    expect(
      buildCompareOptions({
        includeHidden: true,
        extensions: ' .TXT, log;md ',
        minMb: '2',
        maxMb: '',
        days: '1.5',
        exclude: 'node_modules, .git',
        toleranceSec: '2.5',
        now,
      }),
    ).toEqual({
      includeHidden: true,
      extensions: ['txt', 'log', 'md'],
      minSize: 2 * 1024 * 1024,
      maxSize: 0,
      afterMs: now - 1.5 * 86_400_000,
      beforeMs: 0,
      exclude: ['node_modules', '.git'],
      toleranceMs: 2_500,
    })
  })

  it('treats missing or invalid filters as unset', () => {
    expect(
      buildCompareOptions({
        includeHidden: false,
        extensions: '',
        minMb: '-3',
        maxMb: 'abc',
        days: '0',
        exclude: '',
        toleranceSec: '-1',
      }),
    ).toMatchObject({ minSize: 0, maxSize: 0, afterMs: 0, toleranceMs: 0, extensions: [], exclude: [] })
  })
})

describe('csvCell', () => {
  it('escapes quotes and quotes every field', () => {
    expect(csvCell('plain')).toBe('"plain"')
    expect(csvCell('a"b')).toBe('"a""b"')
    expect(csvCell(42)).toBe('"42"')
  })

  it('neutralizes spreadsheet formulas', () => {
    expect(csvCell('=cmd|calc')).toBe("\"'=cmd|calc\"")
    expect(csvCell('+1')).toBe("\"'+1\"")
    expect(csvCell('-1')).toBe("\"'-1\"")
    expect(csvCell('@x')).toBe("\"'@x\"")
  })
})

describe('parentRel', () => {
  it('returns the parent-relative folder or the empty root', () => {
    expect(parentRel('a\\b.txt')).toBe('a')
    expect(parentRel('a\\b\\c.txt')).toBe('a\\b')
    expect(parentRel('a.txt')).toBe('')
  })
})
