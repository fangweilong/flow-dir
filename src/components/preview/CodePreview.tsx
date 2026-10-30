import { useMemo, useState } from 'react'
import type { Translate } from '../../i18n'

type Props = {
  text: string | null | undefined
  name: string
  t: Translate
}

type TokenKind = 'comment' | 'string' | 'keyword' | 'type' | 'number' | 'boolean' | 'plain'

interface Token {
  kind: TokenKind
  text: string
}

const toLookup = (words: string[]): Record<string, true> => {
  const map: Record<string, true> = {}
  for (const w of words) map[w] = true
  return map
}

const KEYWORDS_BY_EXT: Record<string, Record<string, true>> = {
  rs: toLookup([
    'as', 'async', 'await', 'break', 'const', 'continue', 'crate', 'dyn', 'else', 'enum',
    'extern', 'false', 'fn', 'for', 'if', 'impl', 'in', 'let', 'loop', 'match', 'mod',
    'move', 'mut', 'pub', 'ref', 'return', 'self', 'Self', 'static', 'struct', 'super',
    'trait', 'true', 'type', 'unsafe', 'use', 'where', 'while', 'Ok', 'Err', 'Some', 'None',
  ]),
  ts: toLookup([
    'as', 'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue', 'debugger',
    'default', 'delete', 'do', 'else', 'enum', 'export', 'extends', 'false', 'finally', 'for',
    'from', 'function', 'get', 'if', 'implements', 'import', 'in', 'instanceof', 'interface',
    'is', 'keyof', 'let', 'new', 'null', 'package', 'private', 'protected', 'public', 'readonly',
    'return', 'set', 'static', 'super', 'switch', 'this', 'throw', 'true', 'try', 'type',
    'typeof', 'undefined', 'var', 'void', 'while', 'with', 'yield',
  ]),
  js: toLookup([
    'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue', 'debugger',
    'default', 'delete', 'do', 'else', 'export', 'extends', 'false', 'finally', 'for',
    'from', 'function', 'if', 'import', 'in', 'instanceof', 'let', 'new', 'null', 'return',
    'static', 'super', 'switch', 'this', 'throw', 'true', 'try', 'typeof', 'undefined',
    'var', 'void', 'while', 'with', 'yield',
  ]),
  py: toLookup([
    'and', 'as', 'assert', 'async', 'await', 'break', 'class', 'continue', 'def', 'del',
    'elif', 'else', 'except', 'False', 'finally', 'for', 'from', 'global', 'if', 'import',
    'in', 'is', 'lambda', 'None', 'nonlocal', 'not', 'or', 'pass', 'raise', 'return',
    'True', 'try', 'while', 'with', 'yield',
  ]),
  go: toLookup([
    'break', 'case', 'chan', 'const', 'continue', 'default', 'defer', 'else', 'fallthrough',
    'for', 'func', 'go', 'goto', 'if', 'import', 'interface', 'map', 'package', 'range',
    'return', 'select', 'struct', 'switch', 'type', 'var', 'true', 'false', 'nil',
  ]),
  toml: toLookup(['true', 'false']),
  yaml: toLookup(['true', 'false', 'null', 'yes', 'no']),
  yml: toLookup(['true', 'false', 'null', 'yes', 'no']),
  sql: toLookup([
    'select', 'from', 'where', 'insert', 'into', 'update', 'delete', 'join', 'left', 'right',
    'inner', 'outer', 'on', 'group', 'by', 'order', 'having', 'limit', 'offset', 'as',
    'and', 'or', 'not', 'null', 'is', 'in', 'between', 'like', 'create', 'table', 'drop',
    'alter', 'index', 'primary', 'key', 'foreign', 'references', 'distinct', 'union',
  ]),
}
// Aliases
KEYWORDS_BY_EXT.tsx = KEYWORDS_BY_EXT.ts
KEYWORDS_BY_EXT.jsx = KEYWORDS_BY_EXT.js
KEYWORDS_BY_EXT.mjs = KEYWORDS_BY_EXT.js
KEYWORDS_BY_EXT.cjs = KEYWORDS_BY_EXT.js

