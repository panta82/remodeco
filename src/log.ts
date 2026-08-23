let verbose = false

export function setVerbose(v: boolean): void {
  verbose = v
}

export function logVerbose(...args: unknown[]): void {
  if (verbose || process.env.DEBUG === 'remodeco') {
    console.error(...args)
  }
}
