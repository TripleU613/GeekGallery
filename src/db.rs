//! Data access, over Cloudflare D1 (see `d1.rs` for the transport and what its
//! HTTP API does and does not make atomic).
//!
//! There is no local database at all — this app runs on a server explicitly
//! provisioned with no persistent local storage, so every read and write is a
//! network call to D1. Local dev talks to the same D1 database as production;
//! that is a deliberate simplification (see README) to avoid two divergent
//! code paths — a SQLite-shaped dev path and a D1-shaped prod path — that could
//! silently behave differently at exactly the places that matter most
//! (`toggle_like`'s lack of cross-call transactions, in particular).

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::json;

use crate::d1::D1;
use crate::models::{Item, ItemPage, LeaderboardEntry, MediaKind, Sort, Uploader, User, PAGE_SIZE};

/// Process-wide client, set once at startup.
static D1_CLIENT: std::sync::OnceLock<D1> = std::sync::OnceLock::new();

pub fn set_client(d1: D1) {
    let _ = D1_CLIENT.set(d1);
}

/// Panics if called before `set_client`, which can only happen if a handler
/// runs before startup finished — not reachable in practice.
pub fn client() -> &'static D1 {
    D1_CLIENT.get().expect("D1 client not initialized")
}

/// The part of a Google display name that is safe to publish: the first word.
///
/// Google hands back a full "Given Family" name and, until now, the whole of it
/// went out on every card, every detail byline, the leaderboard, the public
/// `/api/v1/items` payload, the oEmbed `author_name`, and -- worst, because
/// search engines keep it -- the `og:description` and the schema.org `creator`
/// of every item that person ever uploaded. Contributing a meme should not
/// publish somebody's surname.
///
/// Redacted HERE, where a database row becomes a `Item` or a `LeaderboardEntry`,
/// rather than in each component that renders one. Same reasoning as `Uploader`
/// carrying no email and no Google subject id: a value that never enters the
/// struct cannot leak through a server function response later, and there is no
/// list of render sites to keep in sync. `users.display_name` still holds the
/// full name, which is what the signed-in visitor's own account menu reads.
///
/// Splitting on whitespace is a heuristic and a deliberately timid one. A
/// mononym, or any name the person chose to write without a space, comes back
/// unchanged -- so the failure mode is "published a whole name that was already
/// one word", never "published nothing". An all-whitespace name is returned as
/// it was for the same reason: it rendered as an empty byline before this
/// function existed and still does, rather than this silently inventing a name.
pub(crate) fn public_first_name(display_name: &str) -> String {
    match display_name.split_whitespace().next() {
        Some(first) => first.to_string(),
        None => display_name.to_string(),
    }
}

// A LEFT JOIN against users, not a separate per-row query: this is the hot
// path (every gallery page, every detail view), and joining costs nothing D1
// doesn't already pay for a single indexed lookup.
const ITEM_SELECT: &str =
    "SELECT s.id, s.slug, s.title, s.kind, s.orig_url, s.thumb_url, s.poster_url, \
     s.width, s.height, s.duration, s.source_url, s.created_at, s.is_public, s.reports, \
     s.likes, u.display_name AS uploader_name, u.avatar_url AS uploader_avatar \
     FROM items s LEFT JOIN users u ON u.id = s.uploader_id";

/// Shape of a row as D1 returns it: JSON, with SQLite's native types (no bool,
/// no fixed-width ints/floats). Converted into `Item` below.
#[derive(Deserialize)]
struct ItemRow {
    id: String,
    // Nullable in the schema (see migration 0008), so a row written before the
    // column existed still deserializes; the id stands in when it is absent.
    slug: Option<String>,
    title: String,
    // Nullable for the same reason as `slug`: the column arrived in migration
    // 0009 with a default, but a row read through an older SELECT shape has to
    // keep deserializing. An absent kind is an image.
    #[serde(default)]
    kind: Option<String>,
    orig_url: String,
    thumb_url: String,
    #[serde(default)]
    poster_url: Option<String>,
    width: i64,
    height: i64,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    source_url: Option<String>,
    created_at: String,
    is_public: i64,
    reports: i64,
    likes: i64,
    uploader_name: Option<String>,
    uploader_avatar: Option<String>,
}

impl From<ItemRow> for Item {
    fn from(r: ItemRow) -> Self {
        Item {
            slug: r.slug.clone().unwrap_or_else(|| r.id.clone()),
            id: r.id,
            title: r.title,
            kind: MediaKind::from_str_or_default(r.kind.as_deref().unwrap_or("image")),
            orig_url: r.orig_url,
            thumb_url: r.thumb_url,
            poster_url: r.poster_url,
            width: r.width as u32,
            height: r.height as u32,
            duration: r.duration,
            source_url: r.source_url,
            created_at: r.created_at,
            is_public: r.is_public != 0,
            reports: r.reports,
            likes: r.likes,
            // Both depend on who is asking / a follow-up query; filled in by
            // the caller (mark_liked).
            liked_by_me: false,
            // Every read path in the app funnels through this conversion, so
            // redacting here covers the gallery, the detail page, search,
            // `MoreItems`, the public JSON API and the oEmbed response in one
            // place.
            uploader: r.uploader_name.map(|display_name| Uploader {
                display_name: public_first_name(&display_name),
                avatar_url: r.uploader_avatar,
            }),
        }
    }
}

/// Cursors are opaque to the client but encode the full sort key, because
/// keyset pagination needs a total order. `likes` alone is not one — many items
/// share a value — so that cursor carries a tie-breaker too.
/// Titles can't contain control characters (`upload_route::clean_title`
/// strips them), so NUL is a safe separator there even though `|` could
/// theoretically appear in free-text titles.
fn encode_cursor(s: &Item, sort: Sort) -> String {
    match sort {
        Sort::Newest | Sort::Clips => s.created_at.clone(),
        Sort::MostLiked => format!("{}|{}", s.likes, s.created_at),
        Sort::Az => format!("{}\u{0}{}", s.title, s.id),
    }
}

fn decode_liked_cursor(cursor: &str) -> (i64, String) {
    match cursor.split_once('|') {
        Some((likes, created)) => (likes.parse().unwrap_or(i64::MAX), created.to_string()),
        // Malformed cursor: start from the top rather than erroring at the
        // user, since a cursor only ever comes from us.
        None => (i64::MAX, String::new()),
    }
}

