//! Every `<a>` pointing at an Axum route must opt out of the client-side router.
//!
//! `leptos_router` intercepts same-origin anchor clicks and resolves them against
//! its own route table unless the anchor carries `download` or `rel="external"`
//! (`leptos_router`'s `location/mod.rs`). The Axum routes in `main.rs` are not in
//! that table, so an intercepted click matches nothing and renders the 404 page.
//! The endpoint is fine; the button looks broken. It is invisible in code review,
//! compiles cleanly, and only shows up as "there are just too many 404s for no
//! reason" -- which is how it was actually reported, after it had happened to the
//! sign-in link, the download button, and the admin browser link.
//!
//! So the invariant is checked rather than remembered. The route list is parsed out
//! of `main.rs` instead of being copied here, which means a newly added Axum route
//! is covered by this test the moment it is registered -- the failure mode of a
//! hardcoded list is that it silently stops covering the thing you just added.
//!
//! This reads source text, which is unusual for a test and worth being honest
//! about: it can only see anchors whose href contains a literal. That is every one
//! of them today. An href built entirely at runtime would pass unexamined, and the
//! structural guard for that case is `components::sign_in::SignInLink` -- one
//! component that owns the attribute for the one link that was built that way.

use std::fs;
use std::path::{Path, PathBuf};

/// Route patterns registered on the Axum router, e.g. `/item/{id}/download`.
fn axum_routes(main_rs: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = main_rs;
    while let Some(i) = rest.find(".route(") {
        rest = &rest[i + ".route(".len()..];
        // The path is the first string literal in the call. Whitespace and a
        // newline between `.route(` and it are normal rustfmt output.
        let Some(start) = rest.find('"') else { break };
        let Some(len) = rest[start + 1..].find('"') else {
            break;
        };
        let path = &rest[start + 1..start + 1 + len];
        if path.starts_with('/') {
            out.push(path.to_string());
        }
    }
    out
}

/// Every `.rs` file under `src/`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("src/ is readable").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether an href literal would be served by this route pattern.
///
/// Segment by segment rather than by prefix, because `/item/{id}/download` and
/// `/item/{id}` share one: a prefix test would demand `rel="external"` on every
/// link to a item's page, which are the links that must *not* have it.
///
/// A `{...}` segment in either matches anything -- in the route because that is
/// what a path parameter means, and in the href because the literal there is a
/// `format!` template whose `{}` is the value being substituted in.
fn route_serves(route: &str, href: &str) -> bool {
    // A query string is the caller's, not part of the path.
    let href = href.split('?').next().unwrap_or(href);
    let r: Vec<&str> = route.trim_end_matches('/').split('/').collect();
    let h: Vec<&str> = href.trim_end_matches('/').split('/').collect();
    r.len() == h.len()
        && r.iter()
            .zip(&h)
            .all(|(rs, hs)| rs.starts_with('{') || hs.starts_with('{') || rs == hs)
}

/// Byte offsets of every `<a` that opens an anchor tag.
///
/// Not `find("<a ")`: an anchor with more than two attributes is formatted with
/// each on its own line, so it starts `<a\n` and a search for `"<a "` walked
/// straight past it. That is most of the anchors in this project, including the
/// download button -- which is how the first version of this test passed while the
/// attribute it exists to require was deliberately deleted.
fn anchor_offsets(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = src[from..].find("<a") {
        let at = from + i;
        from = at + 2;
        // `<article>` and `<a11y-ish>` are not anchors; `<a>` with no attributes
        // has nothing to check but is still an anchor.
        match bytes.get(at + 2) {
            Some(c) if c.is_ascii_whitespace() || *c == b'>' => out.push(at),
            _ => {}
        }
    }
    out
}

/// The tag with every string literal's contents blanked out.
///
/// Needed because the attribute test cannot be a substring search over the raw
/// tag: `href=format!("/item/{}/download", ...)` contains the word "download", so a
/// naive `tag.contains("download")` reported the attribute present on the one
/// anchor whose href happens to mention it -- and the test passed after the
/// attribute was deliberately deleted. Caught by breaking it on purpose, which is
/// the only way to find out whether a guard guards anything.
fn without_string_contents(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut in_string = false;
    for c in tag.chars() {
        if c == '"' {
            in_string = !in_string;
            out.push('"');
        } else if !in_string {
            out.push(c);
        }
    }
    out
}

