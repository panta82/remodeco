import { execSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
execSync('npm pack', { cwd: root, stdio: 'inherit' })
const tgz = fs.readdirSync(root).find((n) => n.endsWith('.tgz'))
if (!tgz) throw new Error('no tarball')
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'remodeco-pack-'))
execSync(`tar -xzf ${path.join(root, tgz)} -C ${dir}`)
const pkg = path.join(dir, 'package')
const bin = path.join(pkg, 'dist/cli/index.js')
if (!fs.existsSync(bin)) throw new Error('missing dist/cli/index.js')
const out = execSync(`node ${bin} --help`, { encoding: 'utf8' })
if (!out.includes('Usage: remodeco')) throw new Error('help output unexpected')
console.log('pack smoke ok', tgz)
