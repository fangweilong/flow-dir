/**
 * Explorer-style type-ahead. A name that starts with the buffer beats a name that merely
 * contains the buffer's characters in order, and either way the first row in list order
 * wins because the list is already sorted the way the user sees it.
 *
 * Case-insensitive throughout, and stable for a `null` buffer (no match).
 */
export function matchTypeAhead(names: readonly string[], buffer: string): number {
  const needle = buffer.trim().toLowerCase()
  if (!needle) return -1
  let subsequence = -1
  for (let i = 0; i < names.length; i++) {
    const name = names[i].toLowerCase()
    if (name.startsWith(needle)) return i
    if (subsequence < 0 && isSubsequence(needle, name)) subsequence = i
  }
  return subsequence
}

/** Left-to-right subsequence, not a contiguous match ("rpt" matches "report"). */
function isSubsequence(needle: string, name: string): boolean {
  let at = 0
  for (let i = 0; i < name.length && at < needle.length; i++) {
    if (name[i] === needle[at]) at++
  }
  return at === needle.length
}
