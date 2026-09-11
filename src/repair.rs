//! Replace covers that were stored black.
//!
//! For a while the upload page captured a clip's poster before the phone had
//! decoded a frame, and the server stored the black canvas it was given.
//! Those rows are still here, and nothing about them changes on its own. So at
//! startup, a little after the site is serving, this walks every clip and GIF,
//! looks at the stored still, and where it is flat draws a new one -- ffmpeg
//! for a clip, the liveliest frame for a GIF -- and writes it and its
//! thumbnail back under the same keys. URLs do not change; the edge cache is
//! purged separately.
//!
//! Idempotent and cheap: a healthy cover costs one small GET. Set
//! `REPAIR_COVERS=0` to skip it.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::media_tools::{is_flat, liveliest_gif_frame, video_poster};
use crate::storage::{
    backend, bound, encode_jpeg, encode_jpeg_at, orig_key, poster_key, thumb_key, POSTER_QUALITY,
    THUMB_MAX_EDGE,
};

#[derive(Deserialize)]
struct Row {
    id: String,
    kind: String,
    orig_url: String,
}

/// Run once, off the request path. Errors are logged per row; one bad file
/// does not stop the pass.
pub async fn covers() {
    if std::env::var("REPAIR_COVERS").is_ok_and(|v| v == "0" || v == "false") {
        return;
    }
    // Let the site come up first: this is maintenance, not startup.
    tokio::time::sleep(Duration::from_secs(20)).await;

    let rows: Vec<Row> = match crate::db::client()
        .query(
            "SELECT id, kind, orig_url FROM items WHERE kind IN ('video','gif')",
            vec![json!(null)].into_iter().take(0).collect(),
        )
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("repair: could not list clips: {e}");
            return;
        }
    };
    let mut fixed = 0usize;
    for row in &rows {
        match repair_one(row).await {
            Ok(true) => fixed += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(id = row.id, "repair: {e}"),
        }
    }
    tracing::info!(
        "repair: {} clips/GIFs checked, {fixed} covers replaced",
        rows.len()
    );
}

async fn repair_one(row: &Row) -> anyhow::Result<bool> {
    let be = backend();
    let is_video = row.kind == "video";
    let still_key = if is_video {
        poster_key(&row.id)
    } else {
        thumb_key(&row.id)
    };
    let current = be.get(&still_key).await?;
    let img = image::load_from_memory(&current)?;
    if !is_flat(&img) {
        return Ok(false);
    }

    // The original's extension is in its URL; the key carries the same one.
    let ext = row
        .orig_url
        .rsplit('.')
        .next()
        .filter(|e| ["mp4", "webm", "gif"].contains(e))
        .unwrap_or(if is_video { "mp4" } else { "gif" });
    let orig = be.get(&orig_key(&row.id, ext)).await?;

    let fresh = if is_video {
        let jpeg = video_poster(&orig)
            .await
            .ok_or_else(|| anyhow::anyhow!("ffmpeg drew nothing"))?;
        image::load_from_memory(&jpeg)?
    } else {
        liveliest_gif_frame(&orig).ok_or_else(|| anyhow::anyhow!("no GIF frame"))?
    };
    if is_flat(&fresh) {
        anyhow::bail!("the file really is that dark");
    }
    let fresh = bound(&fresh);
    let thumb = encode_jpeg(&fresh.thumbnail(THUMB_MAX_EDGE, THUMB_MAX_EDGE))?;
    if is_video {
        let poster = encode_jpeg_at(&fresh, POSTER_QUALITY)?;
        be.put(&poster_key(&row.id), poster, "image/jpeg").await?;
    }
    be.put(&thumb_key(&row.id), thumb, "image/jpeg").await?;
    tracing::info!(id = row.id, kind = row.kind, "repair: cover replaced");
    Ok(true)
}
