//! A short-lived, guarded cache collector; it never becomes a resident daemon.
use crate::{
    cache::{self, Cache},
    config::Launch,
    process::{self, Group},
    relay::Signals,
    wire::{self, Frame},
};
use anyhow::{Context, Result, bail};
use std::{collections::VecDeque, ffi::OsString, time::Duration};
use tokio::{
    process::Child,
    sync::mpsc,
    time::{Instant, sleep_until},
};

fn collector(launch: &Launch) -> Result<Launch> {
    let mut child = launch.clone();
    child.command = std::env::current_exe()?.into_os_string();
    child.args = vec![
        "cache".into(),
        "bootstrap".into(),
        "--cache-dir".into(),
        launch.cache_dir.as_os_str().to_owned(),
        "--timeout-ms".into(),
        OsString::from(launch.timeout.as_millis().to_string()),
    ];
    if launch.repl_cleanup {
        child.args.push("--repl-cleanup".into());
    }
    for key in &launch.cache_env {
        child
            .args
            .extend([OsString::from("--cache-env"), key.into()]);
    }
    for path in &launch.fingerprints {
        child
            .args
            .extend([OsString::from("--fingerprint"), path.as_os_str().to_owned()]);
    }
    child
        .args
        .extend([OsString::from("--"), launch.command.clone()]);
    child.args.extend(launch.args.iter().cloned());
    child.auto_refresh = false;
    Ok(child)
}

async fn collect(
    helper: &mut Child,
    helper_output: &mut mpsc::Receiver<Result<Option<Frame>>>,
    client: &mut mpsc::Receiver<Result<Option<Frame>>>,
    signals: &mut Signals,
    launch: &Launch,
    first: Frame,
) -> Result<Option<(VecDeque<Frame>, Option<serde_json::Value>)>> {
    let deadline = Instant::now() + launch.timeout + Duration::from_secs(6);
    let mut bytes = first.raw.len();
    let mut queued = VecDeque::from([first]);
    let mut stopped = false;
    let mut output_closed = false;
    let mut error = None;
    loop {
        if stopped && output_closed {
            return Ok(Some((queued, error)));
        }
        tokio::select! {
            _ = signals.next() => return Ok(None),
            _ = sleep_until(deadline) => bail!("automatic cache collection timed out"),
            status = helper.wait(), if !stopped => { status.context("cache collector stopped unexpectedly")?; stopped = true; },
            incoming = helper_output.recv(), if !output_closed => {
                match incoming.context("cache collector output stopped")?? {
                    Some(frame) => { if let Some(value) = frame.value.get("error") { error = Some(value.clone()); } },
                    None => output_closed = true,
                }
            },
            incoming = client.recv() => {
                let Some(frame) = incoming.context("client reader stopped during cache collection")?? else { return Ok(None); };
                bytes += frame.raw.len();
                if bytes > wire::LIMIT { bail!("pending MCP input exceeds 64 MiB"); }
                queued.push_back(frame);
            }
        }
    }
}

/// Collector input is an ownership pipe: even SIGKILL of this parent closes it.
pub async fn bootstrap(
    launch: &Launch,
    client: &mut mpsc::Receiver<Result<Option<Frame>>>,
    signals: &mut Signals,
) -> Result<Option<(Cache, VecDeque<Frame>)>> {
    let Some(first) = (tokio::select! {
        _ = signals.next() => return Ok(None),
        frame = client.recv() => frame.context("client reader stopped before cache collection")??,
    }) else {
        return Ok(None);
    };
    let mut group = Group::collector();
    let mut helper = process::spawn(&collector(launch)?, &mut group).await?;
    let ownership = helper
        .stdin
        .take()
        .context("cache collector ownership pipe unavailable")?;
    let stdout = helper
        .stdout
        .take()
        .context("cache collector output unavailable")?;
    let (tx, mut rx) = mpsc::channel(1);
    tokio::spawn(wire::pump(tokio::io::BufReader::new(stdout), tx));
    let outcome = collect(&mut helper, &mut rx, client, signals, launch, first).await;
    drop(ownership);
    group.terminate(Some(&mut helper)).await?;
    let Some((queued, error)) = outcome? else {
        return Ok(None);
    };
    if let Some(error) = error {
        let mut output = tokio::io::stdout();
        for request in &queued {
            if let Some(id) = request.value.get("id") {
                wire::send(
                    &mut output,
                    &serde_json::json!({"jsonrpc":"2.0","id":id,"error":error}),
                    launch.timeout,
                )
                .await?;
            }
        }
        bail!("backend rejected initialization; details returned to client");
    }
    let cache = cache::load(launch)
        .context("automatic cache collection failed; backend operation was not sent")?;
    Ok(Some((cache, queued)))
}
