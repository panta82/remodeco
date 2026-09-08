# remodeco

🆁🅴🄼🄾🅅🄴  
🅼🅾🄳🄸🄵🅈  
🅳🅴🄻🄴🅃🄴  
🅲🅾🄿🅈  

...your files in a text editor.

### Here's how it works:

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

5️⃣ Review the changes

![docs_diff.png](misc/docs_diff.png)

6️⃣ If satisfied with the plan, press <kbd>e</kbd> to execute. Otherwise, you can exit and come back later, or start fresh (<kbd>r</kbd>).

### Plan file format

Each file is one line, `F<id>` then a tab, `:`, a tab, then the destination
path verbatim. Tabs are separators only — they are not allowed in paths.
Headings are `#` comments and are labels only.

```properties
F01	:	/absolute/path/to/file.txt
```

| Edit                                        | Effect                          |
|---------------------------------------------|---------------------------------|
| Leave the path unchanged                    | no operation                    |
| Change the full destination path            | rename/move (or copy)           |
| Leave the destination empty after the colon | move to desktop trash or delete |
| Delete the line or comment it with `Ctrl+/` | skip                            |

Default file is `plan.properties`. `--format yaml` writes `plan.yaml`,
`--format plain-text` writes `plan.txt` (no language highlighting). Same
choice in config as `"format": "properties"` / `"yaml"` / `"plain-text"`.
The first three lines start with `#>` and record the format version, session ID,
and root path.
Keep them intact; they bind the plan to its session.

### Builds and releases

Builds are available under [Releases](https://github.com/panta82/remodeco/releases).

| System              | Architectures        | Packages                          |
|---------------------|----------------------|-----------------------------------|
| Linux               | x86-64, ARM64        | `.deb`, `.rpm`, `.tar.gz`         |
| macOS               | Intel, Apple Silicon | `.pkg`, `.tar.gz`                 |
| Windows through WSL | x86-64, ARM64        | Use the Linux packages inside WSL |

### Configuration

- Per-directory configuration: `{dir}/.remodeco.json`
- User configuration: `config.json` in the platform configuration directory

### Useful commands

```sh
remodeco . --dry-run
remodeco list
remodeco --session SESSION_ID
remodeco execute SESSION_ID
remodeco undo SESSION_ID
remodeco delete SESSION_ID
```

Run `remodeco --help` for scanning, editor, copy, and session options.

### Version history

**1.0.0** -
*2026-09-08*:

Project revived, rewritten in Rust, and released with a new plan format.

### License

[MIT](LICENSE)
