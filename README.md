# remodeco

Review file renames (or copies) in your editor, preview a live diff in the terminal, then execute.

Linux and macOS. Node 20+.

```
npm i -g remodeco
remodeco /path/to/dir
```

Or `npx remodeco /path/to/dir`.

## Flow

1. Scans the directory into a session under `~/.local/share/remodeco/sessions/`.
2. Writes `plan.md` (identity mapping: dest = original path).
3. Opens the plan in `$EDITOR` (GUI detached, tmux/Kitty/WezTerm/Zellij sibling pane, else attach-and-wait).
4. Ink TUI: `[e]` Execute, `[c]` Cancel, `[o]` Re-open editor.

`--no-edit` skips the editor. `--dry-run` prints the schedule and does not mutate. `--copy` copies instead of moving.

## plan.md

Headings are labels only. Each file is:

```markdown
- `{01}`	/abs/path/to/file.txt
```

| Edit | Effect |
| --- | --- |
| Leave the path | no-op |
| Change the full dest path | rename/move (or copy) |
| Tab then empty dest | trash (XDG Trash / `~/.Trash`) |
| Delete the line or `Ctrl+/` comment it | skip |

Do not `bash` this file. remodeco parses it and executes in-process.

Per-directory config: `{dir}/.remodeco.json`.

## Safety

Parse-and-execute, never a shell. No-clobber rename (`renameat2` / `renamex_np`). Journal before mutation. Global execute lock.

MIT. Ivan Pantic.
