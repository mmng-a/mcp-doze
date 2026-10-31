//! Own only the process group created for this MCP connection or cache collection.

use crate::config::Launch;
use anyhow::{Context, Result, bail, ensure};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

const TERMINATION_GRACE: Duration = Duration::from_secs(2);

enum Kind {
    Wrapper,
    Collector,
}

/// A connection owns its wrapper group; cache collection owns a backend-only group.
///
/// Before spawning in wrapper mode, the caller must install handlers for SIGTERM,
/// SIGINT and SIGHUP. In particular, its SIGTERM handler must remain installed while
/// `terminate` signals the wrapper's own group.
pub struct Group {
    kind: Kind,
    creator: libc::pid_t,
    pgid: Option<libc::pid_t>,
    started: bool,
    closing: bool,
    finished: bool,
}

impl Group {
    /// Establish a dedicated group before creating any backend processes.
    pub fn wrapper() -> Result<Self> {
        // These calls only inspect or change the current process's group.
        let pid = unsafe { libc::getpid() };
        if unsafe { libc::getpgrp() } != pid && unsafe { libc::setpgid(0, 0) } != 0 {
            return Err(io::Error::last_os_error())
                .context("could not establish a dedicated MCP process group");
        }
        ensure!(
            unsafe { libc::getpgrp() } == pid,
            "MCP wrapper must be its own process-group leader"
        );
        Ok(Self {
            kind: Kind::Wrapper,
            creator: pid,
            pgid: Some(pid),
            started: false,
            closing: false,
            finished: false,
        })
    }

    /// Cache collection keeps the CLI outside the backend's group so it can return.
    pub fn collector() -> Self {
        Self {
            kind: Kind::Collector,
            creator: unsafe { libc::getpid() },
            pgid: None,
            started: false,
            closing: false,
            finished: false,
        }
    }

    fn owns_target(&self) -> bool {
        if self.creator != unsafe { libc::getpid() } {
            return false;
        }
        let Some(pgid) = self.pgid.filter(|id| *id > 1) else {
            return false;
        };
        match self.kind {
            Kind::Wrapper => pgid == self.creator && unsafe { libc::getpgrp() } == pgid,
            Kind::Collector => {
                if unsafe { libc::getpgrp() } == pgid {
                    return false;
                }
                let actual = unsafe { libc::getpgid(pgid) };
                // The leader can exit before its children. Its absent PID must not
                // prevent recovery of the still-owned group.
                actual == pgid
                    || (actual == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH))
            }
        }
    }

    fn signal(&self, signal: libc::c_int) -> Result<()> {
        ensure!(
            self.owns_target(),
            "refusing to signal an unowned process group"
        );
        let pgid = self.pgid.expect("ownership requires a process-group ID");
        if unsafe { libc::kill(-pgid, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(error).context("could not signal the owned MCP process group")
    }

    fn signal_reaping(&self, signal: libc::c_int, mut child: Option<&mut Child>) -> Result<()> {
        for attempt in 0..2 {
            if let Some(child) = child.as_mut() {
                child
                    .try_wait()
                    .context("could not reap an exited MCP backend")?;
            }
            match self.signal(signal) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    // Darwin may return EPERM for an unreaped, already-exited group.
                    let permission = error
                        .downcast_ref::<io::Error>()
                        .and_then(io::Error::raw_os_error)
                        == Some(libc::EPERM);
                    if attempt != 0 || !permission || child.is_none() {
                        return Err(error);
                    }
                }
            }
        }
        unreachable!()
    }

    /// Stop accepting work before awaiting this method; do not cancel it on signals.
    ///
    /// Wrapper mode deliberately ends in SIGKILL of its entire group, including
    /// itself. Waiting only for the direct child would leave surviving grandchildren.
    /// Collector mode reaps its direct child and returns after group recovery.
    pub async fn terminate(&mut self, child: Option<&mut Child>) -> Result<()> {
        if self.finished || self.closing {
            return Ok(());
        }
        self.closing = true;
        if !self.started {
            self.finished = true;
            return Ok(());
        }
        let mut child = child;
        if let Some(child) = child.as_mut() {
            drop(child.stdin.take());
        }
        self.signal_reaping(libc::SIGTERM, child.as_deref_mut())?;
        tokio::time::sleep(TERMINATION_GRACE).await;
        self.signal_reaping(libc::SIGKILL, child.as_deref_mut())?;
        if let Kind::Wrapper = self.kind {
            bail!("owned MCP wrapper group survived SIGKILL");
        }
        if let Some(child) = child {
            child
                .wait()
                .await
                .context("could not reap the MCP backend")?;
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // Unwinding or a cancelled cleanup must not abandon backend descendants.
        if !self.started || self.finished || !self.owns_target() {
            return;
        }
        if let Some(pgid) = self.pgid {
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
    }
}

/// Spawn one backend with its existing environment and working directory intact.
pub async fn spawn(launch: &Launch, group: &mut Group) -> Result<Child> {
    ensure!(
        !group.started && !group.closing,
        "MCP backend already started or closing"
    );
    if let Kind::Wrapper = group.kind {
        ensure!(
            group.owns_target(),
            "MCP wrapper lost its dedicated process group"
        );
    }
    let mut command = Command::new(&launch.command);
    command
        .args(&launch.args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Kind::Collector = group.kind {
        command.as_std_mut().process_group(0);
    }
    let child = command.spawn().context("could not start the MCP backend")?;
    let pid = child
        .id()
        .context("spawned MCP backend has no process ID")? as libc::pid_t;
    if let Kind::Collector = group.kind {
        group.pgid = Some(pid);
    }
    group.started = true;
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[tokio::test(flavor = "current_thread")]
    async fn collector_escalates_without_signalling_the_callers_group() {
        let caller_group = unsafe { libc::getpgrp() };
        let launch = Launch {
            command: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "trap '' TERM; printf 'ready\\n'; sleep 600 & wait".into(),
            ],
            cwd: std::env::temp_dir(),
            cache_dir: std::env::temp_dir(),
            cache_env: vec![],
            fingerprints: vec![],
            repl_cleanup: false,
            timeout: Duration::from_secs(5),
        };
        let mut group = Group::collector();
        let mut child = spawn(&launch, &mut group).await.unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        tokio::time::timeout(Duration::from_secs(5), output.read_line(&mut ready))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ready, "ready\n");
        assert_ne!(group.pgid, Some(caller_group));
        group.terminate(Some(&mut child)).await.unwrap();
        assert_eq!(child.wait().await.unwrap().signal(), Some(libc::SIGKILL));
        assert_eq!(unsafe { libc::getpgrp() }, caller_group);
    }
}
