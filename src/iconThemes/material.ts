import { keyedAssets } from './bundled'

const files = import.meta.glob('../assets/icon-themes/material/*.svg', {
  eager: true,
  query: '?url',
  import: 'default',
}) as Record<string, string>

export default keyedAssets(files)