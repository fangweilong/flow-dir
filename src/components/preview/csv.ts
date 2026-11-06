/** Table limits. Parsing more than this is rarely useful in a preview panel. */
export const CSV_MAX_ROWS = 10_000
export const CSV_MAX_COLS = 200
/** Rows added to the DOM per "show more" click. */
export const CSV_PAGE_SIZE = 500

export type CsvTable = {
  rows: string[][]
  /** The source continued past the row/column cap. */
  truncated: boolean
}

/**
 * Minimal RFC 4180-style parser for previews: quoted delimiters, escaped quotes and
 * embedded newlines are supported. Rendering is paginated separately, so parsing the
 * whole (bounded) text does not imply a huge DOM.
 */
export function parseDelimited(
  text: string,
  delimiter: string,
  maxRows = CSV_MAX_ROWS,
  maxCols = CSV_MAX_COLS,
): CsvTable {
  const rows: string[][] = []
  let row: string[] = []
  let cell = ''
  let inQuotes = false
  let truncated = false

  const addCell = () => {
    if (row.length < maxCols) row.push(cell)
    else truncated = true
    cell = ''
  }

  for (let i = 0; i < text.length; i++) {
    const ch = text[i]
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          cell += '"'
          i++
        } else {
          inQuotes = false
        }
      } else {
        cell += ch
      }
    } else if (ch === '"') {
      inQuotes = true
    } else if (ch === delimiter) {
      addCell()
    } else if (ch === '\n' || ch === '\r') {
      if (ch === '\r' && text[i + 1] === '\n') i++
      addCell()
      if (row.length > 1 || row[0] !== '') rows.push(row)
      row = []
      if (rows.length >= maxRows) {
        // Only call it truncated when something other than the final newline remains.
        truncated = text.slice(i + 1).trim().length > 0
        return { rows, truncated }
      }
    } else {
      cell += ch
    }
  }

  if (cell !== '' || row.length > 0) {
    if (rows.length >= maxRows) {
      truncated = true
    } else {
      addCell()
      if (row.length > 1 || row[0] !== '') rows.push(row)
    }
  }
  return { rows, truncated }
}
