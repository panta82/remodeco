# remodeco

#### *Rename Move Delete Copy*

Perform file operations by editing a list of filenames.

So far, only a proof of concept. Doesn't do any file ops, just outputs a list of linux commands that can be executed.

### TODO

- Use glob to load files
- Instead of a random tmp file, use a well defined file based on search criteria
- Pretty output (chalk)
- List all proposed changes (filter out no change lines). Confirm y/n whether to execute the changes. Then actually execute them
- Return to edit mode in case there are errors in the file. Pretty print errors.
- Interactive mode. Confirm each operation (y/n/a)
- Comments switch, append original names as comments (for reference)
- Configuration file, specify preferred editor without changing env
- Only file names
- Relative paths
- Yes mode, no confirmation, rename immediately
- Link operation, create symlinks (what about existing symlinks?)
- Unattended mode, give it a prepared ini file. Presumes Yes mode
- Directory editing
- Figure out a story for CLI editors
- Test on Windows and Mac
