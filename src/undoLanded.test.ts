import { describe, expect, it } from 'vitest'
import { landedIndices } from './undoLanded'
import type { OpFailure } from './types'

const sources = ['C:\\Work\\a.txt', 'C:\\Work\\b.txt', 'C:\\Work\\c.txt']
const failure = (path: string): OpFailure => ({ path, code: 'DENIED', detail: '' })

describe('landedIndices', () => {
  it('trusts the authoritative succeeded list', () => {
    expect(
      landedIndices(sources, { succeeded: [2], failed: [], failedCount: 99 }),
    ).toEqual([2])
  })

  it('returns nothing for an aborted op without a succeeded list', () => {
    expect(
      landedIndices(sources, { succeeded: [], failed: [], failedCount: 0, aborted: true }),
    ).toEqual([])
  })

  it('uses the complete failed-indices list even past the failure cap', () => {
    expect(
      landedIndices(sources, {
        failedIndices: [0, 2],
        failed: [failure(sources[0])],
        failedCount: 2,
      }),
    ).toEqual([1])
  })

  it('uses the path list only when it is complete', () => {
    expect(
      landedIndices(sources, {
        failed: [failure('c:\\WORK\\b.txt')],
        failedCount: 1,
      }),
    ).toEqual([0, 2])
  })

  it('does not infer from a capped failure list', () => {
    expect(
      landedIndices(sources, {
        failed: [failure(sources[0])],
        failedCount: 2,
      }),
    ).toEqual([])
  })

  it('ignores a batch-level failure with an empty path', () => {
    expect(
      landedIndices(sources, {
        failed: [failure('')],
        failedCount: 1,
      }),
    ).toEqual([])
  })

  it('handles an empty batch', () => {
    expect(landedIndices([], { failed: [], failedCount: 0 })).toEqual([])
  })
})
