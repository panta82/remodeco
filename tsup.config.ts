import { defineConfig } from 'tsup'

export default defineConfig({
  entry: {
    index: 'src/cli/index.ts',
    'tui/app': 'src/tui/app.tsx',
  },
  format: 'esm',
  outDir: 'dist/cli',
  clean: true,
  sourcemap: true,
  jsx: 'react-jsx',
  target: 'node20',
  platform: 'node',
  banner: {
    js: '#!/usr/bin/env node',
  },
  external: ['ink', 'react', 'koffi', 'chokidar', 'env-paths', 'picocolors', 'picomatch'],
})
