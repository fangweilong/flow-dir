import { beforeEach, describe, expect, it } from 'vitest'
import {
  baseName,
  crumbs,
  formatSize,
  hasArchiveAncestor,
  isArchiveMember,
  isArchiveName,
  isDir,
  isFsPath,
  isInsideArchive,
  isShellPath,
  isWebdavPath,
  joinPath,
  markVirtualRoot,
  parentPath,
  pathKey,
  pathLabel,
  resolveAddress,
  sameVolume,
  setWebdavConnections,
  treeChain,
  underPath,
  volumeRoot,
  webdavParts,
} from './paths'

beforeEach(() => {
  setWebdavConnections([])
})

describe('pathKey', () => {
  it('drops trailing separators for local paths but keeps drive roots', () => {
    expect(pathKey('C:\\Work\\')).toBe('c:\\work')
    expect(pathKey('C:\\Work')).toBe('c:\\work')
    expect(pathKey('C:\\')).toBe('c:\\')
    expect(pathKey('\\\\server\\share\\')).toBe('\\\\server\\share')
  })

  it('keeps WebDAV case and drops only trailing slashes', () => {
    expect(pathKey('webdav://nas/Photos/')).toBe('webdav://nas/Photos')
    expect(pathKey('webdav://nas/Photos')).not.toBe(pathKey('webdav://nas/photos'))
  })
})

describe('joinPath / parentPath / baseName', () => {
  it('joins local paths with exactly one separator', () => {
    expect(joinPath('C:\\', 'file.txt')).toBe('C:\\file.txt')
    expect(joinPath('C:\\Work', 'file.txt')).toBe('C:\\Work\\file.txt')
  })

  it('keeps webdav addresses on forward slashes', () => {
    expect(joinPath('webdav://nas/', 'photos')).toBe('webdav://nas/photos')
    expect(joinPath('webdav://nas/photos/', 'a.jpg')).toBe('webdav://nas/photos/a.jpg')
  })

  it('climbs to the drive/UNC root and stops there', () => {
    expect(parentPath('C:\\Work\\a.txt')).toBe('C:\\Work')
    expect(parentPath('C:\\Work')).toBe('C:\\')
    expect(parentPath('C:\\')).toBe('')
    expect(parentPath('\\\\server\\share')).toBe('')
    expect(parentPath('\\\\server\\share\\dir')).toBe('\\\\server\\share')
  })

  it('climbs webdav paths back to the connection root', () => {
    expect(parentPath('webdav://nas/a/b.txt')).toBe('webdav://nas/a')
    expect(parentPath('webdav://nas/a')).toBe('webdav://nas/')
    expect(parentPath('webdav://nas/')).toBe('')
  })

  it('takes the trailing segment with either separator', () => {
    expect(baseName('C:\\Work\\a.txt')).toBe('a.txt')
    expect(baseName('webdav://nas/a/b.txt')).toBe('b.txt')
    expect(baseName('plain')).toBe('plain')
  })
})

describe('resolveAddress', () => {
  it('strips quotes pasted from Explorer', () => {
    expect(resolveAddress('C:\\Work', '"D:\\Data"')).toBe('D:\\Data')
  })

  it('passes absolute drives, UNC and shell namespaces through', () => {
    expect(resolveAddress('C:\\Work', 'D:\\Data')).toBe('D:\\Data')
    expect(resolveAddress('C:\\Work', '\\\\server\\share\\x')).toBe('\\\\server\\share\\x')
    expect(resolveAddress('C:\\Work', 'shell:RecycleBinFolder')).toBe('shell:RecycleBinFolder')
    expect(resolveAddress('C:\\Work', 'D:')).toBe('D:\\')
  })

  it('normalizes relative segments and never climbs above the root', () => {
    expect(resolveAddress('C:\\Work\\Sub', '..\\file.txt')).toBe('C:\\Work\\file.txt')
    expect(resolveAddress('C:\\Work', '..\\..\\..\\etc')).toBe('C:\\etc')
    expect(resolveAddress('C:\\', '..\\x')).toBe('C:\\x')
    expect(resolveAddress('\\\\server\\share\\dir', '..\\..\\..\\x')).toBe('\\\\server\\share\\x')
  })

  it('resolves relative webdav names with forward slashes', () => {
    expect(resolveAddress('webdav://nas/a', '../b/c')).toBe('webdav://nas/b/c')
    expect(resolveAddress('webdav://nas/a/', 'c')).toBe('webdav://nas/a/c')
  })

  it('returns an empty path only for blank input', () => {
    expect(resolveAddress('C:\\Work', '   ')).toBe('')
    expect(resolveAddress('C:\\Work', '')).toBe('')
  })
})

