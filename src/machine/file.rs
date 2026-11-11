//! Descriptor-relative Unix IO: no symlink traversal, replacement, or public output.
use crate::{invalid, Result};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::{Component, Path},
};

pub struct Output {
    directory: File,
    name: CString,
}

fn failure() -> crate::Error {
    invalid("Token-Datei benötigt ein absolutes neues Ziel in einem eigenen privaten Verzeichnis; keine Links, fremden Eigentümer oder unsicheren Rechte.")
}

fn open_at(directory: &File, name: &CString, flags: i32, mode: u32) -> Result<File> {
    // CString is NUL-terminated; the live directory descriptor anchors the lookup.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode as libc::mode_t,
        )
    };
    if fd < 0 {
        return Err(failure());
    }
    // A successful openat returns a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Output {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(failure());
        }
        let mut parts = path.components();
        if parts.next() != Some(Component::RootDir) {
            return Err(failure());
        }
        let parts: Vec<_> = parts.collect();
        if parts.is_empty() {
            return Err(failure());
        }
        let mut directory = File::open("/").map_err(|_| failure())?;
        // geteuid has no preconditions and does not expose credentials.
        let uid = unsafe { libc::geteuid() };
        for (i, part) in parts.iter().enumerate() {
            let Component::Normal(name) = part else {
                return Err(failure());
            };
            use std::os::unix::ffi::OsStrExt;
            let name = CString::new(name.as_bytes()).map_err(|_| failure())?;
            let meta = directory.metadata().map_err(|_| failure())?;
            let last = i == parts.len() - 1;
            if !meta.is_dir()
                || (meta.uid() != uid && meta.uid() != 0)
                || (last && (meta.uid() != uid || meta.mode() & 0o077 != 0))
                || (!last
                    && meta.mode() & 0o022 != 0
                    && !(meta.uid() == 0 && meta.mode() & 0o1000 != 0))
            {
                return Err(failure());
            }
            if last {
                return Ok(Self { directory, name });
            }
            directory = open_at(&directory, &name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        }
        Err(failure())
    }

    fn check(file: &File) -> Result<()> {
        let meta = file.metadata().map_err(|_| failure())?;
        let uid = unsafe { libc::geteuid() };
        if !meta.is_file()
            || meta.uid() != uid
            || meta.mode() & 0o7777 != 0o600
            || meta.nlink() != 1
        {
            return Err(failure());
        }
        Ok(())
    }

    pub fn write_new(&self, bytes: &[u8]) -> Result<()> {
        let mut file = open_at(
            &self.directory,
            &self.name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?;
        // fchmod only touches the descriptor just created with O_EXCL (not a path).
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(failure());
        }
        Self::check(&file)?;
        file.write_all(bytes).map_err(|_| failure())?;
        file.sync_all().map_err(|_| failure())?;
        self.directory.sync_all().map_err(|_| failure())?;
        Ok(())
    }

    pub fn matches(&self, expected_hash: &str) -> Result<()> {
        let file = open_at(
            &self.directory,
            &self.name,
            libc::O_RDONLY | libc::O_NONBLOCK,
            0,
        )?;
        Self::check(&file)?;
        let mut value = String::new();
        file.take(4097)
            .read_to_string(&mut value)
            .map_err(|_| failure())?;
        if value.len() > 4096 || crate::storage::hash(&value) != expected_hash {
            return Err(invalid("Bestehende Token-Ausgabe fehlt oder stimmt nicht überein. Keine automatische Erneuerung; neue Generation ausdrücklich mit renew anlegen."));
        }
        Ok(())
    }
}
