import type { IconAssets } from './catalog'

/** Turns Vite's glob result (`{ './material/a.svg': url }`) into `{ a: url }`. */
export function keyedAssets(files: Record<string, string>): IconAssets {
  const assets: Record<string, string> = {}
  for (const [path, url] of Object.entries(files)) {
    const name = path.slice(path.lastIndexOf('/') + 1, -'.svg'.length)
    assets[name] = url
  }
  return assets
}