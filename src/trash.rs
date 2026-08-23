use crate::model::TrashKey;
use crate::native::rename_no_replace;
use anyhow::{Context, Result, bail};
use chrono::Local;
use directories::BaseDirs;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const PATH_ENCODE: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}')
    .add(b'~');

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrashKind {
    Home,
    Top,
    Macos,
}

#[derive(Clone, Debug)]
pub struct TrashLocation {
    pub root: PathBuf,
    pub kind: TrashKind,
}

pub fn locate_trash(source: &Path) -> Result<TrashLocation> {
    let base = BaseDirs::new().context("cannot determine home directory")?;
    #[cfg(target_os = "macos")]
    {
        let root = base.home_dir().join(".Trash");
        fs::create_dir_all(&root)?;
        return Ok(TrashLocation {
            root,
            kind: TrashKind::Macos,
        });
    }
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::geteuid() };
        let xdg = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.home_dir().join(".local/share"));
        create_private_path(&xdg)?;
        let source_dev = fs::symlink_metadata(source)?.dev();
        if fs::symlink_metadata(&xdg)?.dev() == source_dev {
            let root = xdg.join("Trash");
            ensure_user_trash(&root, uid)?;
            return Ok(TrashLocation {
                root,
                kind: TrashKind::Home,
            });
        }
        let mount = mount_point(source)?;
        let shared = mount.join(".Trash");
        if valid_shared_trash(&shared) {
            let root = shared.join(uid.to_string());
            ensure_user_trash(&root, uid)?;
            return Ok(TrashLocation {
                root,
                kind: TrashKind::Top,
            });
        }
        let root = mount.join(format!(".Trash-{uid}"));
        ensure_user_trash(&root, uid)?;
        Ok(TrashLocation {
            root,
            kind: TrashKind::Top,
        })
    }
}

fn create_private_path(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        missing.push(current.to_path_buf());
        current = current
            .parent()
            .context("trash path has no existing ancestor")?;
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn safe_user_directory(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
            && metadata.mode() & 0o777 == 0o700
    })
}

fn ensure_user_trash(root: &Path, uid: u32) -> Result<()> {
    for directory in [root.to_path_buf(), root.join("files"), root.join("info")] {
        if !directory.exists() {
            fs::create_dir(&directory)?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        if !safe_user_directory(&directory, uid) {
            bail!("unsafe trash directory {}", directory.display());
        }
    }
    Ok(())
}

fn valid_shared_trash(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.mode() & 0o1000 != 0
    })
}

fn mount_point(source: &Path) -> Result<PathBuf> {
    let source_dev = fs::symlink_metadata(source)?.dev();
    let mut last = source.to_path_buf();
    let mut current = source.parent().unwrap_or(Path::new("/")).to_path_buf();
    loop {
        if fs::symlink_metadata(&current)?.dev() != source_dev {
            return Ok(last);
        }
        last = current.clone();
        let Some(parent) = current.parent() else {
            return Ok(last);
        };
        if parent == current {
            return Ok(last);
        }
        current = parent.to_path_buf();
    }
}

pub fn unique_trash_key(location: &TrashLocation, source: &Path) -> Result<TrashKey> {
    let base = source
        .file_name()
        .and_then(|v| v.to_str())
        .context("trash source has invalid filename")?;
    let files_dir = if location.kind == TrashKind::Macos {
        location.root.clone()
    } else {
        location.root.join("files")
    };
    let info_dir = (location.kind != TrashKind::Macos).then(|| location.root.join("info"));
    for number in 0_u64.. {
        let name = if number == 0 {
            base.to_owned()
        } else {
            format!("{base}.{number}")
        };
        let files_path = files_dir.join(&name);
        let info_path = info_dir
            .as_ref()
            .map(|directory| directory.join(format!("{name}.trashinfo")));
        if fs::symlink_metadata(&files_path).is_err()
            && info_path
                .as_ref()
                .is_none_or(|path| fs::symlink_metadata(path).is_err())
        {
            return Ok(TrashKey {
                files_path: path_text(&files_path)?,
                info_path: info_path.as_deref().map(path_text).transpose()?,
                original_path: path_text(source)?,
            });
        }
    }
    unreachable!()
}

pub fn perform_trash(
    source: &Path,
    key: &TrashKey,
    journal_id: &str,
    location: &TrashLocation,
) -> Result<()> {
    if let Some(info_path) = &key.info_path {
        let original = path_text(source)?;
        let path_field = if location.kind == TrashKind::Top {
            let mount = mount_point(source)?;
            source
                .strip_prefix(&mount)
                .unwrap_or(source)
                .to_str()
                .context("invalid trash path")?
                .trim_start_matches('/')
                .to_owned()
        } else {
            original
        };
        let body = format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\nX-Remodeco-Journal={}\n",
            utf8_percent_encode(&path_field, PATH_ENCODE),
            Local::now().format("%Y-%m-%dT%H:%M:%S"),
            journal_id,
        );
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(info_path)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(error) = rename_no_replace(source, Path::new(&key.files_path)) {
        if let Some(info_path) = &key.info_path {
            let _ = fs::remove_file(info_path);
        }
        return Err(error);
    }
    Ok(())
}

pub fn restore_trash(key: &TrashKey) -> Result<()> {
    rename_no_replace(Path::new(&key.files_path), Path::new(&key.original_path))?;
    if let Some(info_path) = &key.info_path {
        match fs::remove_file(info_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("remove {info_path}")),
        }
    }
    Ok(())
}

fn path_text(path: &Path) -> Result<String> {
    Ok(path.to_str().context("path is not UTF-8")?.to_owned())
}
