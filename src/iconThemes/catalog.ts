/**
 * Icon semantics shared by every icon theme.
 *
 * Each theme ships one SVG per key under `src/assets/icon-themes/<theme>/`, named
 * exactly after the key. Adding a file type or folder name here makes it light up in
 * every theme that provides the matching key; a missing key falls back to `file` or
 * `folder`, so themes can stay incomplete without breaking rows.
 */

export type IconThemeId = 'system' | 'flat' | 'material' | 'catppuccin'

export type FileIconKey =
  | 'file'
  | 'document'
  | 'log'
  | 'readme'
  | 'license'
  | 'config'
  | 'tune'
  | 'npm'
  | 'lock'
  | 'makefile'
  | 'git'
  | 'docker'
  | 'database'
  | 'json'
  | 'yaml'
  | 'toml'
  | 'xml'
  | 'markdown'
  | 'html'
  | 'css'
  | 'sass'
  | 'less'
  | 'javascript'
  | 'react'
  | 'typescript'
  | 'react-ts'
  | 'vue'
  | 'svelte'
  | 'astro'
  | 'angular'
  | 'python'
  | 'rust'
  | 'go'
  | 'java'
  | 'c'
  | 'cpp'
  | 'csharp'
  | 'php'
  | 'ruby'
  | 'swift'
  | 'kotlin'
  | 'dart'
  | 'lua'
  | 'shell'
  | 'powershell'
  | 'graphql'
  | 'proto'
  | 'terraform'
  | 'nginx'
  | 'blender'
  | 'image'
  | 'svg'
  | 'video'
  | 'audio'
  | 'pdf'
  | 'word'
  | 'excel'
  | 'powerpoint'
  | 'archive'
  | 'disc'
  | 'android'
  | 'ebook'
  | 'font'
  | 'exe'
  | 'dll'

export type FolderIconKey =
  | 'folder'
  | 'folder-git'
  | 'folder-github'
  | 'folder-node'
  | 'folder-src'
  | 'folder-dist'
  | 'folder-build'
  | 'folder-public'
  | 'folder-assets'
  | 'folder-images'
  | 'folder-docs'
  | 'folder-test'
  | 'folder-config'
  | 'folder-package'
  | 'folder-vscode'
  | 'folder-database'
  | 'folder-target'
  | 'folder-vendor'
  | 'folder-temp'
  | 'folder-music'
  | 'folder-pictures'
  | 'folder-videos'
  | 'folder-downloads'
  | 'folder-documents'
  | 'folder-desktop'
  | 'folder-home'
  | 'folder-lib'
  | 'folder-android'
  | 'folder-ios'
  | 'folder-docker'
  | 'folder-examples'
  | 'folder-scripts'
  | 'folder-styles'
  | 'folder-utils'
  | 'folder-locales'
  | 'folder-tauri'

export type IconKey = FileIconKey | FolderIconKey

export type IconAssets = Partial<Record<IconKey, string>>