fn decode_az_cursor(cursor: &str) -> (String, String) {
    match cursor.split_once('\u{0}') {
        Some((title, id)) => (title.to_string(), id.to_string()),
        None => (String::new(), String::new()),
    }
}

/// One page of public items.
pub async fn list_public(
    cursor: Option<&str>,
    sort: Sort,
    voter: Option<&str>,
) -> anyhow::Result<ItemPage> {
    let rows: Vec<ItemRow> = match sort {
        Sort::Newest => {
            let sql = format!(
                "{ITEM_SELECT} \
                 WHERE s.is_public = 1 AND (?1 IS NULL OR s.created_at < ?1) \
                 ORDER BY s.created_at DESC, s.id DESC \
                 LIMIT ?2"
            );
            client()
                .query(&sql, vec![json!(cursor), json!(PAGE_SIZE + 1)])
                .await?
        }
        // Newest, restricted to what moves. Same cursor shape as Newest, so the
        // two share `encode_cursor`'s arm; the index from migration 0009 covers
        // the `kind` predicate.
        Sort::Clips => {
            let sql = format!(
                "{ITEM_SELECT} \
                 WHERE s.is_public = 1 AND s.kind IN ('video', 'gif') \
                   AND (?1 IS NULL OR s.created_at < ?1) \
                 ORDER BY s.created_at DESC, s.id DESC \
                 LIMIT ?2"
            );
            client()
                .query(&sql, vec![json!(cursor), json!(PAGE_SIZE + 1)])
                .await?
        }
        Sort::MostLiked => {
            let (likes, created) = cursor
                .map(decode_liked_cursor)
                .unwrap_or((i64::MAX, String::new()));
            let sql = format!(
                "{ITEM_SELECT} \
                 WHERE s.is_public = 1 \
                   AND (?1 = 0 OR s.likes < ?2 OR (s.likes = ?2 AND s.created_at < ?3)) \
                 ORDER BY s.likes DESC, s.created_at DESC, s.id DESC \
                 LIMIT ?4"
            );
            client()
                .query(
                    &sql,
                    vec![
                        json!(i64::from(cursor.is_some())),
                        json!(likes),
                        json!(created),
                        json!(PAGE_SIZE + 1),
                    ],
                )
                .await?
        }
        Sort::Az => {
            let (title, id) = cursor.map(decode_az_cursor).unwrap_or_default();
            let sql = format!(
                "{ITEM_SELECT} \
                 WHERE s.is_public = 1 \
                   AND (?1 = 0 OR s.title COLLATE NOCASE > ?2 \
                        OR (s.title COLLATE NOCASE = ?2 AND s.id > ?3)) \
                 ORDER BY s.title COLLATE NOCASE ASC, s.id ASC \
                 LIMIT ?4"
            );
            client()
                .query(
                    &sql,
                    vec![
                        json!(i64::from(cursor.is_some())),
                        json!(title),
                        json!(id),
                        json!(PAGE_SIZE + 1),
                    ],
                )
                .await?
        }
    };

    let has_more = rows.len() as i64 > PAGE_SIZE;
    let mut items: Vec<Item> = rows
        .into_iter()
        .take(PAGE_SIZE as usize)
        .map(Item::from)
        .collect();

    let next_cursor = if has_more {
        items.last().map(|s| encode_cursor(s, sort))
    } else {
        None
    };

    mark_liked(&mut items, voter).await?;
    Ok(ItemPage { items, next_cursor })
}

