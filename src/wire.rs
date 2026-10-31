use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::timeout;

pub const LIMIT: usize = 64 * 1024 * 1024;

pub struct Frame {
    pub raw: Vec<u8>,
    pub value: Value,
}

pub async fn read<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Frame>> {
    let mut raw = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if raw.is_empty() {
                return Ok(None);
            }
            bail!("unterminated MCP frame");
        }
        let size = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if raw.len() + size > LIMIT {
            bail!("MCP frame exceeds 64 MiB");
        }
        let done = available[size - 1] == b'\n';
        raw.extend_from_slice(&available[..size]);
        reader.consume(size);
        if done {
            break;
        }
    }
    let value: Value = serde_json::from_slice(&raw).context("invalid MCP JSON frame")?;
    if !value.is_object() || value["jsonrpc"] != "2.0" {
        bail!("expected a JSON-RPC 2.0 object");
    }
    Ok(Some(Frame { raw, value }))
}

pub async fn pump<R: AsyncBufRead + Unpin>(mut reader: R, tx: mpsc::Sender<Result<Option<Frame>>>) {
    loop {
        let result = read(&mut reader).await;
        let done = !matches!(&result, Ok(Some(_)));
        if tx.send(result).await.is_err() || done {
            break;
        }
    }
}

pub async fn write<W: AsyncWrite + Unpin>(
    writer: &mut W,
    raw: &[u8],
    limit: std::time::Duration,
) -> Result<()> {
    timeout(limit, async {
        writer.write_all(raw).await?;
        writer.flush().await
    })
    .await
    .context("MCP write timed out")??;
    Ok(())
}

pub async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
    limit: std::time::Duration,
) -> Result<()> {
    let mut raw = serde_json::to_vec(value)?;
    if raw.len() >= LIMIT {
        bail!("MCP response exceeds 64 MiB");
    }
    raw.push(b'\n');
    write(writer, &raw, limit).await
}
