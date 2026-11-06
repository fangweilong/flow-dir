/** Stable `code` field of an API error, or the value itself when it is not one. */
export const errorCode = (err: unknown): string =>
  err && typeof err === 'object' && 'code' in err && typeof err.code === 'string'
    ? err.code
    : String(err)