/// Fill in `liked_by_me` for a page in one query rather than one per card.
///
/// `voter` is a signed-in user's id. The column is still called `voter_id` and
/// still holds anonymous ids from before likes required an account (see
/// `toggle_like`), so the name stays; what changed is who can be one.
async fn mark_liked(items: &mut [Item], voter: Option<&str>) -> anyhow::Result<()> {
    let Some(voter) = voter else { return Ok(()) };
    if items.is_empty() {
        return Ok(());
    }

    let holes = (0..items.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT item_id FROM likes WHERE voter_id = ?1 AND item_id IN ({holes})");

    let mut params = vec![json!(voter)];
    params.extend(items.iter().map(|s| json!(s.id)));

    #[derive(Deserialize)]
    struct LikedRow {
        item_id: String,
    }
    let liked: HashSet<String> = client()
        .query::<LikedRow>(&sql, params)
        .await?
        .into_iter()
        .map(|r| r.item_id)
        .collect();

    for s in items.iter_mut() {
        s.liked_by_me = liked.contains(&s.id);
    }
    Ok(())
}

/// One item, by slug or by id.
///
/// Both, in one query, because a link with a bare id has to keep working: every
/// item shared before slugs existed used one, and so does anything that grabbed a
/// URL from the API. Slug is matched first so a item can never be shadowed by
/// another's id.
pub async fn get(slug_or_id: &str, voter: Option<&str>) -> anyhow::Result<Option<Item>> {
    let sql = format!(
        "{ITEM_SELECT} WHERE s.slug = ?1 OR s.id = ?1 \
         ORDER BY CASE WHEN s.slug = ?1 THEN 0 ELSE 1 END \
         LIMIT 1"
    );
    let rows: Vec<ItemRow> = client().query(&sql, vec![json!(slug_or_id)]).await?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let mut item = Item::from(row);

    if let Some(voter) = voter {
        #[derive(Deserialize)]
        struct Hit {
            #[allow(dead_code)]
            x: i64,
        }
        // Keyed on the item's real id, not on whatever form the URL used.
        let hit: Vec<Hit> = client()
            .query(
                "SELECT 1 AS x FROM likes WHERE item_id = ?1 AND voter_id = ?2",
                vec![json!(item.id), json!(voter)],
            )
            .await?;
        item.liked_by_me = !hit.is_empty();
    }
    Ok(Some(item))
}

/// The items either side of this one in the gallery's default order, as
/// `(newer, older)`.
///
/// Slugs, never ids: every link in the UI is built from a slug, and returning an
/// id here would quietly produce the one URL shape nothing should be indexing.
/// `COALESCE` because `items.slug` is nullable (migration 0008) — the same
/// id-as-fallback `Item::from` and `sitemap_items` already apply, so a row written
/// before slugs existed yields a working link instead of a missing neighbour.
///
/// The ordering *and* the tiebreak are `list_public(Sort::Newest)`'s exactly —
/// `created_at DESC, id DESC` — so stepping between detail pages walks the same
/// sequence as the grid the visitor arrived from. Dropping the `id` tiebreak
/// would look identical until two items shared a `created_at`, and then it would
/// skip one, in one direction only.
///
/// One round trip: the anchor row is a CTE that both scalar subqueries join
/// against. Checked against live D1 before being relied on, per CLAUDE.md — a
/// CTE referenced from inside a scalar subquery does work there, and an unknown
/// or hidden slug produces an empty `anchor` and therefore one row of two NULLs
/// rather than an error, which is the right answer: a item that is not in the
/// gallery has nothing either side of it.
pub async fn neighbours(slug_or_id: &str) -> anyhow::Result<(Option<String>, Option<String>)> {
    #[derive(Deserialize)]
    struct NeighbourRow {
        newer: Option<String>,
        older: Option<String>,
    }

    let rows: Vec<NeighbourRow> = client()
        .query(
            "WITH anchor AS ( \
                 SELECT created_at, id FROM items \
                 WHERE (slug = ?1 OR id = ?1) AND is_public = 1 LIMIT 1 \
             ) \
             SELECT \
                 (SELECT COALESCE(items.slug, items.id) FROM items, anchor \
                  WHERE items.is_public = 1 \
                    AND (items.created_at > anchor.created_at \
                         OR (items.created_at = anchor.created_at AND items.id > anchor.id)) \
                  ORDER BY items.created_at ASC, items.id ASC LIMIT 1) AS newer, \
                 (SELECT COALESCE(items.slug, items.id) FROM items, anchor \
                  WHERE items.is_public = 1 \
                    AND (items.created_at < anchor.created_at \
                         OR (items.created_at = anchor.created_at AND items.id < anchor.id)) \
                  ORDER BY items.created_at DESC, items.id DESC LIMIT 1) AS older",
            vec![json!(slug_or_id)],
        )
        .await?;

    Ok(rows
        .into_iter()
        .next()
        .map(|r| (r.newer, r.older))
        .unwrap_or((None, None)))
}

/// A slug for `title` that nothing else is using.
///
/// Appends -2, -3, ... on collision. Racy in principle -- two simultaneous
/// uploads of the same title could both see a free slug -- but the UNIQUE index
/// is the real guard, and the insert failing is preferable to two items quietly
/// sharing a URL. Uploads are not remotely a hot path.
pub async fn unique_slug(title: &str, id: &str) -> String {
    let base = slugify(title);
    if base.is_empty() {
        return id.to_string();
    }

    #[derive(Deserialize)]
    struct Row {
        slug: String,
    }
    let taken: Vec<Row> = client()
        .query(
            "SELECT slug FROM items WHERE slug = ?1 OR slug LIKE ?1 || '-%'",
            vec![json!(base)],
        )
        .await
        .unwrap_or_default();
    let taken: std::collections::HashSet<String> = taken.into_iter().map(|r| r.slug).collect();

    if !taken.contains(&base) {
        return base;
    }
    // Bounded: past a few hundred items with one title, the id is a better URL
    // than "itemified-417" anyway.
    for n in 2..500 {
        let candidate = format!("{base}-{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    id.to_string()
}

/// Every `source_url` on record, for `mirror` to diff a sitemap against.
///
/// The whole column in one query rather than a lookup per link: one round trip
/// an hour beats a hundred, and a set of a few thousand short strings costs
/// nothing to hold. Hidden items are included on purpose -- something pulled
/// in once and then hidden must not be pulled in again on the next pass.
pub async fn known_source_urls() -> anyhow::Result<HashSet<String>> {
    #[derive(Deserialize)]
    struct Row {
        source_url: String,
    }
    let rows: Vec<Row> = client()
        .query(
            "SELECT source_url FROM items WHERE source_url IS NOT NULL",
            vec![],
        )
        .await?;
    Ok(rows.into_iter().map(|r| r.source_url).collect())
}

/// Exact-duplicate lookup by content hash, across every item regardless of
/// `is_public` -- a hidden item re-uploaded byte-for-identical is still the
/// same file, and catching that closes an easy moderation-evasion loop
/// (hide it, then just upload it again).
pub async fn find_by_hash(hash: &str) -> anyhow::Result<Option<Item>> {
    let sql = format!("{ITEM_SELECT} WHERE s.content_hash = ?1 LIMIT 1");
    let rows: Vec<ItemRow> = client().query(&sql, vec![json!(hash)]).await?;
    Ok(rows.into_iter().next().map(Item::from))
}

pub struct NewItem<'a> {
    pub id: &'a str,
    pub slug: &'a str,
    pub title: &'a str,
    pub kind: MediaKind,
    pub orig_url: &'a str,
    pub thumb_url: &'a str,
    /// Videos only; see `Item::poster_url`.
    pub poster_url: Option<&'a str>,
    pub width: u32,
    pub height: u32,
    /// Seconds, videos only, as the browser reported it.
    pub duration: Option<f64>,
    /// SHA-256 of the decoded pixels (stills) or the file (GIFs, videos), for
    /// exact-duplicate detection.
    pub content_hash: &'a str,
    /// The page this was imported from, for a link-import; `None` for a file.
    pub source_url: Option<&'a str>,
    /// Who uploaded this, if they were signed in. `None` for an anonymous
    /// upload — still fully supported; accounts are additive, not a gate.
    pub uploader_id: Option<&'a str>,
    /// Display info for the same uploader, so the row handed straight back to
    /// the caller after insert already shows attribution correctly, without
    /// an extra round trip to re-fetch what the caller already had in hand.
    pub uploader: Option<Uploader>,
}

pub async fn insert(new: NewItem<'_>) -> anyhow::Result<Item> {
    let created_at = chrono::Utc::now().to_rfc3339();

    // item_score/nsfw_score are written as literal 0: the columns are NOT NULL
    // from migration 0001 and nothing scores an upload. They are left in the
    // schema rather than dropped so that scores from any future screening have
    // somewhere to land. Read as "not assessed" -- nothing displays them.
    client()
        .exec(
            "INSERT INTO items (id, slug, title, kind, orig_url, thumb_url, poster_url, \
                                width, height, duration, item_score, nsfw_score, \
                                created_at, is_public, reports, uploader_id, content_hash, \
                                source_url) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,0,?11,1,0,?12,?13,?14)",
            vec![
                json!(new.id),
                json!(new.slug),
                json!(new.title),
                json!(new.kind.as_str()),
                json!(new.orig_url),
                json!(new.thumb_url),
                json!(new.poster_url),
                json!(new.width),
                json!(new.height),
                json!(new.duration),
                json!(created_at),
                json!(new.uploader_id),
                json!(new.content_hash),
                json!(new.source_url),
            ],
        )
        .await?;

    Ok(Item {
        id: new.id.to_string(),
        slug: new.slug.to_string(),
        title: new.title.to_string(),
        kind: new.kind,
        orig_url: new.orig_url.to_string(),
        thumb_url: new.thumb_url.to_string(),
        poster_url: new.poster_url.map(str::to_string),
        width: new.width,
        height: new.height,
        duration: new.duration,
        source_url: new.source_url.map(str::to_string),
        created_at,
        is_public: true,
        reports: 0,
        likes: 0,
        liked_by_me: false,
        uploader: new.uploader,
    })
}

/// Record a report and recompute `items.reports` from `COUNT(*)`, the same
/// self-healing pattern as `items.likes` — a drift-proof counter beats an
/// incremented one given D1's lack of cross-call transactions (see `d1.rs`).
///
/// One report per voter per item (`reports`'s primary key): a repeat report
/// from the same voter is silently a no-op, so a single visitor cannot force
/// auto-hide alone by resubmitting. `voter` is a signed-in user's id, which is
/// what makes that key mean anything — while it was a self-issued cookie, three
/// clears of `item_voter` were three distinct voters and the key bought nothing.
pub async fn report(
    item_id: &str,
    voter: &str,
    reason: &str,
    message: Option<&str>,
) -> anyhow::Result<()> {
    client()
        .exec(
            "INSERT INTO reports (item_id, voter_id, reason, message, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (item_id, voter_id) DO NOTHING",
            vec![
                json!(item_id),
                json!(voter),
                json!(reason),
                json!(message),
                json!(chrono::Utc::now().to_rfc3339()),
            ],
        )
        .await?;

    client()
        .exec(
            "UPDATE items \
             SET reports = (SELECT COUNT(*) FROM reports WHERE item_id = items.id), \
                 is_public = CASE \
                     WHEN (SELECT COUNT(*) FROM reports WHERE item_id = items.id) >= ?2 \
                     THEN 0 ELSE is_public \
                 END \
             WHERE id = ?1",
            vec![json!(item_id), json!(AUTO_HIDE_REPORTS)],
        )
        .await?;
    Ok(())
}

/// Flag a item. At `AUTO_HIDE_REPORTS` it pulls itself from the gallery — the
/// safety valve that makes auto-publishing survivable without a review queue.
pub const AUTO_HIDE_REPORTS: i64 = 3;

pub async fn set_public(id: &str, public: bool) -> anyhow::Result<()> {
    client()
        .exec(
            "UPDATE items SET is_public = ?2 WHERE id = ?1",
            vec![json!(id), json!(i64::from(public))],
        )
        .await?;
    Ok(())
}

/// Delete a item's row and its reports/likes (both `ON DELETE CASCADE`, though
/// D1's HTTP API executes each statement without a guarantee that
/// `PRAGMA foreign_keys` is on for it, so the cascades are not solely relied
/// on — both child tables are cleared explicitly first).
pub async fn delete_item(id: &str) -> anyhow::Result<()> {
    client()
        .exec("DELETE FROM reports WHERE item_id = ?1", vec![json!(id)])
        .await?;
    client()
        .exec("DELETE FROM likes WHERE item_id = ?1", vec![json!(id)])
        .await?;
    client()
        .exec("DELETE FROM items WHERE id = ?1", vec![json!(id)])
        .await?;
    Ok(())
}

/// Every item with at least one report, each with its full report history —
/// the admin queue's unit of review. Two queries, not N+1: one for the items,
/// one for every report against any of them.
pub async fn flagged_items() -> anyhow::Result<Vec<crate::models::FlaggedItem>> {
    use crate::models::{FlaggedItem, ReportDetail};

    // Hidden items (is_public = 0) as well as reported ones, so an admin who hid
    // something by hand can find it again to unhide or delete it.
    let sql = format!(
        "{ITEM_SELECT} WHERE s.reports > 0 OR s.is_public = 0 \
         ORDER BY s.reports DESC, s.created_at DESC"
    );
    let items: Vec<Item> = client()
        .query::<ItemRow>(&sql, vec![])
        .await?
        .into_iter()
        .map(Item::from)
        .collect();
    if items.is_empty() {
        return Ok(vec![]);
    }

    let holes = (0..items.len())
        .map(|i| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let sql2 =
        format!("SELECT item_id, reason, message, created_at FROM reports WHERE item_id IN ({holes}) ORDER BY created_at DESC");

    #[derive(Deserialize)]
    struct Row {
        item_id: String,
        reason: String,
        message: Option<String>,
        created_at: String,
    }
    let rows: Vec<Row> = client()
        .query(&sql2, items.iter().map(|s| json!(s.id)).collect())
        .await?;

    let mut by_item: std::collections::HashMap<String, Vec<ReportDetail>> =
        std::collections::HashMap::new();
    for r in rows {
        by_item.entry(r.item_id).or_default().push(ReportDetail {
            reason: r.reason,
            message: r.message,
            created_at: r.created_at,
        });
    }

    Ok(items
        .into_iter()
        .map(|item| {
            let reports = by_item.remove(&item.id).unwrap_or_default();
            FlaggedItem { item, reports }
        })
        .collect())
}

pub async fn count_public() -> anyhow::Result<i64> {
    #[derive(Deserialize)]
    struct Count {
        n: i64,
    }
    let rows: Vec<Count> = client()
        .query(
            "SELECT COUNT(*) AS n FROM items WHERE is_public = 1",
            vec![],
        )
        .await?;
    Ok(rows.first().map(|r| r.n).unwrap_or(0))
}

/// Toggle a like and return `(new_count, liked_now)`.
///
/// `voter` is a signed-in user's id — `api::like_item` refuses to call this
/// without one. The column keeps its name because it keeps its old contents:
/// likes recorded against anonymous `item_voter` cookie ids are still here and
/// still counted, since step 3 below recomputes from `COUNT(*)` and never
/// increments. Nothing about this migration moves a total; the only thing those
/// rows have lost is an owner who could un-like them.
///
/// D1 gives no transaction spanning separate HTTP calls (see `d1.rs`), so this
/// cannot be "read whether liked, then branch" the way a local sqlx
/// transaction could. Instead:
///
/// 1. Try to insert the like row, ignoring a conflict, and use `RETURNING` to
///    learn — atomically, in this one statement — whether the insert actually
///    happened. This *is* the read: no separate SELECT needed to know the
///    prior state, so nothing can change between checking and acting.
/// 2. If it didn't happen (the row already existed), delete it instead — the
///    unlike path. If a concurrent request already deleted it in the gap
///    between these two calls, this is a no-op, which is fine: the row is
///    gone either way and that is the outcome we want.
/// 3. Recompute `items.likes` from `COUNT(*)` on the source-of-truth `likes`
///    table, rather than incrementing/decrementing a counter. This makes the
///    counter self-healing: any drift from a request that failed partway
///    through gets corrected by the next successful toggle, rather than
///    compounding.
pub async fn toggle_like(item_id: &str, voter: &str) -> anyhow::Result<(i64, bool)> {
    #[derive(Deserialize)]
    struct ItemIdRow {
        #[allow(dead_code)]
        item_id: String,
    }

    let inserted: Vec<ItemIdRow> = client()
        .query(
            "INSERT INTO likes (item_id, voter_id, created_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (item_id, voter_id) DO NOTHING \
             RETURNING item_id",
            vec![
                json!(item_id),
                json!(voter),
                json!(chrono::Utc::now().to_rfc3339()),
            ],
        )
        .await?;

    let now_liked = if inserted.is_empty() {
        client()
            .exec(
                "DELETE FROM likes WHERE item_id = ?1 AND voter_id = ?2",
                vec![json!(item_id), json!(voter)],
            )
            .await?;
        false
    } else {
        true
    };

    #[derive(Deserialize)]
    struct Likes {
        likes: i64,
    }
    let recomputed: Vec<Likes> = client()
        .query(
            "UPDATE items SET likes = (SELECT COUNT(*) FROM likes WHERE item_id = items.id) \
             WHERE id = ?1 \
             RETURNING likes",
            vec![json!(item_id)],
        )
        .await?;

    let count = recomputed.first().map(|r| r.likes).unwrap_or(0);
    Ok((count, now_liked))
}

const USER_COLS: &str = "id, email, display_name, avatar_url, is_admin";

#[derive(Deserialize)]
struct UserRow {
    id: String,
    email: String,
    display_name: String,
    avatar_url: Option<String>,
    is_admin: i64,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        // email intentionally dropped: User is the client-facing shape (see
        // its doc comment), and email has no reason to leave the server.
        let _ = r.email;
        User {
            id: r.id,
            display_name: r.display_name,
            avatar_url: r.avatar_url,
            is_admin: r.is_admin != 0,
        }
    }
}

pub async fn get_user(id: &str) -> anyhow::Result<Option<User>> {
    let sql = format!("SELECT {USER_COLS} FROM users WHERE id = ?1");
    let rows: Vec<UserRow> = client().query(&sql, vec![json!(id)]).await?;
    Ok(rows.into_iter().next().map(User::from))
}

/// Create or update a user from their Google profile, keyed on `google_sub`
/// (Google's stable subject id — the only field from a Google profile that is
/// guaranteed never to change or be reused for a different person).
///
/// Admin comes from `ADMIN_EMAILS` (comma-separated), matched against the email
/// Google just asserted for this login. Deliberately not a hardcoded address:
/// this repo is public, and a list of admin accounts in source is both a
/// disclosure and a thing nobody can change without a deploy.
///
/// Re-evaluated on every login rather than only on insert, so adding an address
/// to the secret promotes that account the next time they sign in, and removing
/// one demotes them. The env var is the single source of truth -- a manual
/// `UPDATE users SET is_admin = 1` would be silently reverted at their next
/// login, which is the intended behaviour.
/// The identity to publish for an account whose real one should stay private.
///
/// `PSEUDONYMS` holds `email=Display Name` pairs, comma-separated, each with an
/// optional `|avatar-url` after the name. An entry with no avatar publishes none
/// at all, which makes the UI fall back to the initial badge it already draws for
/// contributors who never had a Google picture -- so there is no image to host
/// and nothing to keep alive.
///
/// An env var rather than a users column, for two reasons.
///
/// The first is the same reason `ADMIN_EMAILS` is one: the mapping names a real
/// person's account and this repository is public.
///
/// The second is that a column would not work. `upsert_user` rewrites
/// display_name and avatar_url from Google's response on *every* sign-in, so a
/// row edited by hand looks corrected right up until the next login puts the real
/// name straight back -- the identical trap CLAUDE.md documents for a manual
/// `UPDATE users SET is_admin = 1`. Substituting here, at the point the row is
/// written, is what makes it stick: the real name never enters the database at
/// all, so nothing downstream has to remember to hide it.
///
/// `email` is deliberately NOT substituted. It is never sent to a client --
/// neither `User` nor `Uploader` has the field -- and it is what `is_admin_email`
/// matches on, so replacing it would quietly cost this account its admin rights
/// at the next login.
fn pseudonym_for(email: &str) -> Option<(String, Option<String>)> {
    parse_pseudonym(&std::env::var("PSEUDONYMS").unwrap_or_default(), email)
}

/// The parsing half, split out so it can be tested without touching the
/// environment. `std::env::set_var` mutates process-global state that every
/// other test in this binary shares, and cargo runs them in parallel -- a test
/// that sets `PSEUDONYMS` would be reaching into whatever else happened to be
/// running at the time.
fn parse_pseudonym(raw: &str, email: &str) -> Option<(String, Option<String>)> {
    let target = email.trim().to_ascii_lowercase();
    if target.is_empty() {
        return None;
    }
    for entry in raw.split(',') {
        let Some((who, identity)) = entry.split_once('=') else {
            continue;
        };
        if who.trim().to_ascii_lowercase() != target {
            continue;
        }
        let (name, avatar) = match identity.split_once('|') {
            Some((n, a)) => (n.trim(), a.trim()),
            None => (identity.trim(), ""),
        };
        // A blank name would publish an empty byline, which is worse than
        // publishing the real one because it looks like a bug rather than a
        // choice. Treat it as no entry.
        if name.is_empty() {
            continue;
        }
        return Some((
            name.to_string(),
            (!avatar.is_empty()).then(|| avatar.to_string()),
        ));
    }
    None
}

fn is_admin_email(email: &str) -> bool {
    let email = email.trim().to_ascii_lowercase();
    std::env::var("ADMIN_EMAILS")
        .unwrap_or_default()
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .any(|allowed| !allowed.is_empty() && allowed == email)
}

pub async fn upsert_user(
    google_sub: &str,
    email: &str,
    display_name: &str,
    avatar_url: Option<&str>,
) -> anyhow::Result<User> {
    let id = uuid::Uuid::new_v4().to_string();
    let admin = i64::from(is_admin_email(email));
    // Substituted before the row is built, not after it is read, so the real
    // name and picture never reach the database in the first place. `admin` is
    // computed from the untouched `email` above, which is why signing in under a
    // pseudonym still grants admin.
    let (display_name, avatar_url) = match pseudonym_for(email) {
        Some((name, avatar)) => (name, avatar),
        None => (display_name.to_string(), avatar_url.map(str::to_string)),
    };
    let sql = format!(
        "INSERT INTO users (id, google_sub, email, display_name, avatar_url, is_admin, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?7, ?6) \
         ON CONFLICT (google_sub) DO UPDATE SET \
             email = excluded.email, \
             display_name = excluded.display_name, \
             avatar_url = excluded.avatar_url, \
             is_admin = excluded.is_admin \
         RETURNING {USER_COLS}"
    );
    let rows: Vec<UserRow> = client()
        .query(
            &sql,
            vec![
                json!(id),
                json!(google_sub),
                json!(email),
                json!(display_name),
                json!(avatar_url),
                json!(chrono::Utc::now().to_rfc3339()),
                json!(admin),
            ],
        )
        .await?;

    rows.into_iter()
        .next()
        .map(User::from)
        .ok_or_else(|| anyhow::anyhow!("upsert_user: RETURNING produced no row"))
}

/// Ranked by upload count among accounts with at least one public upload —
/// an account that has never uploaded doesn't clutter a leaderboard entry at
/// zero.
pub async fn leaderboard(limit: i64) -> anyhow::Result<Vec<LeaderboardEntry>> {
    #[derive(Deserialize)]
    struct Row {
        display_name: String,
        avatar_url: Option<String>,
        upload_count: i64,
    }
    let rows: Vec<Row> = client()
        .query(
            "SELECT u.display_name, u.avatar_url, COUNT(*) AS upload_count \
             FROM items s JOIN users u ON u.id = s.uploader_id \
             WHERE s.is_public = 1 \
             GROUP BY u.id \
             ORDER BY upload_count DESC, u.display_name ASC \
             LIMIT ?1",
            vec![json!(limit)],
        )
        .await?;

    Ok(rows
        .into_iter()
        // Redacted too, for the same reason and because leaving it out here
        // would undo the rest: /leaderboard lists exactly the people who upload
        // while signed in, so a full name here is trivially matched back to the
        // first-name-only byline on their items. Ordering still uses the full
        // name from SQL, so the sort is unchanged.
        .map(|r| LeaderboardEntry {
            display_name: public_first_name(&r.display_name),
            avatar_url: r.avatar_url,
            upload_count: r.upload_count,
        })
        .collect())
}

/// The most-liked item uploaded in the last 24 hours, falling back to the
/// most-liked public item overall when nothing new has landed recently — a
/// quiet day shouldn't mean the homepage's featured slot goes empty.
pub async fn item_of_the_day() -> anyhow::Result<Option<Item>> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();

    let recent_sql = format!(
        "{ITEM_SELECT} \
         WHERE s.is_public = 1 AND s.created_at >= ?1 \
         ORDER BY s.likes DESC, s.created_at DESC \
         LIMIT 1"
    );
    let recent: Vec<ItemRow> = client().query(&recent_sql, vec![json!(cutoff)]).await?;
    if let Some(row) = recent.into_iter().next() {
        return Ok(Some(Item::from(row)));
    }

    let fallback_sql = format!(
        "{ITEM_SELECT} WHERE s.is_public = 1 ORDER BY s.likes DESC, s.created_at DESC LIMIT 1"
    );
    let fallback: Vec<ItemRow> = client().query(&fallback_sql, vec![]).await?;
    Ok(fallback.into_iter().next().map(Item::from))
}

/// Lowercase, alphanumerics kept, every other run collapsed to one dash, no
/// leading or trailing dash. Empty for a title with nothing sluggable in it --
/// callers decide the fallback, which for a item is its id.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true; // swallow a leading dash
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// The shortest text the trigram tokeniser can index. A one- or two-character
/// term produces no trigrams at all and matches nothing -- silently, with no
/// error -- so those queries take the LIKE path instead.
const MIN_TRIGRAM: usize = 3;

/// How many results the fuzzy fallback may return. Far below `PAGE_SIZE`: past
/// the first few, a row is only there because it shares an incidental trigram
/// with the query, and a page of those reads as broken rather than helpful.
const FUZZY_LIMIT: i64 = 8;

/// Wraps a user's term as an FTS5 phrase. Unavoidable, not defensive: FTS5
/// reads `-` as NOT, so an unquoted "capri-item" fails outright with
/// `no such column: item`. A search box is free text, not a query language.
fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// Every 3-character window of `s`, lowercased -- the same units the trigram
/// index stores, so OR-ing them scores a row by how much of the query it
/// actually contains. This is what makes a misspelling still find its item.
fn trigrams(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.to_lowercase().chars().collect();
    if chars.len() < MIN_TRIGRAM {
        return vec![];
    }
    chars
        .windows(MIN_TRIGRAM)
        .map(|w| w.iter().collect::<String>())
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// Search over titles *and* tag names, via the trigram `items_search` index.
///
/// Two precise passes and one forgiving one, because a single query cannot be
/// both and a visitor typing into a box has no way to tell us which they meant:
///
/// 1. **Exact-ish:** every term must appear somewhere in the title or tags.
///    Trigram matching means a term hits *inside* a word, not just at its start,
///    so "versary" finds Geekgallery and "apri" finds Capri-Item -- on a site
///    of wordplay titles, a match inside a word is the common case,
///    not an edge case. Terms too short to have trigrams are ANDed in as LIKE
///    conditions in the same query, so "dy item" still means both.
/// 2. **Fuzzy**, only if the first pass found nothing: OR the query's trigrams
///    and let bm25 rank by how much of the query each item actually contains, so
///    one wrong keystroke lands on the right item instead of an empty page.
///
/// Capped tighter than a normal page: this tier is a "did you mean", and every
/// row past the first few shares a trigram or two with the query by accident.
///
/// Ordered by relevance, not recency.
pub async fn search_items(query: &str, voter: Option<&str>) -> anyhow::Result<Vec<Item>> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }

    // Split by what the index can actually answer. Sending a 1-2 character term
    // to a trigram index matches nothing at all -- silently, with no error -- so
    // those have to be asked a different way, in the same query rather than
    // instead of it. Getting this wrong turned "dy item" from one exact hit into
    // the entire gallery, because the whole query fell through to fuzzy.
    let (long, short): (Vec<&str>, Vec<&str>) =
        terms.iter().partition(|t| t.chars().count() >= MIN_TRIGRAM);

    let mut where_parts = vec!["s.is_public = 1".to_string()];
    let mut params: Vec<serde_json::Value> = vec![];

    if !long.is_empty() {
        params.push(json!(long
            .iter()
            .map(|t| fts_phrase(t))
            .collect::<Vec<_>>()
            .join(" ")));
        where_parts.push(format!("items_search MATCH ?{}", params.len()));
    }
    for term in &short {
        // `%` and `_` escaped, or a visitor typing "100%" matches every item.
        params.push(json!(format!(
            "%{}%",
            term.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )));
        let i = params.len();
        where_parts.push(format!(
            "(items_search.title LIKE ?{i} ESCAPE '\\' OR items_search.tags LIKE ?{i} ESCAPE '\\')"
        ));
    }

    // Only rank by relevance when there is a MATCH to rank by: bm25 is
    // meaningless for a pure-LIKE query, and `ORDER BY rank` without a MATCH is
    // an error rather than a no-op.
    let order = if long.is_empty() {
        "s.created_at DESC"
    } else {
        "rank"
    };
    params.push(json!(PAGE_SIZE));
    let sql = format!(
        "{ITEM_SELECT} \
         JOIN items_search ON items_search.rowid = s.rowid \
         WHERE {} \
         ORDER BY {order} \
         LIMIT ?{}",
        where_parts.join(" AND "),
        params.len(),
    );

    let mut rows: Vec<ItemRow> = client().query(&sql, params).await?;

    // Fuzzy fallback. Deliberately drops the AND between terms as well as the
    // requirement to match at all -- a misspelling is usually one term of
    // several, and insisting on the others would keep the result set empty.
    if rows.is_empty() {
        let grams: Vec<String> = terms.iter().flat_map(|t| trigrams(t)).collect();
        if !grams.is_empty() {
            let expr = grams
                .iter()
                .map(|g| fts_phrase(g))
                .collect::<Vec<_>>()
                .join(" OR ");
            let sql = format!(
                "{ITEM_SELECT} \
                 JOIN items_search ON items_search.rowid = s.rowid \
                 WHERE s.is_public = 1 AND items_search MATCH ?1 \
                 ORDER BY rank \
                 LIMIT ?2"
            );
            rows = client()
                .query(&sql, vec![json!(expr), json!(FUZZY_LIMIT)])
                .await?;
        }
    }

    let mut items: Vec<Item> = rows.into_iter().map(Item::from).collect();
    mark_liked(&mut items, voter).await?;
    Ok(items)
}

