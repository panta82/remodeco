# Architecture

Remodeco is a Rust executable for Linux and macOS. It scans files into an
editable plan, previews the requested changes, then executes them through a
persistent journal. See [README.md](README.md) for usage and plan editing rules.

## Session flow and storage

`main` handles CLI dispatch; `controller` coordinates session creation, plan
validation, execution, and undo. A new session records the original paths and
metadata fingerprints in a manifest. Opening an existing session takes its lock
before loading it.

The normal flow opens the plan in an editor, then enters the confirmation TUI.
The TUI polls for saved edits and reloads the preview. Confirmation captures the
session revision, mode, and plan hash; execution rejects changes made after
confirmation. `--dry-run` prints the validated schedule after the editor step.
The `execute` subcommand executes a saved plan or resumes an interrupted
execution without opening the editor or preview.

Sessions live under `sessions/<id>/` in the platform data directory, which
`REMODECO_DATA_DIR` can override. Each contains `session.json`, `manifest.json`,
an editable plan, a `session.lock`, and journal files. `model` defines the
serialized records; `store`, `atomic`, and `lock` handle persistence and locking.

The default plan is `plan.properties`, with entries written as
`F<id><tab>:<tab>/destination/path` and directory labels written as `#` comments.
Three metadata lines prefixed with `#> ` identify the format version, session,
and root. The parser reads these before ordinary comments. Root paths are stored
verbatim without quotes. The
`--format yaml` and `--format plain-text` options change the filename to
`plan.yaml` or `plan.txt`; all three use the same line-oriented content and parser.


## Terminal preview

The confirmation view uses Ratatui with Crossterm on the terminal's alternate
screen. It owns the visible window while open and restores the terminal on
normal exit, panic, or re-opening the editor. If terminal setup fails, it falls
back to a line-oriented execute/editor/reset/cancel prompt.

`diff` uses `similar` to compute a Myers sequence diff over path-aware tokens.
The preview renders deletions in red with strikethrough beside green insertions,
with files grouped under directory headers and numeric prefixes aligned to the
plan's ID width. `NO_COLOR` disables colors; text modifiers remain.

`tui/preview` caches styled lines when the plan loads or changes, and renders
only the viewport. Search and filter affect the view; execution still applies
the whole plan. The view supports vertical and horizontal scrolling. Reset
regenerates the original plan from the manifest and is limited to draft sessions
without an active journal.

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `main`, `cli`, `config` | Command dispatch, arguments, configuration, and platform paths. |
| `controller` | Session lifecycle, confirmation checks, scheduling, execution, and inverse journals for undo. |
| `model` | Session, manifest, operation, schedule, and journal types. |
| `scan` | Deterministic filesystem scan, exclusions, and metadata fingerprints. |
| `plan` | Plan generation, parsing, header validation, hashes, and operation classification. |
| `store`, `atomic`, `lock` | Session files, atomic writes, and advisory locks. |
| `schedule` | Destination validation and ordering of trash, mkdir, stage, commit, and copy steps. |
| `execute` | Journal execution, interrupted-step reconciliation, source checks, and empty source directory cleanup. |
| `native`, `trash` | Platform filesystem operations and desktop trash handling. |
| `editor` | Editor discovery and GUI, multiplexer, or attached editor lifecycle. |
| `diff`, `tui`, `tui/preview` | Path differences, terminal lifecycle, input, and preview rendering. |
| `util` | Shared time and path helpers. |

## Execution safeguards and limits

- Plans are parsed, never evaluated as shell. IDs refer back to the manifest;
  editing a destination does not change the recorded source.
- Source fingerprints use filesystem metadata, not file content hashes. The
  executor checks them before mutation. A changed source offers proceed / skip /
  all / quit; a missing source offers skip / all / quit. For missing sources,
  all skips the remaining missing sources.
- Scheduling rejects duplicate destinations and conflicting existing paths.
  Move cycles use temporary sibling paths. Native renames and copies refuse to
  overwrite an existing destination. Cross-filesystem renames fail; there is no
  copy-and-delete fallback for moves.
- Destination parent identity is captured while scheduling and checked while
  executing, reducing the window for symlink or directory-swap races.
- The executor persists each scheduled step before and after mutation. It
  reconciles in-progress steps when resuming an interrupted execution, and
  requires the plan, revision, and mode to match the journal.
- A per-session advisory lock prevents concurrent Remodeco editing sessions.
  A shared execution lock in the data directory serializes execution and undo
  for sessions using that directory.
- Trash is the default for empty destinations. If no safe trash location is
  available, the TTY offers permanent delete / skip / all / quit. Permanent
  deletion has no undo data.
- After moves or trash operations, empty source directories under the session
  root are removed with `rmdir`, never recursively. The session root is kept.
- Undo builds an inverse journal from a completed execution. It reverses moves,
  trashes copies, restores items with trash restore records, and recreates
  missing parents. Resuming an interrupted undo is not supported.