const EXTENSIONS: Record<string, FileIconKey> = {
  // Images
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image', bmp: 'image', webp: 'image',
  ico: 'image', tif: 'image', tiff: 'image', heic: 'image', avif: 'image', jxl: 'image',
  svg: 'svg',
  // Video / audio
  mp4: 'video', mkv: 'video', mov: 'video', avi: 'video', webm: 'video', flv: 'video',
  wmv: 'video', m4v: 'video', mpg: 'video', mpeg: 'video', ogv: 'video',
  mp3: 'audio', flac: 'audio', wav: 'audio', m4a: 'audio', aac: 'audio', ogg: 'audio',
  opus: 'audio', wma: 'audio', mid: 'audio', midi: 'audio',
  // Documents
  pdf: 'pdf',
  doc: 'word', docx: 'word', odt: 'word', rtf: 'word',
  xls: 'excel', xlsx: 'excel', ods: 'excel', csv: 'excel', tsv: 'excel',
  ppt: 'powerpoint', pptx: 'powerpoint', odp: 'powerpoint',
  txt: 'document', log: 'log',
  md: 'markdown', markdown: 'markdown', mdx: 'markdown',
  epub: 'ebook', mobi: 'ebook', azw3: 'ebook',
  // Archives / disks / binaries
  zip: 'archive', rar: 'archive', '7z': 'archive', tar: 'archive', gz: 'archive',
  tgz: 'archive', xz: 'archive', zst: 'archive', bz2: 'archive', deb: 'archive',
  rpm: 'archive', jar: 'archive', war: 'archive',
  iso: 'disc', dmg: 'disc', vhd: 'disc', vmdk: 'disc',
  exe: 'exe', msi: 'exe', com: 'exe',
  dll: 'dll', so: 'dll', dylib: 'dll', sys: 'dll', ocx: 'dll',
  apk: 'android', aab: 'android',
  // Code
  js: 'javascript', mjs: 'javascript', cjs: 'javascript',
  jsx: 'react',
  ts: 'typescript', mts: 'typescript', cts: 'typescript',
  tsx: 'react-ts',
  vue: 'vue', svelte: 'svelte', astro: 'astro',
  py: 'python', pyw: 'python', pyc: 'python', pyo: 'python', ipynb: 'python',
  rs: 'rust', go: 'go', java: 'java', class: 'java',
  c: 'c', h: 'c',
  cpp: 'cpp', cc: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp', hxx: 'cpp',
  cs: 'csharp', php: 'php', phtml: 'php', rb: 'ruby', erb: 'ruby',
  swift: 'swift', kt: 'kotlin', kts: 'kotlin', dart: 'dart', lua: 'lua',
  sh: 'shell', bash: 'shell', zsh: 'shell', fish: 'shell', bat: 'shell', cmd: 'shell',
  ps1: 'powershell', psm1: 'powershell', psd1: 'powershell',
  html: 'html', htm: 'html', xhtml: 'html',
  css: 'css', scss: 'sass', sass: 'sass', less: 'less',
  json: 'json', jsonc: 'json',
  yaml: 'yaml', yml: 'yaml', toml: 'toml',
  xml: 'xml', xsd: 'xml', xsl: 'xml', xslt: 'xml', plist: 'xml',
  graphql: 'graphql', gql: 'graphql', proto: 'proto',
  tf: 'terraform', tfvars: 'terraform', hcl: 'terraform',
  ini: 'tune', cfg: 'tune', conf: 'tune', env: 'tune', properties: 'tune',
  sql: 'database', db: 'database', sqlite: 'database', sqlite3: 'database',
  db3: 'database', mdb: 'database', accdb: 'database', dbf: 'database',
  ttf: 'font', otf: 'font', woff: 'font', woff2: 'font', eot: 'font',
  blend: 'blender',
  lock: 'lock',
}

/** Whole-name rules run before the extension table; keys are lower-case. */
const FILE_NAMES: Record<string, FileIconKey> = {
  dockerfile: 'docker',
  containerfile: 'docker',
  makefile: 'makefile',
  gnumakefile: 'makefile',
  justfile: 'makefile',
  'cmakelists.txt': 'cpp',
  'package.json': 'npm',
  'package-lock.json': 'npm',
  'npm-shrinkwrap.json': 'npm',
  '.npmrc': 'npm',
  '.yarnrc': 'npm',
  '.yarnrc.yml': 'npm',
  'pnpm-lock.yaml': 'lock',
  'pnpm-workspace.yaml': 'lock',
  'yarn.lock': 'lock',
  'bun.lock': 'lock',
  'bun.lockb': 'lock',
  'cargo.toml': 'rust',
  'cargo.lock': 'rust',
  'rust-toolchain.toml': 'rust',
  'go.mod': 'go',
  'go.sum': 'go',
  'gemfile': 'ruby',
  'gemfile.lock': 'ruby',
  'pom.xml': 'java',
  'build.gradle': 'java',
  'settings.gradle': 'java',
  'build.gradle.kts': 'kotlin',
  'settings.gradle.kts': 'kotlin',
  'requirements.txt': 'python',
  'pyproject.toml': 'python',
  'setup.py': 'python',
  'setup.cfg': 'python',
  'pipfile': 'python',
  'pipfile.lock': 'python',
  'poetry.lock': 'lock',
  'tsconfig.json': 'typescript',
  'jsconfig.json': 'javascript',
  'nginx.conf': 'nginx',
  '.editorconfig': 'config',
  '.prettierrc': 'config',
  '.eslintrc': 'config',
  '.babelrc': 'config',
  'biome.json': 'config',
  'tauri.conf.json': 'config',
  '.env': 'tune',
  '.env.local': 'tune',
  '.env.development': 'tune',
  '.env.production': 'tune',
  '.gitignore': 'git',
  '.gitattributes': 'git',
  '.gitmodules': 'git',
  '.gitconfig': 'git',
  '.gitkeep': 'git',
  'changelog.md': 'document',
  'changelog': 'document',
}

