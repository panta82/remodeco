# Rust rewrite architecture

This branch replaces the Node/Ink implementation with one Rust executable while
preserving the `rewrite` branch's Markdown plan and JSON session formats.

## Terminal and diff decision

Remodeco uses Ratatui with Crossterm, the same basic terminal stack used by the
Rust Codex and Grok CLIs. Unlike those applications, it deliberately uses
Ratatui's **inline viewport** and never enters the terminal's alternate screen.
The preview therefore remains a small part of an ordinary shell session: native
scrollback, selection, multiplexers, and basic terminals keep working normally.

The preview is not a source-code unified diff. It compares an original path with
an edited destination. `similar` supplies a Myers sequence diff over path-aware
tokens, rendered as compact `- old` / `+ new` rows. Styling is restricted to
basic red, green, bold, and dim attributes and is disabled by `NO_COLOR`.

References used for the decision:

- [Codex Rust workspace overview](https://github.com/openai/codex/blob/main/codex-rs/README.md)
  identifies Ratatui as its TUI framework.
- [Codex diff renderer](https://github.com/openai/codex/blob/main/codex-rs/tui/src/diff_render.rs)
  uses Ratatui spans and `diffy` for full unified source diffs.
- [Grok Build](https://github.com/xai-org/grok-build) is likewise a Rust
  Ratatui/Crossterm application, but is designed around a full-screen agent UI.
- [Ratatui viewport documentation](https://docs.rs/ratatui/latest/ratatui/enum.Viewport.html)
  defines `Inline` specifically for a UI embedded in ordinary CLI output.

If a terminal does not answer the cursor-position query needed by Ratatui's
inline viewport, Remodeco falls back to a line-oriented prompt with the same
execute/editor/cancel flow. It never switches to an alternate screen.

## Modules

- `plan`: generate and strictly parse the line-oriented `plan.md` format.
- `scan`: deterministic filesystem scan plus source fingerprints.
- `session`: compatible session, manifest, journal, locking, and atomic storage.
- `schedule`: validate operations and order mkdir/stage/commit/copy/trash steps.
- `execute`: journal-before-mutation execution, desktop trash, and undo.
- `editor`: editor discovery and GUI/multiplexer/attached editor lifecycle.
- `tui`: inline diff preview and the small execute/cancel/re-open interaction.

## Safety invariants

- Plans are parsed, never evaluated as shell.
- Sources are fingerprinted at scan time and checked immediately before mutation.
- Renames and copies are no-clobber operations.
- Destination parent identity is captured while scheduling and checked while
  executing, reducing the window for symlink or directory-swap races.
- Every step is persisted before and after mutation; interrupted runs remain
  inspectable and resumable through their journal.
- A per-session lock prevents concurrent editing and a global execution lock
  serializes filesystem mutation.
