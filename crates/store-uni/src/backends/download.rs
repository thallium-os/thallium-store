//! Native downloader with parallel range segmentation.
//!
//! Replaces UNI's `aria2c -x8 -s8` shell-out. When the server advertises
//! `Accept-Ranges: bytes` and the file is large enough, the body is fetched
//! as N concurrent range requests written to their offsets in a preallocated
//! file; otherwise it falls back to a single stream. Bytes fetched are
//! published through a shared counter so the caller can emit real progress
//! without coupling to the transport.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

const SEGMENTS: u64 = 8;
const MIN_SEGMENTED: u64 = 4 * 1024 * 1024; // below 4 MiB a single stream wins

/// Download `url` to `dest`, adding fetched bytes to `done`. Returns total size.
pub async fn download(
    url: &str,
    dest: &Path,
    done: &Arc<AtomicU64>,
    total_out: &Arc<AtomicU64>,
) -> Result<u64> {
    let client = reqwest::Client::builder()
        .user_agent("thallium-store")
        .build()?;

    let head = client.head(url).send().await?;
    let total = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let ranged = head
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("bytes"))
        .unwrap_or(false);

    total_out.store(total, Ordering::Relaxed);

    if ranged && total >= MIN_SEGMENTED {
        segmented(&client, url, dest, total, done).await?;
    } else {
        single(&client, url, dest, done).await?;
    }
    Ok(total)
}

async fn single(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    done: &Arc<AtomicU64>,
) -> Result<()> {
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let mut file = tokio::fs::File::create(dest).await?;
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await?;
        done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush().await?;
    Ok(())
}

async fn segmented(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    total: u64,
    done: &Arc<AtomicU64>,
) -> Result<()> {
    // Preallocate so each segment can seek to its own offset.
    let file = tokio::fs::File::create(dest).await?;
    file.set_len(total).await?;
    drop(file);

    let seg = total.div_ceil(SEGMENTS);
    let mut handles = Vec::new();
    for i in 0..SEGMENTS {
        let start = i * seg;
        if start >= total {
            break;
        }
        let end = (start + seg - 1).min(total - 1);
        let client = client.clone();
        let url = url.to_string();
        let dest = dest.to_path_buf();
        let done = Arc::clone(done);
        handles.push(tokio::spawn(async move {
            let mut resp = client
                .get(&url)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await?
                .error_for_status()?;
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&dest)
                .await?;
            file.seek(std::io::SeekFrom::Start(start)).await?;
            while let Some(chunk) = resp.chunk().await? {
                file.write_all(&chunk).await?;
                done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
            }
            file.flush().await?;
            Ok::<(), anyhow::Error>(())
        }));
    }

    for handle in handles {
        handle
            .await
            .map_err(|err| anyhow!("download task panicked: {err}"))?
            .context("download segment failed")?;
    }
    Ok(())
}