/// A lightweight row for `sitemap.xml` -- only what the sitemap's image
/// extension needs, not a full `Item` (no likes/tags/uploader join).
pub struct SitemapItem {
    /// The slug, since that is what the sitemap should advertise -- an id URL
    /// works but is the form nobody should be indexing.
    pub id: String,
    pub title: String,
    pub kind: MediaKind,
    pub orig_url: String,
    pub thumb_url: String,
    pub poster_url: Option<String>,
    pub duration: Option<f64>,
    pub created_at: String,
}

/// Every public item, newest first, for the sitemap's `<image:image>`
/// extension. Capped, not paged: the sitemap protocol has no cursor concept
/// within one file -- a sitemap *index* (multiple files) is the real answer
/// once a site outgrows one, not something worth building ahead of actually
/// needing it.
pub async fn sitemap_items(limit: i64) -> anyhow::Result<Vec<SitemapItem>> {
    #[derive(Deserialize)]
    struct Row {
        id: String,
        slug: Option<String>,
        title: String,
        #[serde(default)]
        kind: Option<String>,
        orig_url: String,
        thumb_url: String,
        #[serde(default)]
        poster_url: Option<String>,
        #[serde(default)]
        duration: Option<f64>,
        created_at: String,
    }
    let rows: Vec<Row> = client()
        .query(
            "SELECT id, slug, title, kind, orig_url, thumb_url, poster_url, duration, created_at \
             FROM items WHERE is_public = 1 ORDER BY created_at DESC LIMIT ?1",
            vec![json!(limit)],
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| SitemapItem {
            id: r.slug.clone().unwrap_or_else(|| r.id.clone()),
            title: r.title,
            kind: MediaKind::from_str_or_default(r.kind.as_deref().unwrap_or("image")),
            orig_url: r.orig_url,
            thumb_url: r.thumb_url,
            poster_url: r.poster_url,
            duration: r.duration,
            created_at: r.created_at,
        })
        .collect())
}

