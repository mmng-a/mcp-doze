//! Private, atomic discovery snapshots. Execution data is never stored here.
use crate::config::Launch;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const FORMAT: u32 = 1;
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;
const REFRESH: &str = "run mcp-doze cache refresh with the same options and command";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Only initialization and discovery results belong in a snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    pub format: u32,
    pub identity: String,
    pub initialize: Value,
    pub lists: BTreeMap<String, Value>,
}

fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

fn executable(launch: &Launch, cwd: &Path) -> Result<PathBuf> {
    let command = Path::new(&launch.command);
    if command.components().count() > 1 || command.is_absolute() {
        return fs::canonicalize(absolute(command, cwd))
            .context("cannot resolve the MCP executable");
    }
    let search = std::env::var_os("PATH").context("PATH is missing")?;
    for directory in std::env::split_paths(&search) {
        let path = absolute(&directory.join(command), cwd);
        if let Ok(meta) = fs::metadata(&path) {
            if meta.is_file() && meta.mode() & 0o111 != 0 {
                return fs::canonicalize(path).context("cannot resolve the MCP executable");
            }
        }
    }
    bail!("MCP executable was not found in PATH")
}

fn fingerprint(hash: &mut Sha256, path: &Path) -> Result<()> {
    let resolved = fs::canonicalize(path).context("cannot resolve a fingerprint path")?;
    let meta = fs::metadata(&resolved).context("cannot inspect a fingerprint path")?;
    feed(hash, resolved.as_os_str().as_bytes());
    for value in [meta.dev(), meta.ino(), meta.mode() as u64] {
        feed(hash, &value.to_le_bytes());
    }
    if meta.is_file() {
        for value in [
            meta.len(),
            meta.mtime() as u64,
            meta.mtime_nsec() as u64,
            meta.ctime() as u64,
            meta.ctime_nsec() as u64,
        ] {
            feed(hash, &value.to_le_bytes());
        }
    }
    Ok(())
}

