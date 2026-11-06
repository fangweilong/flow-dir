import { describe, expect, it } from 'vitest'
import { parseDelimited } from './csv'

describe('parseDelimited', () => {
  it('parses quotes, escaped quotes and embedded newlines', () => {
    const { rows, truncated } = parseDelimited(
      'name,note\r\n"Smith, J","said ""hi""\nnext"\nlast,plain\n',
      ',',
    )
    expect(truncated).toBe(false)
    expect(rows).toEqual([
      ['name', 'note'],
      ['Smith, J', 'said "hi"\nnext'],
      ['last', 'plain'],
    ])
  })

  it('parses tab-separated input', () => {
    expect(parseDelimited('a\tb\n1\t2', '\t').rows).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ])
  })

  it('stops at the row cap and reports truncation only for remaining data', () => {
    const capped = parseDelimited('a\n1\n2\n3', ',', 2)
    expect(capped.rows).toEqual([['a'], ['1']])
    expect(capped.truncated).toBe(true)

    const exact = parseDelimited('a\n1\n', ',', 2)
    expect(exact.rows).toEqual([['a'], ['1']])
    expect(exact.truncated).toBe(false)
  })

  it('drops columns past the cap and reports truncation', () => {
    const { rows, truncated } = parseDelimited('a,b,c\n1,2,3', ',', 10, 2)
    expect(rows).toEqual([
      ['a', 'b'],
      ['1', '2'],
    ])
    expect(truncated).toBe(true)
  })

  it('handles empty input', () => {
    expect(parseDelimited('', ',')).toEqual({ rows: [], truncated: false })
  })
})
