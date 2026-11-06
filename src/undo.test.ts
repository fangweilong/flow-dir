import { describe, expect, it } from 'vitest'
import { undoEntryFor } from './undo'

describe('undoEntryFor', () => {
  const sources = ['C:\\Work\\a.txt', 'C:\\Work\\b.txt']

  it('returns null for an empty batch and for a fully skipped one', () => {
    expect(undoEntryFor('copy', [], { targetDir: 'D:\\' })).toBeNull()
    expect(undoEntryFor('copy', sources, { targetDir: 'D:\\', skip: sources })).toBeNull()
  })

  it('records partial batches with positional rename names kept aligned', () => {
    expect(
      undoEntryFor('rename', sources, { newNames: ['x.txt', 'y.txt'], skip: ['c:\\WORK\\a.txt'] }),
    ).toEqual({
      kind: 'rename',
      items: [{ path: 'C:\\Work\\b.txt', oldName: 'b.txt', newName: 'y.txt' }],
    })
  })

  it('records a single rename from newName', () => {
    expect(undoEntryFor('rename', ['C:\\Work\\a.txt'], { newName: 'z.txt' })).toEqual({
      kind: 'rename',
      items: [{ path: 'C:\\Work\\a.txt', oldName: 'a.txt', newName: 'z.txt' }],
    })
    // A newName cannot name more than one source.
    expect(undoEntryFor('rename', sources, { newName: 'z.txt' })).toBeNull()
  })

  it('maps moves back to their source folders', () => {
    expect(undoEntryFor('move', sources, { targetDir: 'D:\\Archive' })).toEqual({
      kind: 'move',
      items: [
        { from: 'C:\\Work\\a.txt', to: 'D:\\Archive\\a.txt' },
        { from: 'C:\\Work\\b.txt', to: 'D:\\Archive\\b.txt' },
      ],
    })
  })

  it('records created copies for deletion on undo', () => {
    expect(undoEntryFor('copy', sources, { targetDir: 'D:\\Archive\\' })).toEqual({
      kind: 'copy',
      created: ['D:\\Archive\\a.txt', 'D:\\Archive\\b.txt'],
    })
  })

  it('records recycle-bin deletes but not permanent deletes', () => {
    expect(undoEntryFor('delete', sources)).toEqual({ kind: 'delete', paths: sources })
    expect(undoEntryFor('delete', sources, { permanent: true })).toBeNull()
  })

  it('has no inverse for folder creation', () => {
    expect(undoEntryFor('newdir', sources)).toBeNull()
  })

  it('ignores operations missing their destination', () => {
    expect(undoEntryFor('move', sources)).toBeNull()
    expect(undoEntryFor('copy', sources)).toBeNull()
  })
})
