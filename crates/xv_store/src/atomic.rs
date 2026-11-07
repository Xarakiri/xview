use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// `$XVIEW_DATA_DIR` if set, else `<platform data dir>/XView`; created if missing.
pub fn data_dir() -> PathBuf {
    let dir = match std::env::var_os("XVIEW_DATA_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("XView"),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes via a temp file in the same directory, `sync_all`, then `rename`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy();
    let tmp = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        if let Ok(d) = std::fs::File::open(parent) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Errors with `InvalidData` if `path` exists but is unreadable or fails
/// `valid`, so a save never replaces a file that has no backup.
pub(crate) fn refuse_if_unparseable(
    path: &Path,
    valid: impl Fn(&str) -> bool,
) -> std::io::Result<()> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(b) if std::str::from_utf8(&b).is_ok_and(&valid) => Ok(()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "refusing to overwrite unreadable/corrupt file {}",
                path.display()
            ),
        )),
    }
}

/// Renames `path` to `<name>.corrupt-<unix>` (suffixed `-1`, `-2`... if taken)
/// so an earlier backup is never overwritten. Returns the new path.
pub(crate) fn move_aside(path: &Path) -> std::io::Result<PathBuf> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy()
        .into_owned();
    let mut n = 0u32;
    loop {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let target = path.with_file_name(format!("{name}.corrupt-{unix}{suffix}"));
        if !target.exists() {
            std::fs::rename(path, &target)?;
            return Ok(target);
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let t = tempfile::TempDir::new().unwrap();
        let p = t.path().join("sub/dir/f.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("f.json")]);
    }
}
