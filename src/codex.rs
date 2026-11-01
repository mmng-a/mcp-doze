//! Wrap installed Codex plugin transports without moving their registrations.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

fn directories(path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("cannot read {}", path.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn manifests(home: &Path) -> Result<Vec<PathBuf>> {
    let root = home.join("plugins/cache");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for marketplace in directories(&root)? {
        for plugin in directories(&marketplace)? {
            for version in directories(&plugin)? {
                let path = version.join(".mcp.json");
                if path.try_exists()? {
                    found.push(path);
                }
            }
        }
    }
    Ok(found)
}

fn private_directory(path: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o777 == 0o700,
        "backup directory must be owned by this user with mode 0700"
    );
    Ok(())
}

fn lock(directory: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join(".sync.lock"))?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o777 == 0o600,
        "plugin synchronization lock must be private and owned by this user"
    );
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

fn wrapped(value: &mut Value, binary: &str) -> Result<usize> {
    let Some(servers) = value.get_mut("mcpServers") else {
        return Ok(0);
    };
    let servers = servers
        .as_object_mut()
        .context("mcpServers must be an object")?;
    let mut count = 0;
    for (name, server) in servers {
        ensure!(server.is_object(), "MCP server must be an object");
        if server.get("enabled") == Some(&Value::Bool(false)) || server.get("url").is_some() {
            continue;
        }
        let Some(command) = server.get("command") else {
            continue;
        };
        let command = command
            .as_str()
            .context("MCP command must be a string")?
            .to_owned();
        ensure!(!command.is_empty(), "MCP command cannot be empty");
        if Path::new(&command)
            .file_name()
            .is_some_and(|n| n == "mcp-doze")
        {
            continue;
        }
        let original = match server.get("args") {
            None => Vec::new(),
            Some(args) => args
                .as_array()
                .context("MCP args must be an array")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .context("MCP arguments must be strings")
                })
                .collect::<Result<Vec<_>>>()?,
        };
        let mut args = vec![
            "run".to_owned(),
            "--auto-refresh".to_owned(),
            "--timeout-ms".to_owned(),
            "30000".to_owned(),
        ];
        if name == "cua_repl" {
            args.push("--repl-cleanup".to_owned());
        }
        args.extend(["--".to_owned(), command]);
        args.extend(original);
        server["command"] = json!(binary);
        server["args"] = json!(args);
        count += 1;
    }
    Ok(count)
}

/// Update only command/args. Backups are private and never used as configuration.
pub fn sync(home: &Path, binary: &Path) -> Result<Value> {
    let binary = binary.to_str().context("wrapper path must be UTF-8")?;
    let paths = manifests(home)?;
    if paths.is_empty() {
        return Ok(json!({"manifests":0,"servers":0}));
    }
    let backups = home.join("backups/mcp-doze/plugins");
    private_directory(&backups)?;
    let _lock = lock(&backups)?;
    let mut plans = Vec::new();
    for path in paths {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.len() <= 1024 * 1024,
            "plugin manifests must be regular files owned by this user, at most 1 MiB"
        );
        let mut original = Vec::new();
        (&mut file)
            .take(1024 * 1024 + 1)
            .read_to_end(&mut original)?;
        ensure!(
            original.len() <= 1024 * 1024,
            "plugin manifest exceeds 1 MiB"
        );
        let mut value: Value = serde_json::from_slice(&original)
            .with_context(|| format!("invalid plugin manifest: {}", path.display()))?;
        let count = wrapped(&mut value, binary)?;
        if count > 0 {
            plans.push((path, original, value, meta.permissions(), count));
        }
    }
    let mut count = 0;
    for (path, original, value, permissions, servers) in &plans {
        let record = serde_json::to_vec(
            &json!({"path":path,"original":serde_json::from_slice::<Value>(original)?}),
        )?;
        let key = format!("{:x}", Sha256::digest(&record));
        let backup = backups.join(format!("{key}.json"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&backup)
        {
            Ok(mut file) => {
                file.write_all(&record)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = fs::symlink_metadata(&backup)?;
                ensure!(
                    meta.is_file()
                        && meta.uid() == unsafe { libc::geteuid() }
                        && meta.mode() & 0o777 == 0o600,
                    "existing plugin backup must be private and owned by this user"
                );
                ensure!(
                    fs::read(&backup)? == record,
                    "existing plugin backup differs"
                );
            }
            Err(error) => return Err(error.into()),
        }
        ensure!(
            fs::read(path)? == *original,
            "plugin manifest changed during synchronization"
        );
        let temporary = path.with_file_name(format!(".mcp-doze-{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let outcome = (|| -> Result<()> {
            serde_json::to_writer_pretty(&mut file, value)?;
            file.write_all(b"\n")?;
            file.set_permissions(fs::Permissions::from_mode(permissions.mode()))?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        outcome.with_context(|| format!("cannot update plugin manifest: {}", path.display()))?;
        count += servers;
    }
    Ok(json!({"manifests":plans.len(),"servers":count}))
}

pub fn command(mut args: impl Iterator<Item = OsString>) -> Result<()> {
    ensure!(
        args.next().as_deref().is_some_and(|s| s == "sync-plugins"),
        "expected codex sync-plugins"
    );
    let mut home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".codex")))
        .context("HOME or CODEX_HOME is required")?;
    if let Some(flag) = args.next() {
        if flag != "--codex-home" {
            bail!("expected --codex-home PATH");
        }
        home = PathBuf::from(args.next().context("--codex-home requires a path")?);
    }
    ensure!(args.next().is_none(), "unexpected sync-plugins argument");
    println!("{}", sync(&home, &std::env::current_exe()?)?);
    Ok(())
}
