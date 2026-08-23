export function yamlSingleQuote(s: string): string {
  return "'" + s.replace(/'/g, "''") + "'"
}

export function parseYamlSingleQuoted(s: string): string {
  s = s.trim()
  if (s.startsWith("'") && s.endsWith("'") && s.length >= 2) {
    return s.slice(1, -1).replace(/''/g, "'")
  }
  return s
}

export function emitFrontMatter(id: string, root: string): string {
  return `---
remodeco: 1
id: ${id}
root: ${yamlSingleQuote(root)}
---
`
}