/// Derive a stable identifier without storing command arguments or environment values.
pub fn identity(launch: &Launch) -> Result<String> {
    let cwd = fs::canonicalize(&launch.cwd).context("cannot resolve the working directory")?;
    let mut hash = Sha256::new();
    feed(&mut hash, b"mcp-doze-discovery");
    feed(&mut hash, &FORMAT.to_le_bytes());
    feed(&mut hash, launch.command.as_bytes());
    fingerprint(&mut hash, &executable(launch, &cwd)?)?;
    feed(&mut hash, cwd.as_os_str().as_bytes());
    feed(&mut hash, &[u8::from(launch.repl_cleanup)]);
    feed(&mut hash, b"arguments");
    feed(&mut hash, &(launch.args.len() as u64).to_le_bytes());
    for argument in &launch.args {
        feed(&mut hash, argument.as_bytes());
        let path = absolute(Path::new(argument), &cwd);
        if path.exists() {
            fingerprint(&mut hash, &path)?;
        }
    }
    feed(&mut hash, b"fingerprints");
    feed(&mut hash, &(launch.fingerprints.len() as u64).to_le_bytes());
    for path in &launch.fingerprints {
        fingerprint(&mut hash, &absolute(path, &cwd))?;
    }
    let mut names = launch.cache_env.clone();
    names.sort_unstable();
    names.dedup();
    feed(&mut hash, b"environment");
    feed(&mut hash, &(names.len() as u64).to_le_bytes());
    for name in names {
        feed(&mut hash, name.as_bytes());
        match std::env::var_os(&name) {
            Some(value) => {
                feed(&mut hash, &[1]);
                feed(&mut hash, value.as_bytes());
            }
            None => feed(&mut hash, &[0]),
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn path(launch: &Launch) -> Result<PathBuf> {
    Ok(launch.cache_dir.join(format!("{}.json", identity(launch)?)))
}

// Keep an open directory descriptor so a rename cannot redirect file operations.
struct Directory(File);

impl Directory {
    fn open(path: &Path, create: bool) -> Result<Self> {
        if create {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .context("cannot create the cache directory")?;
        }
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .context("cannot open a real cache directory")?;
        let meta = directory.metadata()?;
        ensure!(
            meta.is_dir()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o777 == 0o700,
            "cache directory must be owned by this user with mode 0700"
        );
        Ok(Self(directory))
    }

    fn file(&self, name: &str, flags: i32) -> Result<File> {
        let name = CString::new(name)?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata()?;
        ensure!(
            meta.is_file()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o777 == 0o600,
            "cache files must be regular files owned by this user with mode 0600"
        );
        Ok(file)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let name = CString::new(name)?;
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        Ok(())
    }
}

/// Read a complete validated snapshot, or fail without starting a server.
pub fn load(launch: &Launch) -> Result<Cache> {
    let key = identity(launch)?;
    let result = (|| -> Result<Cache> {
        let directory = Directory::open(&launch.cache_dir, false)?;
        let file = directory.file(&format!("{key}.json"), libc::O_RDONLY)?;
        ensure!(
            file.metadata()?.len() <= MAX_CACHE_BYTES,
            "cache is too large"
        );
        let mut bytes = Vec::new();
        file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_CACHE_BYTES, "cache is too large");
        let cache: Cache =
            serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid cache"))?;
        ensure!(
            cache.format == FORMAT && cache.identity == key,
            "cache identity changed"
        );
        ensure!(
            cache.initialize.is_object(),
            "invalid initialization snapshot"
        );
        ensure!(
            cache.lists.values().all(Value::is_object),
            "invalid discovery snapshot"
        );
        Ok(cache)
    })();
    result.with_context(|| format!("discovery cache is unavailable; {REFRESH}"))
}

/// Atomically replace a snapshot. Refresh callers hold `CacheLock` while collecting.
pub fn save(launch: &Launch, cache: &Cache) -> Result<()> {
    let key = identity(launch)?;
    ensure!(
        cache.format == FORMAT && cache.identity == key,
        "cache identity changed during refresh"
    );
    ensure!(
        cache.initialize.is_object() && cache.lists.values().all(Value::is_object),
        "invalid discovery snapshot"
    );
    let bytes = serde_json::to_vec(cache)?;
    ensure!(
        bytes.len() as u64 <= MAX_CACHE_BYTES,
        "discovery cache exceeds 64 MiB"
    );
    let directory = Directory::open(&launch.cache_dir, true)?;
    let temporary = format!(
        ".{key}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let result = (|| -> Result<()> {
        let mut file = directory.file(&temporary, libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let source = CString::new(temporary.as_str())?;
        let destination = CString::new(format!("{key}.json"))?;
        if unsafe {
            libc::renameat(
                directory.0.as_raw_fd(),
                source.as_ptr(),
                directory.0.as_raw_fd(),
                destination.as_ptr(),
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        directory.0.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = directory.unlink(&temporary);
    }
    result.context("cannot save discovery cache")
}

pub fn invalidate(launch: &Launch) -> Result<()> {
    if !launch.cache_dir.exists() {
        return Ok(());
    }
    let directory = Directory::open(&launch.cache_dir, false)?;
    directory.unlink(&format!("{}.json", identity(launch)?))
}

/// Serializes refreshes. The lock file remains in place to keep waiters synchronized.
pub struct CacheLock(File);

impl CacheLock {
    pub fn acquire(launch: &Launch) -> Result<Self> {
        let directory = Directory::open(&launch.cache_dir, true)?;
        let file = directory.file(
            &format!("{}.lock", identity(launch)?),
            libc::O_CREAT | libc::O_RDWR,
        )?;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).context("cannot lock discovery cache");
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "mcp-doze-cache-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            DirBuilder::new().mode(0o700).create(&directory).unwrap();
            Self(directory)
        }

        fn launch(&self) -> Launch {
            Launch {
                command: "/bin/cat".into(),
                args: vec![],
                cwd: self.0.clone(),
                cache_dir: self.0.join("cache"),
                cache_env: vec![],
                fingerprints: vec![],
                repl_cleanup: false,
                auto_refresh: false,
                timeout: Duration::from_secs(10),
            }
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(launch: &Launch) -> Cache {
        Cache {
            format: FORMAT,
            identity: identity(launch).unwrap(),
            initialize: json!({"protocolVersion":"2025-06-18", "capabilities":{"tools":{}}}),
            lists: BTreeMap::from([("tools/list".into(), json!({"tools":[]}))]),
        }
    }

    #[test]
    fn round_trip_private_snapshot_and_reject_corruption() {
        let temporary = Temporary::new();
        let launch = temporary.launch();
        let cache = snapshot(&launch);
        let _lock = CacheLock::acquire(&launch).unwrap();
        save(&launch, &cache).unwrap();
        assert_eq!(load(&launch).unwrap().initialize, cache.initialize);
        let file = path(&launch).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(&launch.cache_dir).unwrap().mode() & 0o777,
            0o700
        );
        fs::write(file, b"broken").unwrap();
        assert!(
            load(&launch)
                .unwrap_err()
                .to_string()
                .contains("cache refresh")
        );
        invalidate(&launch).unwrap();
        assert!(load(&launch).is_err());
    }

    #[test]
    fn argument_file_and_cleanup_policy_change_identity() {
        let temporary = Temporary::new();
        let file = temporary.0.join("entry.py");
        fs::write(&file, b"old").unwrap();
        let mut launch = temporary.launch();
        launch.args.push(file.into_os_string());
        let first = identity(&launch).unwrap();
        fs::write(temporary.0.join("entry.py"), b"new content").unwrap();
        let second = identity(&launch).unwrap();
        assert_ne!(first, second);
        launch.repl_cleanup = true;
        assert_ne!(second, identity(&launch).unwrap());
    }

    #[test]
    fn cache_symlink_is_not_followed_or_written_through() {
        let temporary = Temporary::new();
        let launch = temporary.launch();
        DirBuilder::new()
            .mode(0o700)
            .create(&launch.cache_dir)
            .unwrap();
        let target = temporary.0.join("unrelated");
        fs::write(&target, b"preserve").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, path(&launch).unwrap()).unwrap();
        assert!(load(&launch).is_err());
        save(&launch, &snapshot(&launch)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"preserve");
        assert!(
            !fs::symlink_metadata(path(&launch).unwrap())
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
