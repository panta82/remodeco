import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { describe, expect, it } from 'vitest'
import { scanTree } from '../src/scan/walk.ts'

describe('scanTree', () => {
  it('lists files, skips node_modules and hidden', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remodeco-scan-'))
    fs.writeFileSync(path.join(dir, 'a.txt'), 'a')
    fs.mkdirSync(path.join(dir, 'sub'))
    fs.writeFileSync(path.join(dir, 'sub', 'b.txt'), 'b')
    fs.mkdirSync(path.join(dir, 'node_modules'))
    fs.writeFileSync(path.join(dir, 'node_modules', 'x.js'), 'x')
    fs.writeFileSync(path.join(dir, '.secret'), 's')
    const r = scanTree({ root: dir, recursive: true, hidden: false, exclude: ['.git', 'node_modules', '.hg', '.svn'] })
    expect(r.rows.map((x) => path.basename(x.from)).sort()).toEqual(['a.txt', 'b.txt'])
  })
})
