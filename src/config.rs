use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

/// Launch settings come from the host's command, cwd and inherited environment.
#[derive(Clone, Debug)]
pub struct Launch {
    pub command: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub cache_dir: PathBuf,
    pub cache_env: Vec<String>,
    pub fingerprints: Vec<PathBuf>,
    pub repl_cleanup: bool,
    pub auto_refresh: bool,
    pub timeout: Duration,
}
