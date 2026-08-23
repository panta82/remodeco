import envPaths from 'env-paths'

export function remodecoPaths(): { config: string; data: string } {
  if (process.env.REMODECO_CONFIG_DIR && process.env.REMODECO_DATA_DIR) {
    return { config: process.env.REMODECO_CONFIG_DIR, data: process.env.REMODECO_DATA_DIR }
  }
  const p = envPaths('remodeco', { suffix: '' })
  return {
    config: process.env.REMODECO_CONFIG_DIR ?? p.config,
    data: process.env.REMODECO_DATA_DIR ?? p.data,
  }
}
