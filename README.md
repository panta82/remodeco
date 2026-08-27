# remodeco

**RE**move  
**MO**dify  
**DE**lete  
**CO**py  

Here's how it works:

1️⃣ Run `remodeco ~/Pictures`

2️⃣ A `.properties` plan file opens in your editor:

```properties
F01	:	/home/joesmith/Pictures/mom.jpg
F02	:	/home/joesmith/Pictures/dad.jpg
F03	:	/home/joesmith/Pictures/sis.jpg
``` 

3️⃣ Edit the file

![docs_editor.png](misc/docs_editor.png)

4️⃣ Save and exit (if editor is TUI), or tab switch back into remodeco (if editor is GUI).

5️⃣ Review the changeses

![docs_diff.png](misc/docs_diff.png)

6️⃣ Review in the fullscreen preview: `e` execute, `o` re-open the editor, `r` reset, `c`/`q` cancel. `/` search, `n`/`N` next/previous match, `f` filter. `j`/`k` or arrows scroll, `g`/`G` or Home/End jump, `Ctrl-d`/`u` half-page, `?` lists keys. Filter and search only change what you see — execute still applies the whole plan.

If you accept the changes, your files will be renamed, copied, or moved to trash.

### Plan file format

Each file is one line, `F<id>` then a tab, `:`, a tab, then the destination
path verbatim. Tabs are separators only — they are not allowed in paths.
Headings are `#` comments and are labels only.

```properties
F01	:	/absolute/path/to/file.txt
```

| Edit | Effect |
| --- | --- |
| Leave the path unchanged | no operation |
| Change the full destination path | rename/move (or copy) |
| Leave the destination empty after the colon | move to desktop trash or delete |
| Delete the line or comment it with `Ctrl+/` | skip |

Default file is `plan.properties`. `--format yaml` writes `plan.yaml`,
`--format plain-text` writes `plan.txt` (no language highlighting). Same
choice in config as `"format": "properties"` / `"yaml"` / `"plain-text"`.
Draft sessions on `plan.md` or another extension are rewritten and moved on
resume; edits are kept.

### Configuration

- Per-directory configuration: `{dir}/.remodeco.json`
- User configuration: `config.json` in the platform configuration directory

### Useful commands

```sh
remodeco . --dry-run
remodeco --list-sessions
remodeco --session SESSION_ID
remodeco undo SESSION_ID
```

Run `remodeco --help` for scanning, editor, copy, and session options.

### License

MIT. Ivan Pantic.