function tokenizeLine(line: string, ext: string): Token[] {
  const tokens: Token[] = []
  const keywords = KEYWORDS_BY_EXT[ext]

  let i = 0
  const len = line.length

  // Check single-line comment at start
  const trimmed = line.trimStart()
  const indent = line.slice(0, line.length - trimmed.length)

  // Shell / Python / YAML comments: #
  const isHashComment = (ext === 'py' || ext === 'sh' || ext === 'yaml' || ext === 'yml' || ext === 'toml') && trimmed.startsWith('#')
  // C-style comments: //
  const isSlashComment = trimmed.startsWith('//')
  // SQL comments: --
  const isDashComment = (ext === 'sql') && trimmed.startsWith('--')

  if (isHashComment || isSlashComment || isDashComment) {
    if (indent) tokens.push({ kind: 'plain', text: indent })
    tokens.push({ kind: 'comment', text: trimmed })
    return tokens
  }

  while (i < len) {
    const ch = line[i]

    // Trailing comment
    if (
      (ch === '/' && line[i + 1] === '/') ||
      ((ext === 'py' || ext === 'sh' || ext === 'toml' || ext === 'yaml' || ext === 'yml') && ch === '#') ||
      (ext === 'sql' && ch === '-' && line[i + 1] === '-')
    ) {
      tokens.push({ kind: 'comment', text: line.slice(i) })
      break
    }

    // String literals
    if (ch === '"' || ch === "'" || ch === '`') {
      const quote = ch
      let j = i + 1
      while (j < len && line[j] !== quote) {
        if (line[j] === '\\' && j + 1 < len) j++
        j++
      }
      if (j < len) j++ // include quote
      tokens.push({ kind: 'string', text: line.slice(i, j) })
      i = j
      continue
    }

    // Numbers
    if (/\d/.test(ch) && (i === 0 || !/[a-zA-Z0-9_$]/.test(line[i - 1]))) {
      let j = i
      while (j < len && /[0-9a-fA-FxX_.]/.test(line[j])) j++
      tokens.push({ kind: 'number', text: line.slice(i, j) })
      i = j
      continue
    }

    // Identifiers & keywords
    if (/[a-zA-Z_$]/.test(ch)) {
      let j = i
      while (j < len && /[a-zA-Z0-9_$]/.test(line[j])) j++
      const word = line.slice(i, j)
      const isKw = Boolean(keywords && (keywords[word] || (ext === 'sql' && keywords[word.toLowerCase()])))
      const isType = /^[A-Z][a-zA-Z0-9_$]*$/.test(word)
      const isBool = word === 'true' || word === 'false' || word === 'null' || word === 'None' || word === 'True' || word === 'False'

      if (isBool) {
        tokens.push({ kind: 'boolean', text: word })
      } else if (isKw) {
        tokens.push({ kind: 'keyword', text: word })
      } else if (isType && (ext === 'rs' || ext === 'ts' || ext === 'tsx' || ext === 'go' || ext === 'java')) {
        tokens.push({ kind: 'type', text: word })
      } else {
        tokens.push({ kind: 'plain', text: word })
      }
      i = j
      continue
    }

    // Plain characters/symbols
    let j = i
    while (j < len && !/[a-zA-Z0-9_$"'/`#-]/.test(line[j])) j++
    if (j === i) j++
    tokens.push({ kind: 'plain', text: line.slice(i, j) })
    i = j
  }

  return tokens
}

export function CodePreview({ text, name, t }: Props) {
  const [wrap, setWrap] = useState(false)
  const [showLineNumbers, setShowLineNumbers] = useState(true)
  const [copied, setCopied] = useState(false)

  const ext = useMemo(() => {
    const dot = name.lastIndexOf('.')
    return dot >= 0 ? name.slice(dot + 1).toLowerCase() : ''
  }, [name])

  const lines = useMemo(() => {
    if (!text) return []
    return text.split(/\r?\n/)
  }, [text])

  const tokenizedLines = useMemo(() => {
    return lines.map((line) => tokenizeLine(line, ext))
  }, [lines, ext])

  const copyAll = () => {
    if (!text) return
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    })
  }

  if (!text) {
    return <div className="grid h-full place-items-center text-[0.71875rem] text-muted-2">{t('preview.empty')}</div>
  }

  const lineCountDigits = Math.max(2, String(lines.length).length)

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-line pb-1.5 text-[0.625rem]">
        <div className="flex items-center gap-2 text-muted-2">
          {ext && (
            <span className="rounded bg-surface-3 px-1.5 py-0.5 font-mono uppercase font-semibold text-muted">
              {ext}
            </span>
          )}
          <span>{lines.length} lines</span>
          <span>{text.length} chars</span>
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setShowLineNumbers((prev) => !prev)}
            className={`rounded px-1.5 py-0.5 ${
              showLineNumbers ? 'bg-sel font-medium text-fg-strong' : 'text-muted hover:bg-surface-3'
            }`}
            title="Toggle line numbers"
          >
            #
          </button>
          <button
            type="button"
            onClick={() => setWrap((prev) => !prev)}
            className={`rounded px-1.5 py-0.5 ${
              wrap ? 'bg-sel font-medium text-fg-strong' : 'text-muted hover:bg-surface-3'
            }`}
            title="Toggle wrap"
          >
            Wrap
          </button>
          <button
            type="button"
            onClick={copyAll}
            className="rounded px-1.5 py-0.5 text-muted hover:bg-surface-3 hover:text-fg"
          >
            {copied ? t('tools.copyDone') : t('tools.copy')}
          </button>
        </div>
      </div>

      <div
        data-selectable
        className="min-h-0 flex-1 overflow-auto font-mono text-[0.6875rem] leading-relaxed"
      >
        <div className="flex w-max min-w-full">
          {showLineNumbers && (
            <div
              className="select-none shrink-0 border-r border-line/40 bg-surface-2/40 py-1 pr-2.5 text-right font-mono text-muted-2"
              style={{ minWidth: `${lineCountDigits * 0.55 + 1.2}rem` }}
            >
              {lines.map((_, i) => (
                <div key={i} className="px-1">{i + 1}</div>
              ))}
            </div>
          )}
          <div
            className={`min-w-0 flex-1 py-1 pl-3 pr-2 text-fg ${
              wrap ? 'whitespace-pre-wrap break-words' : 'whitespace-pre'
            }`}
          >
            {tokenizedLines.map((tokens, lineIdx) => (
              <div key={lineIdx} className="hover:bg-surface-2/30">
                {tokens.length === 0 ? (
                  '\u00A0'
                ) : (
                  tokens.map((token, tokIdx) => {
                    let cls = 'text-fg'
                    switch (token.kind) {
                      case 'comment':
                        cls = 'text-muted-2 italic'
                        break
                      case 'string':
                        cls = 'text-emerald-600 dark:text-emerald-400'
                        break
                      case 'keyword':
                        cls = 'text-purple-600 dark:text-purple-400 font-medium'
                        break
                      case 'type':
                        cls = 'text-amber-600 dark:text-amber-400'
                        break
                      case 'number':
                        cls = 'text-blue-600 dark:text-blue-400'
                        break
                      case 'boolean':
                        cls = 'text-rose-600 dark:text-rose-400 font-semibold'
                        break
                      default:
                        cls = 'text-fg'
                    }
                    return (
                      <span key={tokIdx} className={cls}>
                        {token.text}
                      </span>
                    )
                  })
                )}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