const FILE_PREFIXES: [string, FileIconKey][] = [
  ['readme', 'readme'],
  ['license', 'license'],
  ['licence', 'license'],
  ['copying', 'license'],
  ['.git', 'git'],
  ['docker-compose', 'docker'],
  ['.env.', 'tune'],
  ['tsconfig.', 'typescript'],
  ['jsconfig.', 'javascript'],
  ['vite.config.', 'config'],
  ['webpack.config.', 'config'],
  ['rollup.config.', 'config'],
  ['esbuild.config.', 'config'],
  ['postcss.config.', 'config'],
  ['tailwind.config.', 'config'],
  ['vitest.config.', 'config'],
  ['playwright.config.', 'config'],
  ['.eslintrc.', 'config'],
  ['.prettierrc.', 'config'],
  ['tauri.', 'config'],
]

const FOLDER_NAMES: Record<string, FolderIconKey> = {
  '.git': 'folder-git',
  '.github': 'folder-github',
  node_modules: 'folder-node',
  src: 'folder-src',
  source: 'folder-src',
  dist: 'folder-dist',
  out: 'folder-dist',
  output: 'folder-dist',
  build: 'folder-build',
  public: 'folder-public',
  static: 'folder-public',
  assets: 'folder-assets',
  resources: 'folder-assets',
  res: 'folder-assets',
  images: 'folder-images',
  img: 'folder-images',
  icons: 'folder-images',
  docs: 'folder-docs',
  documentation: 'folder-docs',
  test: 'folder-test',
  tests: 'folder-test',
  __tests__: 'folder-test',
  spec: 'folder-test',
  specs: 'folder-test',
  config: 'folder-config',
  configs: 'folder-config',
  '.config': 'folder-config',
  conf: 'folder-config',
  packages: 'folder-package',
  package: 'folder-package',
  '.vscode': 'folder-vscode',
  database: 'folder-database',
  db: 'folder-database',
  target: 'folder-target',
  vendor: 'folder-vendor',
  temp: 'folder-temp',
  tmp: 'folder-temp',
  cache: 'folder-temp',
  music: 'folder-music',
  pictures: 'folder-pictures',
  photos: 'folder-pictures',
  videos: 'folder-videos',
  video: 'folder-videos',
  downloads: 'folder-downloads',
  documents: 'folder-documents',
  desktop: 'folder-desktop',
  home: 'folder-home',
  lib: 'folder-lib',
  libs: 'folder-lib',
  library: 'folder-lib',
  android: 'folder-android',
  ios: 'folder-ios',
  docker: 'folder-docker',
  examples: 'folder-examples',
  example: 'folder-examples',
  samples: 'folder-examples',
  scripts: 'folder-scripts',
  styles: 'folder-styles',
  style: 'folder-styles',
  css: 'folder-styles',
  utils: 'folder-utils',
  util: 'folder-utils',
  helpers: 'folder-utils',
  locales: 'folder-locales',
  locale: 'folder-locales',
  i18n: 'folder-locales',
  lang: 'folder-locales',
  tauri: 'folder-tauri',
  // Localized Windows known folders, so a Chinese profile keeps the same icons.
  桌面: 'folder-desktop',
  下载: 'folder-downloads',
  文档: 'folder-documents',
  图片: 'folder-pictures',
  音乐: 'folder-music',
  视频: 'folder-videos',
  主目录: 'folder-home',
}

function lowerExtension(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? name.slice(dot + 1) : ''
}

/** Picks the semantic icon key for a file row. */
export function fileIconKey(name: string): FileIconKey {
  const lower = name.toLowerCase()
  const exact = FILE_NAMES[lower]
  if (exact) return exact
  for (const [prefix, key] of FILE_PREFIXES) {
    if (lower.startsWith(prefix)) return key
  }
  return EXTENSIONS[lowerExtension(lower)] ?? 'file'
}

/** Picks the semantic icon key for a directory row; unknown names share the plain folder. */
export function folderIconKey(name: string): FolderIconKey {
  return FOLDER_NAMES[name.toLowerCase()] ?? 'folder'
}