/**
 * Scroll offset that centres a row inside its scroll port.
 *
 * All rectangles are viewport coordinates except `scrollTop`, which is the port's
 * current offset. The result is clamped at 0; the browser clamps the far end.
 */
export function centerOffset(
  rowTop: number,
  rowHeight: number,
  boxTop: number,
  boxHeight: number,
  scrollTop: number,
): number {
  return Math.max(0, rowTop - boxTop + scrollTop - (boxHeight - rowHeight) / 2)
}