/// How many public items of each kind there are, for the home page's
/// structured data and the llms.txt summary. One query, three numbers.
pub async fn counts_by_kind() -> anyhow::Result<(i64, i64, i64)> {
    #[derive(Deserialize)]
    struct Row {
        kind: Option<String>,
        n: i64,
    }
    let rows: Vec<Row> = client()
        .query(
            "SELECT kind, COUNT(*) AS n FROM items WHERE is_public = 1 GROUP BY kind",
            vec![],
        )
        .await?;
    let mut out = (0, 0, 0);
    for r in rows {
        match MediaKind::from_str_or_default(r.kind.as_deref().unwrap_or("image")) {
            MediaKind::Image => out.0 += r.n,
            MediaKind::Gif => out.1 += r.n,
            MediaKind::Video => out.2 += r.n,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented names only, here and in the pseudonym tests below. A test fixture
    // is source code: putting a real contributor's name in one publishes it to
    // anyone who reads the repository, which is the exact thing the function
    // under test exists to prevent.
    #[test]
    fn only_the_first_name_is_published() {
        assert_eq!(public_first_name("Ada Lovelace"), "Ada");
        assert_eq!(public_first_name("Mary Jane Watson"), "Mary");
        // Hyphenated given names are one word and survive whole.
        assert_eq!(public_first_name("Mary-Jane Watson"), "Mary-Jane");
        // Google pads and double-spaces more often than you would hope.
        assert_eq!(public_first_name("  Ada   Lovelace  "), "Ada");
    }

    /// Never returns less than it was given when there is no surname to drop.
    /// A mononym is the whole name, and an empty name has to stay empty rather
    /// than have one invented for it.
    #[test]
    fn names_with_nothing_to_redact_are_untouched() {
        assert_eq!(public_first_name("Madonna"), "Madonna");
        assert_eq!(public_first_name("さくら"), "さくら");
        assert_eq!(public_first_name(""), "");
        assert_eq!(public_first_name("   "), "   ");
    }

    #[test]
    fn pseudonym_replaces_name_and_drops_the_picture() {
        let raw = "someone@example.com=itemonymous";
        assert_eq!(
            parse_pseudonym(raw, "someone@example.com"),
            Some(("itemonymous".to_string(), None))
        );
    }

    #[test]
    fn pseudonym_can_carry_its_own_avatar() {
        let raw = "a@b.com=Nobody|https://example.com/x.png";
        assert_eq!(
            parse_pseudonym(raw, "a@b.com"),
            Some((
                "Nobody".to_string(),
                Some("https://example.com/x.png".to_string())
            ))
        );
    }

    /// Google reports the address in whatever case the account was created with,
    /// so matching has to be case- and whitespace-insensitive on both sides or
    /// the substitution silently stops applying.
    #[test]
    fn pseudonym_matching_ignores_case_and_padding() {
        let raw = "  Someone@Example.COM = Quiet Item  ";
        assert_eq!(
            parse_pseudonym(raw, "someone@example.com"),
            Some(("Quiet Item".to_string(), None))
        );
    }

    #[test]
    fn picks_the_right_entry_out_of_several() {
        let raw = "x@y.com=First,target@z.com=Second,q@r.com=Third";
        assert_eq!(
            parse_pseudonym(raw, "target@z.com"),
            Some(("Second".to_string(), None))
        );
    }

    /// Everything that is not a match falls through to None, which publishes the
    /// real Google name. That is the safe direction for a bug in *this* function
    /// to fail, but it means the entry has to be right -- hence the cases.
    #[test]
    fn anything_unmatched_or_malformed_yields_no_substitution() {
        for (raw, email) in [
            ("", "a@b.com"),                 // unset
            ("a@b.com=Name", "other@b.com"), // different person
            ("a@b.com", "a@b.com"),          // no '=' at all
            ("a@b.com=", "a@b.com"),         // blank name
            ("a@b.com=   ", "a@b.com"),      // whitespace-only name
            ("a@b.com=Name", ""),            // no email to match
        ] {
            assert_eq!(
                parse_pseudonym(raw, email),
                None,
                "expected no substitution for raw={raw:?} email={email:?}"
            );
        }
    }

    #[test]
    fn slugs_are_url_safe() {
        assert_eq!(slugify("Item-a-Versary"), "item-a-versary");
        assert_eq!(slugify("Item of Man"), "item-of-man");
        assert_eq!(slugify("  Itemified!!  "), "itemified");
        assert_eq!(slugify("Item   of    spaces"), "item-of-spaces");
    }

    /// Empty rather than a placeholder, so the caller can fall back to the id.
    /// A title of pure punctuation or non-Latin script has nothing to slug.
    #[test]
    fn unsluggable_titles_are_empty() {
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("😭😭😭"), "");
    }

    /// Trigrams are what the fuzzy fallback ORs together, so a term shorter than
    /// one must produce nothing rather than a partial gram that matches
    /// everything.
    #[test]
    fn trigrams_need_three_characters() {
        assert!(trigrams("dy").is_empty());
        assert!(trigrams("").is_empty());
        assert_eq!(trigrams("geekie"), ["gee", "eek", "eki", "kie"]);
    }

    /// FTS5 reads a bare hyphen as NOT and errors with `no such column`, and an
    /// unescaped quote ends the phrase early. Both must survive quoting.
    #[test]
    fn fts_phrases_neutralise_operators() {
        assert_eq!(fts_phrase("capri-item"), "\"capri-item\"");
        assert_eq!(fts_phrase("say \"item\""), "\"say \"\"item\"\"\"");
    }

    #[test]
    fn admin_emails_match_case_insensitively_and_exactly() {
        // SAFETY: single-threaded test, and the var is read only here.
        unsafe { std::env::set_var("ADMIN_EMAILS", "Boss@Example.com, other@example.com") };
        assert!(is_admin_email("boss@example.com"));
        assert!(is_admin_email("  OTHER@EXAMPLE.COM  "));
        assert!(!is_admin_email("boss@example.com.evil.test"));
        assert!(!is_admin_email("nobody@example.com"));
        unsafe { std::env::remove_var("ADMIN_EMAILS") };
        assert!(!is_admin_email("boss@example.com"));
    }
}
