# Rust rewrite architecture

This branch replaces the Node/Ink implementation with one Rust executable while
preserving the `rewrite` branch's Markdown plan and JSON session formats.

## Terminal and diff decision

Remodeco uses Ratatui with Crossterm, the same basic terminal stack used by the
Rust Codex and Grok CLIs. The confirmation view is a **fullscreen** TUI on the
terminal's alternate screen (`smcup` / `rmcup`), the same model as `htop`,
`less`, and GNU `dialog`. While the preview is open it owns the whole visible
window; on exit, panic, or re-opening the editor, it leaves the alternate
screen and restores the previous terminal contents.

The preview is not a source-code unified diff. It compares an original path with
an edited destination. `similar` supplies a Myers sequence diff over path-aware
tokens, rendered as a single inline line (red/strikethrough deletions next to
green insertions). Files are grouped under blue directory headers, with
zero-padded numeric prefixes aligned to the plan's id width. Styling is
restricted to basic colors, bold, dim, and strikethrough, and is disabled by
`NO_COLOR`.

Myers runs when the plan is loaded or reloaded, not on every frame. The TUI
keeps those styled lines and paints only the viewport, so large change sets
stay responsive. Search (`/`, `n`/`N`) and filter (`f`) are view-only; execute
still applies the whole plan. Movement follows pager conventions (`j`/`k`,
`g`/`G`, Home/End, `Ctrl-d`/`u`, arrows, mouse wheel).

References used for the decision:

- [Codex Rust workspace overview](https://github.com/openai/codex/blob/main/codex-rs/README.md)
  identifies Ratatui as its TUI framework.
- [Codex diff renderer](https://github.com/openai/codex/blob/main/codex-rs/tui/src/diff_render.rs)
  uses Ratatui spans and `diffy` for full unified source diffs.
- [Grok Build](https://github.com/xai-org/grok-build) is likewise a Rust
  Ratatui/Crossterm application designed around a full-screen agent UI.
- [Ratatui viewport documentation](https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html)
  defines `Fullscreen` as the default viewport for an application that owns the
  terminal window.

If raw mode or the alternate screen cannot be entered, Remodeco falls back to a
line-oriented prompt with the same execute/editor/cancel flow.

## Modules

- `plan`: generate and strictly parse the line-oriented plan
  (`F0001<tab>:<tab>/path`, `#` comments). Default file is `plan.properties`;
  `--format yaml` / `--format plain-text` write `plan.yaml` / `plan.txt`.
  Draft sessions on another extension are migrated on resume.
- `scan`: deterministic filesystem scan plus source fingerprints.
- `session`: compatible session, manifest, journal, locking, and atomic storage.
- `schedule`: validate operations and order mkdir/stage/commit/copy/trash steps.
- `execute`: journal-before-mutation execution, desktop trash, and undo.
- `editor`: editor discovery and GUI/multiplexer/attached editor lifecycle.
- `tui`: fullscreen alternate-screen shell (event loop, keys, execute/cancel/re-open).
  The virtualized change list lives in `tui/preview.rs`.

## Safety invariants

- Plans are parsed, never evaluated as shell.
- Sources are fingerprinted at scan time and checked immediately before mutation.
  A changed file offers proceed / skip / all / quit. A missing file offers
  skip / all / quit (all skips remaining missing sources). If trash cannot
  be used, the TTY offers permanent delete / skip / all / quit.
- Renames and copies are no-clobber operations.
- Destination parent identity is captured while scheduling and checked while
  executing, reducing the window for symlink or directory-swap races.
- Every step is persisted before and after mutation; interrupted runs remain
  inspectable and resumable through their journal.
- A per-session lock prevents concurrent editing and a global execution lock
  serializes filesystem mutation.
