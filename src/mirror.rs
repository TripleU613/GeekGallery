//! An hourly pull from another gallery, for a site kept in step with one that
//! does not run this codebase.
//!
//! Off unless `MIRROR_SITEMAP` names a sitemap. With it set, every pass reads
//! that one file, works out which of its links this site has not already
//! taken, and hands the new ones to the ordinary importer. Nothing else is
//! read: no crawl, no link following, no guessing at URLs. A steady week is
//! one request an hour.
//!
//! What makes the diff exact is that the importer records the page it was
//! given as the row's `source_url`, so the sitemap and the database are
//! comparing the same strings. Two independent things then keep a repeat pass
//! from costing anything: a link already on record is never fetched, and a
//! file already stored is refused by its content hash even if it arrives from
//! a URL nobody has seen before.
//!
//! Deliberately in the app rather than a scheduled job somewhere else. It
//! needs the database, the storage backend, the guarded fetch and the job
//! registry, all of which live here -- an external runner would need a
//! credential and an endpoint of its own to reach any of them, which is three
//! new things to secure in exchange for a timer.

use std::time::Duration;

/// A sitemap is small. This is the ceiling on a page that claims otherwise.
const SITEMAP_CAP: usize = 8 * 1024 * 1024;

/// How long between passes when `MIRROR_EVERY_MINS` says nothing.
const DEFAULT_EVERY_MINS: u64 = 60;

/// How many new links one pass will take on when `MIRROR_PER_RUN` says
/// nothing.
///
/// A cap, not a target: it exists so a first pass against a gallery of
/// thousands spreads over a day instead of queueing thousands of fetches at
/// once. `jobs::spawn` runs three at a time regardless, so this bounds how
/// much is waiting, not how hard the other site is hit.
const DEFAULT_PER_RUN: usize = 40;

/// Never faster than this, whatever the environment asks for. A mirror is not
/// a poller, and a typo in a variable should not turn one into a nuisance.
const FLOOR_MINS: u64 = 15;

struct Config {
    sitemap: String,
    /// Only links containing this are considered. A gallery that publishes
    /// every page once per language lists each item several times; without a
    /// filter the mirror would import each of them and lean on the content
    /// hash to refuse the copies, which works but wastes a fetch apiece.
    matching: Option<String>,
    every: Duration,
    per_run: usize,
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn config() -> Option<Config> {
    let sitemap = env_non_empty("MIRROR_SITEMAP")?;
    let mins = env_non_empty("MIRROR_EVERY_MINS")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_EVERY_MINS)
        .max(FLOOR_MINS);
    let per_run = env_non_empty("MIRROR_PER_RUN")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_PER_RUN);
    Some(Config {
        sitemap,
        matching: env_non_empty("MIRROR_MATCH"),
        every: Duration::from_secs(mins * 60),
        per_run,
    })
}

/// Start the loop, or log why there isn't one. Called once at startup.
pub fn spawn() {
    let Some(cfg) = config() else {
        tracing::info!("mirror: off (MIRROR_SITEMAP unset)");
        return;
    };
    tracing::info!(
        "mirror: {} every {} min, up to {} new link(s) a pass{}",
        cfg.sitemap,
        cfg.every.as_secs() / 60,
        cfg.per_run,
        match &cfg.matching {
            Some(m) => format!(", only links containing {m}"),
            None => String::new(),
        }
    );
    tokio::spawn(async move {
        loop {
            match pass(&cfg).await {
                Ok((0, seen)) => {
                    tracing::info!("mirror: nothing new ({seen} link(s) on the sitemap)");
                }
                Ok((queued, seen)) => {
                    tracing::info!("mirror: queued {queued} new of {seen} link(s)");
                }
                // A pass that fails is a pass skipped, never a process that
                // stops: the other site being down for an hour must not mean
                // the mirror is off until the next deploy.
                Err(e) => tracing::warn!("mirror: pass failed, trying again next time: {e}"),
            }
            tokio::time::sleep(cfg.every).await;
        }
    });
}

/// One pass. Returns how many links were queued and how many the sitemap
/// offered, for the log line.
async fn pass(cfg: &Config) -> anyhow::Result<(usize, usize)> {
    let xml = crate::fetch::guarded_text(&cfg.sitemap, SITEMAP_CAP)
        .await
        .map_err(|e| anyhow::anyhow!("could not read the sitemap: {e}"))?;

    let mut links = locs(&xml);
    if let Some(m) = &cfg.matching {
        links.retain(|l| l.contains(m.as_str()));
    }
    let seen = links.len();

    let known = crate::db::known_source_urls().await?;
    let mut queued = 0;
    for link in links {
        if queued >= cfg.per_run {
            break;
        }
        if known.contains(&link) {
            continue;
        }
        crate::import_route::queue(link, String::new(), None);
        queued += 1;
    }
    Ok((queued, seen))
}

/// The `<loc>` values in a sitemap.
///
/// Read with a string scan rather than an XML parser: the shape is two fixed
/// tags and this app has no XML dependency to spend on one file. A sitemap
/// index (a list of other sitemaps) has the same `<loc>` shape, so pointing
/// the mirror at one yields sitemap URLs rather than pages -- which import as
/// nothing and log as refusals. Point it at the leaf.
fn locs(xml: &str) -> Vec<String> {
    const OPEN: &str = "<loc>";
    const CLOSE: &str = "</loc>";
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(OPEN) {
        rest = &rest[i + OPEN.len()..];
        let Some(end) = rest.find(CLOSE) else { break };
        let raw = rest[..end].trim();
        if !raw.is_empty() {
            // `&` is the one character a sitemap has to escape and a URL is
            // allowed to contain; the rest never appear in one.
            out.push(raw.replace("&amp;", "&"));
        }
        rest = &rest[end + CLOSE.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::locs;

    #[test]
    fn reads_the_locs_out_of_a_sitemap() {
        let xml = "<?xml version=\"1.0\"?>\n<urlset>\n\
             <url><loc>https://example.com/a</loc><lastmod>2026-01-01</lastmod></url>\n\
             <url><loc>https://example.com/b?x=1&amp;y=2</loc></url>\n\
             </urlset>";
        assert_eq!(
            locs(xml),
            vec![
                "https://example.com/a".to_string(),
                "https://example.com/b?x=1&y=2".to_string(),
            ]
        );
    }

    /// Whitespace around a `<loc>` is normal in a pretty-printed sitemap, and
    /// a URL with a newline in it would be refused by the fetcher rather than
    /// fetched -- so it is trimmed here, where the reason is obvious.
    #[test]
    fn trims_and_skips_empty_entries() {
        let xml = "<loc>\n  https://example.com/a\n  </loc><loc></loc><loc>   </loc>";
        assert_eq!(locs(xml), vec!["https://example.com/a".to_string()]);
    }

    #[test]
    fn an_unclosed_tag_ends_the_scan_rather_than_looping() {
        assert_eq!(
            locs("<loc>https://example.com/a</loc><loc>https://example.com/b"),
            vec!["https://example.com/a".to_string()]
        );
        assert!(locs("no tags here").is_empty());
    }
}
