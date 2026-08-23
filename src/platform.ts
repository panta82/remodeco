const ALLOWED = new Set(['linux', 'darwin'])

export function isSupportedPlatform(platform = process.platform): boolean {
  return ALLOWED.has(platform)
}

/** Exit 2 before any scan or persistent write. --help/--version skip this. */
export function requireSupportedPlatform(): void {
  if (isSupportedPlatform()) return
  console.error('remodeco v1 is Linux and macOS only')
  process.exit(2)
}