describe('webdav addressing', () => {
  beforeEach(() => {
    setWebdavConnections([{ id: 'nas', name: 'My NAS' }])
  })

  it('resolves ids and display names to the stable id', () => {
    expect(webdavParts('webdav://NAS/photos/2024')).toEqual({ id: 'nas', segments: ['photos', '2024'] })
    expect(webdavParts('webdav://my nas/photos')).toEqual({ id: 'nas', segments: ['photos'] })
  })

  it('normalizes dots and duplicate slashes', () => {
    expect(webdavParts('webdav://nas//a/./b/../c')).toEqual({ id: 'nas', segments: ['a', 'c'] })
  })

  it('labels breadcrumbs with the connection name, not the id', () => {
    expect(crumbs('webdav://nas/a/b')).toEqual([
      { label: 'My NAS', path: 'webdav://nas/' },
      { label: 'a', path: 'webdav://nas/a' },
      { label: 'b', path: 'webdav://nas/a/b' },
    ])
    expect(pathLabel('webdav://nas/a/b')).toBe('b')
    expect(pathLabel('webdav://nas/')).toBe('My NAS')
  })

  it('treats one connection as one volume', () => {
    expect(sameVolume('webdav://nas/a', 'webdav://nas/b')).toBe(true)
    expect(sameVolume('webdav://nas/a', 'webdav://other/b')).toBe(false)
    expect(sameVolume('C:\\a', 'D:\\b')).toBe(false)
    expect(sameVolume('C:\\a', 'c:\\b')).toBe(true)
    expect(sameVolume('\\\\srv\\share\\a', '\\\\srv\\share\\b')).toBe(true)
  })
})

describe('tree navigation helpers', () => {
  it('builds breadcrumbs from a drive path', () => {
    expect(crumbs('C:\\Work\\a')).toEqual([
      { label: 'C:', path: 'C:\\' },
      { label: 'Work', path: 'C:\\Work' },
      { label: 'a', path: 'C:\\Work\\a' },
    ])
  })

  it('keeps a UNC share as one root crumb', () => {
    expect(crumbs('\\\\server\\share\\a\\b')).toEqual([
      { label: '\\\\server\\share', path: '\\\\server\\share' },
      { label: 'a', path: '\\\\server\\share\\a' },
      { label: 'b', path: '\\\\server\\share\\a\\b' },
    ])
  })

  it('walks ancestors deepest first', () => {
    expect(treeChain('C:\\a\\b')).toEqual(['C:\\a\\b', 'C:\\a', 'C:\\'])
  })

  it('recognizes volumes and namespace paths', () => {
    expect(volumeRoot('C:\\Work\\a')).toBe('C:\\')
    expect(volumeRoot('\\\\server\\share\\a')).toBe('\\\\server\\share')
    expect(volumeRoot('shell:RecycleBinFolder')).toBe('')
    expect(isFsPath('C:\\x')).toBe(true)
    expect(isFsPath('\\\\server\\share')).toBe(true)
    expect(isFsPath('webdav://nas/x')).toBe(false)
    expect(isShellPath('::20D04FE0-3AEA-1069-A2D8-08002B30309D')).toBe(true)
    expect(isWebdavPath('WEBDAV://nas')).toBe(true)
  })

  it('matches underPath only at segment boundaries', () => {
    expect(underPath('C:\\Work\\a', 'C:\\Work')).toBe(true)
    expect(underPath('C:\\work', 'C:\\Work')).toBe(true)
    expect(underPath('C:\\Workbench', 'C:\\Work')).toBe(false)
    expect(underPath('webdav://nas/a/b', 'webdav://nas/a')).toBe(true)
    expect(underPath('webdav://nas/a/b', 'webdav://nas/a/')).toBe(true)
    expect(underPath('webdav://nas/ab', 'webdav://nas/a')).toBe(false)
    // WebDAV servers may be case-sensitive; only local paths are folded.
    expect(underPath('webdav://nas/Foo/x', 'webdav://nas/Foo')).toBe(true)
    expect(underPath('webdav://nas/Foo/x', 'webdav://nas/foo')).toBe(false)
  })
})

describe('archive path detection', () => {
  it('recognizes supported archive extensions case-insensitively', () => {
    expect(isArchiveName('backup.ZIP')).toBe(true)
    expect(isArchiveName('data.tar.gz')).toBe(true)
    expect(isArchiveName('notes.txt')).toBe(false)
  })

  it('only treats listed archive roots as virtual folders', () => {
    markVirtualRoot('C:\\data\\backup.zip', true)
    expect(isInsideArchive('C:\\data\\backup.zip')).toBe(true)
    expect(isInsideArchive('C:\\DATA\\BACKUP.ZIP\\sub\\file.txt')).toBe(true)
    expect(isInsideArchive('C:\\data\\other\\file.txt')).toBe(false)
    markVirtualRoot('C:\\data\\backup.zip', false)
    expect(isInsideArchive('C:\\data\\backup.zip\\file.txt')).toBe(false)
  })

  it('distinguishes the archive itself from a member', () => {
    markVirtualRoot('C:\\data\\backup.zip', true)
    expect(isArchiveMember('C:\\data\\backup.zip')).toBe(false)
    expect(isArchiveMember('C:\\data\\backup.zip\\file.txt')).toBe(true)
  })

  it('detects archive ancestors for clipboard paths restored after a restart', () => {
    expect(hasArchiveAncestor('C:\\data\\backup.zip\\file.txt')).toBe(true)
    expect(hasArchiveAncestor('C:\\data\\file.txt')).toBe(false)
  })
})

describe('formatting', () => {
  it('formats sizes with binary units', () => {
    expect(formatSize(0, false)).toBe('0 B')
    expect(formatSize(1536, false)).toBe('1.5 KB')
    expect(formatSize(10 * 1024 * 1024, false)).toBe('10 MB')
    expect(formatSize(0, true)).toBe('')
    expect(formatSize(Number.NaN, false)).toBe('—')
  })

  it('reads the directory attribute bit', () => {
    expect(isDir(0x10)).toBe(true)
    expect(isDir(0x20)).toBe(false)
  })
})
