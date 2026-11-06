import { describe, expect, it } from 'vitest'
import { matchTypeAhead } from './typeahead'

describe('matchTypeAhead', () => {
  const names = ['alpha.txt', 'report.txt', 'rpt.log', 'delta.txt']

  it('returns -1 for an empty buffer', () => {
    expect(matchTypeAhead(names, '')).toBe(-1)
    expect(matchTypeAhead(names, '   ')).toBe(-1)
  })

  it('matches a prefix case-insensitively', () => {
    expect(matchTypeAhead(names, 'REP')).toBe(1)
    expect(matchTypeAhead(names, 'delta')).toBe(3)
  })

  it('prefers any prefix over a subsequence, wherever it appears', () => {
    expect(matchTypeAhead(names, 'rpt')).toBe(2)
  })

  it('falls back to the first left-to-right subsequence', () => {
    expect(matchTypeAhead(names, 'rptz')).toBe(-1)
    expect(matchTypeAhead(['report.txt', 'other.log'], 'rpt')).toBe(0)
  })

  it('returns -1 when nothing matches', () => {
    expect(matchTypeAhead(names, 'zzz')).toBe(-1)
    expect(matchTypeAhead([], 'a')).toBe(-1)
  })
})
