import { describe, expect, it } from 'vitest'
import { parsePlan, dropCommentLines, BULLET_RE, classifyOps, ParseError } from '../src/plan/parse.ts'
import { generatePlanMd, idWidthForCount } from '../src/plan/generate.ts'
import { isLineSafePath } from '../src/plan/representable.ts'

describe('BULLET_RE', () => {
  it('requires a tab', () => {
    expect(BULLET_RE.test('- `{1}` /a/b')).toBe(false)
    expect(BULLET_RE.test('- `{1}`\t/a/b')).toBe(true)
    expect(BULLET_RE.test('- `{1}`\t')).toBe(true)
  })
})

describe('dropCommentLines', () => {
  it('drops a Ctrl+/ wrapped line', () => {
    expect(dropCommentLines(['keep', '<!-- - `{1}`\t/p -->', 'also'])).toEqual(['keep', 'also'])
  })
  it('does not eat dests that contain <!--', () => {
    expect(dropCommentLines(['- `{1}`\t/foo/<!--bar.mp3'])).toEqual(['- `{1}`\t/foo/<!--bar.mp3'])
  })
  it('fails unterminated block', () => {
    expect(() => dropCommentLines(['<!--', 'still'])).toThrow(ParseError)
  })
})

describe('parsePlan', () => {
  const src = `---
remodeco: 1
id: test
root: '/tmp/r'
---

# /tmp/r

## a

- \`{01}\`\t/tmp/r/a/one.txt
- \`{02}\`\t

<!-- - \`{03}\`\t/tmp/r/a/skip.txt -->
`
  it('parses identity, trash, skip', () => {
    const p = parsePlan(src)
    expect(p.header?.root).toBe('/tmp/r')
    expect(p.bullets.map((b) => b.id)).toEqual([1, 2])
    expect(p.bullets[1].kind).toBe('trash')
    expect(p.unknownLines).toEqual([])
  })
  it('treats missing tab as unknown line', () => {
    const bad = src.replace('`{01}`\t', '`{01}` ')
    const p = parsePlan(bad)
    expect(p.unknownLines.length).toBeGreaterThan(0)
  })
})

describe('classifyOps', () => {
  it('noop vs move vs skip', () => {
    const man = new Map([
      [1, '/tmp/r/a/one.txt'],
      [2, '/tmp/r/a/two.txt'],
      [3, '/tmp/r/a/skip.txt'],
    ])
    const parsed = parsePlan(`---
remodeco: 1
id: t
root: '/tmp/r'
---
- \`{1}\`\t/tmp/r/a/one.txt
- \`{2}\`\t/tmp/r/a/renamed.txt
`)
    const { ops, skipped } = classifyOps(parsed, man, 'move')
    expect(ops[0].kind).toBe('noop')
    expect(ops[1].kind).toBe('move')
    expect(skipped).toContain(3)
  })
})

describe('generatePlanMd', () => {
  it('pads ids and groups headings', () => {
    const md = generatePlanMd({
      sessionId: 's',
      root: '/tmp/r',
      idWidth: idWidthForCount(12),
      rows: [
        { id: 1, from: '/tmp/r/a/one.txt' },
        { id: 2, from: '/tmp/r/b/two.txt' },
      ],
    })
    expect(md).toContain("root: '/tmp/r'")
    expect(md).toContain('- `{01}`\t/tmp/r/a/one.txt')
    expect(md).toContain('## a')
    expect(md).toContain('## b')
  })
})

describe('line-safe paths', () => {
  it('allows backticks, comments, trailing space; rejects NUL/CR/LF', () => {
    expect(isLineSafePath("/tmp/You`ve.mp3")).toBe(true)
    expect(isLineSafePath('/tmp/a<!--b')).toBe(true)
    expect(isLineSafePath('/tmp/a ')).toBe(true)
    expect(isLineSafePath('/tmp/a')).toBe(true)
    expect(isLineSafePath('/tmp/a\n')).toBe(false)
  })
})

describe('backtick dest', () => {
  it('round-trips a dest with a grave accent', () => {
    const from = "/tmp/r/You`ve Made Me So Very Happy.mp3"
    const md = generatePlanMd({
      sessionId: 's',
      root: '/tmp/r',
      idWidth: 1,
      rows: [{ id: 1, from }],
    })
    const p = parsePlan(md)
    expect(p.bullets[0].destRaw).toBe(from)
  })
})
