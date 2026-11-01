#![cfg(unix)]

mod auto;
mod cache;
mod config;
mod process;
mod relay;
mod wire;

use anyhow::{Context, Result, bail};
use config::Launch;
use std::{env, ffi::OsString, path::PathBuf, time::Duration};

const HELP: &str = "mcp-doze — start each stdio MCP server on first use\n\n\
Usage:\n  mcp-doze run [OPTIONS] -- COMMAND [ARG...]\n  \
mcp-doze cache refresh [OPTIONS] -- COMMAND [ARG...]\n  \
mcp-doze cache inspect [OPTIONS] -- COMMAND [ARG...]\n\n\
Options:\n  --cache-dir PATH       Cache directory (default: OS user cache)\n  \
--cache-env NAME       Include this environment variable in the cache identity\n  \
--fingerprint PATH     Track an extra script or manifest (repeatable)\n  \
--auto-refresh         Collect a missing or stale cache automatically\n  \
--repl-cleanup         Acknowledge the verified REPL turn_ended tool while cold\n  \
--timeout-ms N         Initialization and I/O timeout (default: 10000)\n  \
-h, --help             Show help\n  -V, --version          Show version\n\n\
The host remains the only MCP configuration source. No daemon, sharing or idle stop.\n";

fn parse() -> Result<(String, Launch)> {
    let mut args = env::args_os().skip(1);
    let first = args.next().context("missing command; use --help")?;
    let mode = if first == "cache" {
        args.next()
            .context("expected cache refresh or cache inspect")?
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid subcommand"))?
    } else if first == "run" {
        "run".to_owned()
    } else {
        bail!("expected run, cache refresh or cache inspect; use --help");
    };
    if !["run", "refresh", "inspect", "bootstrap"].contains(&mode.as_str()) {
        bail!("unknown subcommand; use --help");
    }
    let cache_dir = if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path).join("mcp-doze")
    } else {
        let home =
            PathBuf::from(env::var_os("HOME").context("HOME is required or pass --cache-dir")?);
        if cfg!(target_os = "macos") {
            home.join("Library/Caches/mcp-doze")
        } else {
            home.join(".cache/mcp-doze")
        }
    };
    let mut launch = Launch {
        command: OsString::new(),
        args: vec![],
        cwd: env::current_dir()?,
        cache_dir,
        cache_env: vec![],
        fingerprints: vec![],
        repl_cleanup: false,
        auto_refresh: false,
        timeout: Duration::from_secs(10),
    };
    let mut delimiter = false;
    while let Some(arg) = args.next() {
        if arg == "--" {
            delimiter = true;
            break;
        }
        if arg == "--auto-refresh" {
            launch.auto_refresh = true;
            continue;
        }
        if arg == "--repl-cleanup" {
            launch.repl_cleanup = true;
            continue;
        }
        let value = args.next().context("option requires a value; use --help")?;
        match arg.to_str() {
            Some("--cache-dir") => launch.cache_dir = PathBuf::from(value),
            Some("--cache-env") => {
                let name = value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("cache environment name must be UTF-8"))?;
                if name.is_empty() || name.contains('=') || name.contains('\0') {
                    bail!("invalid cache environment name");
                }
                launch.cache_env.push(name);
            }
            Some("--fingerprint") => launch.fingerprints.push(PathBuf::from(value)),
            Some("--timeout-ms") => {
                let ms: u64 = value
                    .to_str()
                    .context("timeout must be an integer")?
                    .parse()
                    .context("timeout must be an integer")?;
                if ms == 0 || ms > 3_600_000 {
                    bail!("timeout must be between 1 and 3600000 ms");
                }
                launch.timeout = Duration::from_millis(ms);
            }
            _ => bail!("unknown option; use --help"),
        }
    }
    if !delimiter {
        bail!("separate the backend command with --");
    }
    launch.command = args.next().context("missing backend command after --")?;
    launch.args = args.collect();
    Ok((mode, launch))
}

fn main() {
    let first = env::args_os().nth(1);
    if first.as_deref().is_some_and(|x| x == "--help" || x == "-h") {
        print!("{HELP}");
        return;
    }
    if first
        .as_deref()
        .is_some_and(|x| x == "--version" || x == "-V")
    {
        println!("mcp-doze {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let result = (|| -> Result<()> {
        let (mode, launch) = parse()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let outcome = runtime.block_on(async {
            match mode.as_str() {
                "run" => relay::run(&launch).await,
                "refresh" => relay::refresh(&launch, false).await,
                "bootstrap" => relay::refresh(&launch, true).await,
                "inspect" => {
                    let cached = cache::load(&launch)?;
                    println!("{}", serde_json::json!({"cache_file":cache::path(&launch)?,"format":cached.format,"cached_methods":cached.lists.keys().collect::<Vec<_>>(),"protocolVersion":cached.initialize["protocolVersion"],"serverInfo":cached.initialize["serverInfo"]}));
                    Ok(())
                }
                _ => unreachable!(),
            }
        });
        runtime.shutdown_background();
        outcome
    })();
    if let Err(error) = result {
        eprintln!("mcp-doze: {error:#}");
        std::process::exit(1);
    }
    // Tokio's stdin worker may still be blocked after a backend-side disconnect.
    std::process::exit(0);
}
