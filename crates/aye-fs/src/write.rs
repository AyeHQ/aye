//! Replacing a file whole.

use std::fs::{self, File};
use std::io;
use std::path::Path;

/// Writes `dir/name` whole or not at all: `write` fills a temporary file in
/// `dir` (`<prefix>….tmp`), which is then renamed over `name`, replacing it
/// on Windows too. A reader sees the old file or the new one, never part of
/// either; there is no fsync. The file gets the permissions of one
/// `fs::write` creates (0o666 less the umask), not a temporary file's 0o600.
pub fn atomic_write(
    dir: &Path,
    name: &str,
    prefix: &str,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix).suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    let mut file = builder.tempfile_in(dir)?;
    write(file.as_file_mut())?;
    file.persist(dir.join(name)).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::atomic_write;

    #[test]
    fn the_file_is_replaced_and_no_temporary_is_left() {
        let dir = tempfile::tempdir().unwrap();
        for content in ["first\n", "second\n"] {
            atomic_write(dir.path(), "f.json", ".f.", |file| {
                file.write_all(content.as_bytes())
            })
            .unwrap();
            assert_eq!(
                fs::read_to_string(dir.path().join("f.json")).unwrap(),
                content
            );
        }
        let failed = atomic_write(dir.path(), "f.json", ".f.", |_| {
            Err(std::io::Error::other("no"))
        });
        assert!(failed.is_err());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["f.json"]);
        assert_eq!(
            fs::read_to_string(dir.path().join("f.json")).unwrap(),
            "second\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_has_the_permissions_of_an_ordinary_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        atomic_write(dir.path(), "f", ".f.", |_| Ok(())).unwrap();
        fs::write(dir.path().join("ordinary"), "").unwrap();
        let mode = |name: &str| {
            fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode()
        };
        assert_eq!(mode("f"), mode("ordinary"));
    }
}