/// Whether this tag carries `download` as an attribute of its own.
fn has_download_attr(tag: &str) -> bool {
    without_string_contents(tag)
        .split(|c: char| c.is_whitespace() || c == '=')
        .any(|t| t == "download")
}

/// The href literal inside an `<a>` tag, if it has one.
fn href_of(tag: &str) -> Option<&str> {
    let after = &tag[tag.find("href=")? + "href=".len()..];
    // `href="/x"`, or `href=format!("/x/{}", y)` -- in both the target is the
    // next string literal, and in both it starts with `/` when it is a path.
    let start = after.find('"')? + 1;
    let len = after[start..].find('"')?;
    let value = &after[start..start + len];
    value.starts_with('/').then_some(value)
}

#[test]
fn anchors_to_axum_routes_opt_out_of_the_router() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let routes = axum_routes(&fs::read_to_string(root.join("src/main.rs")).expect("main.rs"));
    assert!(
        routes.len() > 5,
        "parsed only {} routes out of main.rs -- the parser has stopped working, \
         and a test that finds no routes cannot fail for the right reason",
        routes.len(),
    );

    let mut files = vec![];
    sources(&root.join("src"), &mut files);

    let mut offenders = vec![];
    for file in files {
        let src = fs::read_to_string(&file).expect("source is readable");
        // Attributes routinely span lines, so each tag runs to its closing `>`
        // rather than to the end of a line.
        for tag_start in anchor_offsets(&src) {
            let Some(end) = src[tag_start..].find('>') else {
                break;
            };
            let tag = &src[tag_start..tag_start + end];

            let Some(href) = href_of(tag) else { continue };
            if !routes.iter().any(|r| route_serves(r, href)) {
                continue;
            }
            if tag.contains("rel=\"external\"") || has_download_attr(tag) {
                continue;
            }
            let line = src[..tag_start].matches('\n').count() + 1;
            offenders.push(format!(
                "{}:{line} -> href {href} is an Axum route and has neither \
                 rel=\"external\" nor download, so clicking it renders the 404 page",
                file.strip_prefix(root).unwrap_or(&file).display(),
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "anchors that the router will intercept and 404:\n  {}",
        offenders.join("\n  "),
    );
}

#[test]
fn multi_line_anchors_are_found() {
    // rustfmt puts every attribute on its own line once there are a few, which is
    // how most anchors here are written.
    let src = "view! {\n    <a\n        class=\"x\"\n        href=\"/y\"\n    >\n}";
    assert_eq!(anchor_offsets(src).len(), 1, "missed a multi-line anchor");
    // Not every tag that starts with the letter a.
    assert!(anchor_offsets("<article class=\"x\">").is_empty());
    assert_eq!(anchor_offsets("<a>text</a>").len(), 1);
}

#[test]
fn the_download_attribute_is_told_apart_from_the_word_in_an_href() {
    assert!(has_download_attr(
        r#"<a download="" href=format!("/item/{}", s.slug)"#
    ));
    assert!(has_download_attr(r#"<a download href="/x""#));
    // The case that made the first version of this test useless.
    assert!(!has_download_attr(
        r#"<a class="icon-btn" href=format!("/item/{}/download", s.slug)"#
    ));
}

#[test]
fn route_matching_is_by_segment_not_by_prefix() {
    // The distinction the whole test rests on: these two share a prefix, and
    // only one of them is an Axum route.
    assert!(route_serves("/item/{id}/download", "/item/{}/download"));
    assert!(route_serves(
        "/item/{id}/download",
        "/item/item-in-a-box/download"
    ));
    assert!(!route_serves("/item/{id}/download", "/item/item-in-a-box"));
    assert!(route_serves(
        "/auth/google/login",
        "/auth/google/login?return_to=/"
    ));
    assert!(!route_serves("/auth/google/login", "/auth/logout"));
}
