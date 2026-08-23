# remodeco

Review file renames (or copies) in your editor, preview a live diff in the
terminal, then execute them safely.

Remodeco is a single, fast-starting Rust executable for Linux and macOS.

```sh
cargo install --path .
remodeco /path/to/dir
```

## Flow

1. Remodeco scans the directory into a session under the platform data folder
   (`~/.local/share/remodeco/sessions/` on Linux).
2. It writes a Markdown `plan.md` whose destinations initially equal the source
   paths.
3. It opens the plan in your editor. GUI editors are detached; tmux, Zellij,
   Kitty, and WezTerm can open a sibling pane; terminal editors attach and wait.
4. A small inline terminal view shows the changed paths. It does not use an
   alternate screen, so native scrollback and selection continue to work.
5. Press `e` to execute, `o` to re-open the editor, or `c`/`q` to cancel.

The preview uses `j`/`k`, arrow keys, and Page Up/Page Down when there are more
changes than fit in the inline viewport. Set `NO_COLOR=1` for an unstyled view.

## `plan.md`

Headings are labels only. Each file is represented by one strict, tab-separated
line:

```markdown
- `{01}`	/absolute/path/to/file.txt
```

| Edit | Effect |
| --- | --- |
| Leave the path unchanged | no operation |
| Change the full destination path | rename/move (or copy) |
| Leave the destination empty after the tab | move to desktop trash |
| Delete the line or comment it with `Ctrl+/` | skip |

Do not execute the Markdown as shell. Remodeco parses the file and performs the
operations in-process. Filenames may contain backticks, underscores, spaces,
and `<!--`; NUL, CR, LF, invalid UTF-8, and non-absolute destinations are
rejected.

Per-directory configuration remains compatible with the rewrite branch and
lives at `{dir}/.remodeco.json`. User configuration is `config.json` in the
platform configuration directory.

## Useful commands

```sh
remodeco . --dry-run
remodeco --list-sessions
remodeco --session SESSION_ID
remodeco undo SESSION_ID
```

Run `remodeco --help` for scanning, editor, copy, and session options.

## Safety

Plans are parsed rather than evaluated. File identities are captured during the
scan and checked again immediately before mutation. Renames and copies never
replace an existing destination. Destination parent identities are checked at
execution time, each mutation is journaled, and a global lock serializes
execution. Trash uses the freedesktop.org layout on Linux and `~/.Trash` on
macOS.

The persisted Markdown, manifest, session, and journal formats are compatible
with sessions created by the TypeScript `rewrite` branch.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the terminal/diff decision and module
layout.

MIT. Ivan Pantic.
