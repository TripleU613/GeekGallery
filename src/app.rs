use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, Meta, MetaTags, Title};
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::{path, ParamSegment, SsrMode, StaticSegment};

use crate::api::current_user;
use crate::components::admin::Admin;
use crate::components::detail::ItemDetail;
use crate::components::gallery::Gallery;
use crate::components::icon::{
    Ico, LuCircleAlert, LuCirclePlus, LuGithub, LuImage, LuLogOut, LuTrophy, LuUserRound,
};
use crate::components::leaderboard::Leaderboard;
use crate::components::legal::{About, Dmca, Privacy, Terms};
use crate::components::progress::{Loading, TopProgress};
use crate::components::search::SearchPage;
use crate::components::search_box::SearchBox;
use crate::components::sign_in::SignInLink;
use crate::components::sort_chips::{GalleryView, SortCtx};
use crate::components::upload::Upload;

/// Cloudflare Web Analytics beacon.
///
/// The one piece of third-party script on the site. Cloudflare already records
/// requests, bandwidth, cache hit ratio, threats and top paths at the edge with
/// nothing added -- that is server-side and needs no tag. This adds what the edge
/// cannot see: page views tied to a session, referrers, browser/OS/country
/// breakdowns, and Core Web Vitals (LCP/INP/CLS), which is what shows whether
/// the gallery is fast on real phones.
///
/// Cookieless and env-gated: absent token, absent script. Kept as a token rather
/// than committed, since it identifies the account's analytics site.
fn cf_beacon_token() -> Option<String> {
    // `shell` is server-rendered but compiles for wasm too, so this needs a body
    // in both builds. There is no environment to read in the browser, and the
    // tag is already in the HTML by then.
    #[cfg(feature = "ssr")]
    {
        std::env::var("CF_ANALYTICS_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
    }
    #[cfg(not(feature = "ssr"))]
    None
}

/// The HTML document. Server-side only entry point; `HydrationScripts` injects
/// the wasm loader so the client picks up where SSR left off.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    let f = crate::flavor::get();
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                // The brand art lives wherever the flavor says (its media
                // bucket, or this repo's placeholder under /brand), at fixed
                // names. `SITE_ASSETS_VERSION` is the cache bust: public
                // paths are served through Cloudflare with a day's TTL, so
                // replacing the bytes at an unchanged URL would leave the old
                // icon in tabs for hours.
                <link rel="icon" type="image/png" sizes="32x32" href=f.asset("favicon-32.png")/>
                <link rel="icon" type="image/png" sizes="192x192" href=f.asset("favicon-192.png")/>
                <link rel="apple-touch-icon" href=f.asset("apple-touch-icon.png")/>
                <meta name="theme-color" content=f.theme.bg.clone()/>
                // Installable, and the manifest's `shortcuts` are what put
                // "Upload" one long-press away from the home screen icon
                // (Android) or the taskbar (Chrome desktop). Generated per
                // flavor by `main.rs`, not a static file.
                <link rel="manifest" href="/manifest.webmanifest"/>
                // The palette, as the CSS variables every Tailwind colour
                // utility reads. Before the stylesheet so nothing ever paints
                // with the fallback palette first.
                <style inner_html=f.theme.css()/>
                // The flavor, for the wasm bundle to read back before it
                // hydrates (see `lib.rs`). JSON in a non-script type is inert;
                // `<` is escaped inside it so it can never close the tag.
                <script type="application/json" id=crate::flavor::EMBED_ID inner_html=f.to_embedded_json()/>
                <AutoReload options=options.clone()/>
                // The stylesheet goes FIRST, ahead of HydrationScripts, and the
                // order is the whole point.
                //
                // HydrationScripts emits a `modulepreload` for the JS and a
                // `<link rel="preload" as="fetch">` for the wasm. Measured in
                // production: 8KB of JS and **569KB of wasm** on the wire, versus
                // 7KB for the stylesheet. `as="fetch"` is a High-priority hint,
                // so with this component first the browser opened the 569KB
                // download before it had parsed far enough to discover the CSS
                // link at all -- and then painted the server-rendered HTML
                // unstyled while the wasm monopolised the connection. That is the
                // "it flashes plain HTML for a second on load" report, and it
                // gets worse the slower the network, which is exactly backwards.
                //
                // Nothing about first paint needs the wasm: this route is
                // SsrMode::Async, so the HTML arrives complete and the wasm only
                // makes it interactive. The stylesheet is the one asset the first
                // paint genuinely cannot do without, so it is the one that gets
                // discovered first.
                //
                // Emitted here rather than as a <Stylesheet> inside App: the
                // hashed filename lives in a file next to the binary, so this
                // needs LeptosOptions, which only exists server-side. See
                // `hash-files` in Cargo.toml for why the hash matters.
                <HashedStylesheet options=options.clone() id="leptos"/>
                <HydrationScripts options=options.clone()/>
                <MetaTags/>
                // Last in <head> and deferred, so analytics never delays first
                // paint. `data-cf-beacon` is Cloudflare's own attribute contract.
                {cf_beacon_token()
                    .map(|token| {
                        view! {
                            <script
                                defer
                                src="https://static.cloudflareinsights.com/beacon.min.js"
                                data-cf-beacon=format!("{{\"token\": \"{token}\"}}")
                            />
                        }
                    })}
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Sort lives here, not in Gallery: the desktop header renders the same four
    // filters the mobile in-page row does, and both must drive one value.
    let (view, set_view) = signal(GalleryView::default());
    provide_context(SortCtx { view, set_view });

    // The in-flight counter behind the top bar. Creating a signal and providing
    // context during render is fine; it is writing an existing one that kills
    // hydration, which is why nothing here calls `start`.
    let loading = Loading::provide();
    let f = crate::flavor::get();

    view! {
        <Title text=f.name.clone()/>
        <Meta property="og:site_name" content=f.name.clone()/>
        // Without this Google Images shows the site's stills as small
        // previews only; with it the full-size original is eligible, which is
        // how a meme gets found by picture and not just by caption.
        <Meta name="robots" content="max-image-preview:large"/>
        <crate::seo::SiteJsonLd/>

        // Handing the router a `set_is_routing` is not just a notification: it
        // switches client-side navigation to a transition, so the previous view
        // stays mounted until the new route's resources resolve, and this stays
        // true for exactly that window. That is a real load state rather than a
        // guessed timer -- and it is also why the route-level <Suspense>
        // fallbacks no longer flash on in-app navigation. The bar replaces
        // them there; they still render on a full page load.
        <Router set_is_routing=SignalSetter::map(move |routing: bool| {
            if routing { loading.start() } else { loading.finish() }
        })>
            // One shell, rearranged entirely by CSS. The primary nav exists
            // exactly once in the DOM and becomes either the desktop sidebar's
            // link list or the mobile bottom bar -- rendering two copies would
            // duplicate landmarks, and branching on window.innerWidth in Rust
            // would invite the hydration mismatch that already took down the
            // wasm module once this session.
            <div class="flex min-h-[100dvh] flex-col">
                // Fixed, so per spec it is not a flex item and cannot touch
                // this column's layout. First in source order anyway, because
                // it is first on screen.
                <TopProgress/>
                <Header/>
                <BottomNav/>
                <main class="mx-auto w-full max-w-content flex-1 px-4 pt-[calc(56px+0.75rem)] pb-[calc(58px+env(safe-area-inset-bottom)+1.5rem)] lg:px-8 lg:pt-[calc(60px+1.5rem)] lg:pb-8">
                    <Routes fallback=NotFound>
                    // Async mode on the two data-driven routes: the default
                    // out-of-order streaming flushes <head> before resources
                    // resolve, so og:/twitter: tags and card markup ended up in
                    // a JS-swapped <template> where unfurlers never see them.
                    <Route path=path!("/") view=Gallery ssr=SsrMode::Async/>
                    // `/{noun}/:id`, from the flavor: a gallery of "pics" has
                    // its pages at /pic/<slug>, and a site that renames its
                    // noun keeps every link ever shared by keeping the noun.
                    <Route path=(StaticSegment(crate::flavor::noun_static()), ParamSegment("id")) view=ItemDetail ssr=SsrMode::Async/>
                    // No server data; streaming is fine here.
                    <Route path=path!("/upload") view=Upload/>
                    <Route path=path!("/leaderboard") view=Leaderboard ssr=SsrMode::Async/>
                    <Route path=path!("/search") view=SearchPage ssr=SsrMode::Async/>
                    // Not SsrMode::Async: it carries no SEO-relevant content
                    // and is gated server-side in every fn it calls anyway, so
                    // there is nothing here for out-of-order streaming to leak.
                    <Route path=path!("/admin") view=Admin/>
                        <Route path=path!("/about") view=About/>
                        <Route path=path!("/privacy") view=Privacy/>
                        <Route path=path!("/tos") view=Terms/>
                        <Route path=path!("/dmca") view=Dmca/>
                    </Routes>
                </main>
                <SiteFooter/>
            </div>
        </Router>
    }
}

/// The site's fine print and its siblings: /privacy, /tos, /dmca, /about and
/// a link to each sister site the flavor names.
///
/// On every page, the gallery and the item pages included. It used to be
/// hidden on those two -- they are where people spend their time, and two
/// words of boilerplate under an image grid read as furniture -- but the
/// cross-link changed the arithmetic: a footer nobody sees links nobody
/// anywhere. Under the infinite grid it sits after the sentinel and comes
/// into view when the collection runs out, which is when a visitor is
/// looking for somewhere else to go anyway.
///
/// These are also the site's only links to /privacy and /tos, and Google's
/// OAuth verification requires a reachable privacy policy for the sign-in
/// this app ships. `AccountAction`'s menu carries the same two links for
/// signed-in visitors; the footer stays *in addition* because the signed-out
/// control is a bare sign-in link with no menu behind it -- exactly the
/// audience that needs the policy before they sign in.
#[component]
fn SiteFooter() -> impl IntoView {
    view! {
        // Padded clear of the mobile bottom nav rather than competing with
        // it, and on the same max-w-content column and lg gutter as <main>
        // so it lines up with the content above rather than with the
        // viewport.
        <footer class="px-4 pb-[calc(58px+env(safe-area-inset-bottom)+0.75rem)] pt-2 lg:px-8 lg:pb-6">
            // gap-2 rather than gap-4: the padding below is what separates
            // them now. `py-1.5 px-2` takes each link from a 17px-tall target
            // to 26px without changing how the footer looks -- 11px type has
            // to be reachable with a thumb like everything else, and these
            // two were the only controls on the site under the 24px floor.
            <div class="mx-auto flex max-w-content justify-center gap-2 text-[0.6875rem] text-ink-3/70">
                <A href="/privacy" attr:class="px-2 py-1.5 transition-colors hover:text-ink-2">
                    "Privacy"
                </A>
                <A href="/tos" attr:class="px-2 py-1.5 transition-colors hover:text-ink-2">
                    "Terms"
                </A>
                <A href="/dmca" attr:class="px-2 py-1.5 transition-colors hover:text-ink-2">
                    "DMCA"
                </A>
                <A href="/about" attr:class="px-2 py-1.5 transition-colors hover:text-ink-2">
                    "About"
                </A>
                // The sister sites, if any. Plain anchors: they leave the
                // app, so the router must not intercept them.
                {crate::flavor::get()
                    .sister_sites
                    .iter()
                    .map(|s| {
                        view! {
                            <a
                                href=s.url.clone()
                                rel="external noopener"
                                class="px-2 py-1.5 transition-colors hover:text-ink-2"
                            >
                                {s.name.clone()}
                            </a>
                        }
                    })
                    .collect_view()}
            </div>
        </footer>
    }
}

/// The header. On mobile: brand, a search field and the account action. On
/// desktop it additionally carries the section links and -- on the gallery only
/// -- the sort chips, replacing the sidebar entirely.
#[component]
fn Header() -> impl IntoView {
    view! {
        <header class="fixed inset-x-0 top-0 z-30 h-topbar border-b border-line bg-bg lg:h-topbar-lg">
            // Inner wrapper so the bar's contents line up with the content
            // column below it: the bar stays full-bleed (its border needs to
            // reach the viewport edges) while the brand and the icons sit on the
            // same left/right edges as the grid.
            <div class="mx-auto flex h-full max-w-content items-center gap-3 px-4 lg:gap-4 lg:px-8">
            // The real mark, at 2x for retina. width/height are set so the
            // header does not reflow while it loads, and it is eager rather than
            // lazy because it is the first thing above the fold.
            // `lg:flex-1` here and on the icon group opposite makes the two
            // side regions equal, which is what actually centres the search
            // field: left to size themselves, the 93px brand and the 3-icon
            // group are not the same width, and the field sat 29px left of the
            // page centre. Measured, not eyeballed. Below lg the brand stays
            // `flex-none` so the field can take the rest of a narrow bar.
            <A href="/" attr:class="flex flex-none items-center gap-2 lg:flex-1" attr:aria-label=format!("{}, home", crate::flavor::get().name)>
                <img
                    src=crate::flavor::get().asset("logo.png")
                    alt=crate::flavor::get().name.clone()
                    width=crate::flavor::get().logo_width.to_string()
                    height=crate::flavor::get().logo_height.to_string()
                    decoding="async"
                    class="h-7 w-auto lg:h-8"
                />
                // A square mark gets the name printed beside it; a wordmark
                // already says it and would say it twice.
                {(!crate::flavor::get().logo_has_name).then(|| view! {
                    <span class="text-[1.05rem] font-semibold tracking-tight text-ink">
                        {crate::flavor::get().name.clone()}
                    </span>
                })}
            </A>

            // The search field, between the brand and the account controls at
            // every width. It used to collapse behind a magnifier on desktop
            // and expand on click, which meant the bar held two things that
            // both meant "search" -- the icon and, once open, the field it
            // revealed. The field is its own affordance, so the icon is gone
            // and the state that drove it with it.
            //
            // `flex-1` with a desktop cap and `mx-auto`: the field takes the
            // room between the two fixed side groups, and once the cap binds,
            // the auto margins absorb what is left over and centre it rather
            // than letting one 448px pill drift against the brand.
            <SearchBox/>

            <div class="flex flex-none items-center gap-1 lg:flex-1 lg:justify-end lg:gap-2">
                // Desktop-only section links, as icons alongside the account
                // action. No Gallery entry: the brand is the gallery, which is
                // the default view. Icon-only, so each carries an aria-label
                // and a title -- the label is the accessible name, the title is
                // the hover tooltip.
                <nav class="hidden flex-none items-center gap-1 lg:flex" aria-label="main">
                    <A
                        href="/upload"
                        attr:class="icon-btn"
                        attr:aria-label="Upload"
                        attr:title="Upload"
                    >
                        <Ico icon=LuCirclePlus size=18/>
                    </A>
                    <A
                        href="/leaderboard"
                        attr:class="icon-btn"
                        attr:aria-label="Leaderboard"
                        attr:title="Leaderboard"
                    >
                        <Ico icon=LuTrophy size=18/>
                    </A>
                </nav>

                // The source. An ordinary anchor, not <A>: it is a different
                // origin, so leptos_router leaves it alone -- rel="external" is
                // for same-origin Axum paths and would say nothing here.
                //
                // Visible at every width, unlike the two nav icons above: someone
                // who wants the code is as likely to be on a phone, and this is one
                // 36px control next to the account button rather than a third entry
                // competing for the mobile bottom bar.
                //
                // noopener with a new tab, always: target="_blank" alone hands the
                // opened page a window.opener reference back to this one.
                {(!crate::flavor::get().repo_url.is_empty()).then(|| view! {
                    <a
                        class="icon-btn"
                        href=crate::flavor::get().repo_url.clone()
                        target="_blank"
                        rel="noopener noreferrer"
                        aria-label="Source on GitHub"
                        title="Source on GitHub"
                    >
                        <Ico icon=LuGithub size=18/>
                    </a>
                })}

                <AccountAction/>
                </div>
            </div>
        </header>
    }
}

/// Primary navigation on mobile only: a fixed bottom bar. Desktop uses the
/// header above instead of a sidebar.
#[component]
fn BottomNav() -> impl IntoView {
    view! {
        <nav class="fixed inset-x-0 bottom-0 z-30 flex border-t border-line bg-bg pb-[env(safe-area-inset-bottom)] lg:hidden" aria-label="main">
            // exact, not the default prefix match: every route starts with "/",
            // so without this the Gallery link would be aria-current on every
            // page in the app.
            <A href="/" attr:class="flex min-h-bottomnav flex-1 flex-col items-center justify-center gap-[3px] px-2 py-1 text-ink-3 transition-colors hover:text-ink-2 aria-[current=page]:text-accent" exact=true>
                <Ico icon=LuImage/>
                <span class="text-[0.7rem] aria-[current=page]:font-semibold">"Gallery"</span>
            </A>
            <A href="/upload" attr:class="flex min-h-bottomnav flex-1 flex-col items-center justify-center gap-[3px] px-2 py-1 text-ink-3 transition-colors hover:text-ink-2 aria-[current=page]:text-accent">
                <Ico icon=LuCirclePlus/>
                <span class="text-[0.7rem] aria-[current=page]:font-semibold">"Upload"</span>
            </A>
            <A href="/leaderboard" attr:class="flex min-h-bottomnav flex-1 flex-col items-center justify-center gap-[3px] px-2 py-1 text-ink-3 transition-colors hover:text-ink-2 aria-[current=page]:text-accent">
                <Ico icon=LuTrophy/>
                <span class="text-[0.7rem] aria-[current=page]:font-semibold">"Leaderboard"</span>
            </A>
        </nav>
    }
}

/// Sign-in link, or the signed-in user's avatar plus sign-out. Always the
/// compact icon form now that the header is the only place it appears -- the
/// wider sidebar variant went away with the sidebar.
#[component]
fn AccountAction() -> impl IntoView {
    let user = Resource::new(|| (), |_| current_user());
    // Whether the account menu is open. Signed-out visitors never see it, so
    // this is only ever toggled by the avatar button below.
    let (menu, set_menu) = signal(false);

    view! {
        <Suspense fallback=|| ()>
            {move || {
                user.get()
                    .map(|res| match res {
                        Ok(Some(u)) => {
                            let initial = u
                                .display_name
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_else(|| "?".into());
                            view! {
                                // `relative` so the panel can hang off the
                                // avatar rather than off the header, which would
                                // put it at the far edge of the viewport.
                                <div class="relative flex flex-none items-center">
                                    <button
                                        type="button"
                                        class="icon-btn overflow-hidden"
                                        aria-label="Account"
                                        title="Account"
                                        aria-haspopup="menu"
                                        aria-expanded=move || menu.get().to_string()
                                        on:click=move |_| set_menu.update(|m| *m = !*m)
                                    >
                                        {u
                                            .avatar_url
                                            .clone()
                                            .map(|src| {
                                                view! {
                                                    <img class="h-6 w-6 rounded-full object-cover" src=src alt=""/>
                                                }
                                                    .into_any()
                                            })
                                            .unwrap_or_else(|| {
                                                // No Google picture: their
                                                // initial, so the control still
                                                // reads as "you" rather than as a
                                                // generic person glyph.
                                                view! {
                                                    <span class="flex h-6 w-6 items-center justify-center rounded-full bg-surface-raised text-[0.7rem] font-semibold text-ink-2">
                                                        {initial.clone()}
                                                    </span>
                                                }
                                                    .into_any()
                                            })}
                                    </button>

                                    <Show when=move || menu.get()>
                                        // A click anywhere else closes it. A
                                        // full-viewport transparent layer behind
                                        // the panel does that without a document
                                        // listener to add, remove, and leak.
                                        <div
                                            class="fixed inset-0 z-40"
                                            on:click=move |_| set_menu.set(false)
                                        />
                                        <div
                                            class="absolute right-0 top-full z-50 mt-1 min-w-[11rem] overflow-hidden rounded-lg border border-line bg-surface-raised py-1 shadow-lg"
                                            role="menu"
                                        >
                                            <p class="truncate px-3 py-1.5 text-[0.8rem] text-ink-3">
                                                {u.display_name.clone()}
                                            </p>
                                            <div class="my-1 border-t border-line"/>
                                            {u.is_admin
                                                .then(|| {
                                                    view! {
                                                        <A
                                                            href="/admin"
                                                            attr:class="block px-3 py-2 text-[0.85rem] text-accent hover:bg-surface-hover"
                                                            attr:role="menuitem"
                                                        >
                                                            "Admin"
                                                        </A>
                                                    }
                                                })}
                                            // The fine print, reachable from
                                            // any page including the gallery,
                                            // which no longer carries a footer.
                                            <div class="my-1 border-t border-line"/>
                                            <A
                                                href="/privacy"
                                                attr:class="block px-3 py-2 text-[0.85rem] text-ink-2 transition-colors hover:bg-surface-hover hover:text-ink"
                                                attr:role="menuitem"
                                            >
                                                "Privacy"
                                            </A>
                                            <A
                                                href="/tos"
                                                attr:class="block px-3 py-2 text-[0.85rem] text-ink-2 transition-colors hover:bg-surface-hover hover:text-ink"
                                                attr:role="menuitem"
                                            >
                                                "Terms"
                                            </A>
                                            <div class="my-1 border-t border-line"/>
                                            <form method="post" action="/auth/logout">
                                                <button
                                                    type="submit"
                                                    role="menuitem"
                                                    class="flex w-full items-center gap-2 px-3 py-2 text-left text-[0.85rem] text-ink-2 transition-colors hover:bg-surface-hover hover:text-ink"
                                                >
                                                    <Ico icon=LuLogOut size=15/>
                                                    "Sign out"
                                                </button>
                                            </form>
                                        </div>
                                    </Show>
                                </div>
                            }
                                .into_any()
                        }
                        _ => {
                            // Ok(None) (never signed in, or Google sign-in is
                            // not configured yet) and Err (couldn't reach D1)
                            // get the same link -- nothing useful to tell a
                            // visitor apart between those here.
                            view! {
                                // SignInLink, not a hand-rolled anchor: it owns the
                                // rel="external" that keeps the router from
                                // intercepting this Axum-only path and 404ing on it,
                                // and it reads the return path itself.
                                <SignInLink label="Sign in" attr:class="icon-btn">
                                    <Ico icon=LuUserRound size=18/>
                                </SignInLink>
                            }
                                .into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

/// The site logo as this page's link-preview image, for pages that have no image
/// of their own.
///
/// Deliberately opt-in per page rather than a default in `App`, because
/// leptos_meta resolves a duplicated `og:image` first-set-wins: a default here
/// silently beat `ItemDetail`'s own tag and every item previewed as the site logo.
/// Raw markup in `shell` is worse still -- it cannot be overridden at all, and
/// produced two `og:image` tags where an unfurler picks whichever it likes.
///
/// Absolute, because unfurlers reject a relative `og:image` outright.
#[component]
pub fn SitePreview() -> impl IntoView {
    view! {
        <Meta property="og:image" content=crate::seo::absolute(&crate::flavor::get().asset("logo-large.png"))/>
        <Meta property="og:type" content="website"/>
    }
}

/// Set the response status during SSR.
///
/// Without this a missing page renders the 404 body under HTTP 200 -- a soft
/// 404, which Google treats as a thin duplicate of every other soft 404 on the
/// site rather than as "gone". No-op on the client, where the response has
/// already been sent.
pub fn set_status(code: u16) {
    #[cfg(feature = "ssr")]
    if let Some(resp) = use_context::<leptos_axum::ResponseOptions>() {
        resp.set_status(
            axum::http::StatusCode::from_u16(code).unwrap_or(axum::http::StatusCode::OK),
        );
    }
    #[cfg(not(feature = "ssr"))]
    let _ = code;
}

#[component]
fn NotFound() -> impl IntoView {
    set_status(404);
    let f = crate::flavor::get();

    view! {
        <Title text=format!("no {} here — {}", f.noun, f.name)/>
        // Same shape as the site's other zero states -- icon tile, one line --
        // which is what makes this read as a finished page rather than a stub.
        //
        // It carries a link again. Leaning on the header logo was the argument
        // for leaving it out, and it was wrong: this is the one page on the site
        // that exists because the visitor's next step failed, so the next step is
        // the whole content, and "the chrome has a home link" is true of every
        // dead end ever shipped. The item-missing 404 in `detail.rs` always had
        // one, which made the two 404s disagree about whether that was needed.
        //
        // Written out here rather than reusing EmptyState because that
        // component's line is a <p>: a 404 needs a real <h1>, and dropping the
        // page's only heading is an SEO regression on top of an accessibility
        // one.
        <section class="flex min-h-[46vh] flex-col items-center justify-center gap-3 text-center">
            <span class="inline-flex items-center justify-center rounded-lg border border-line bg-surface p-3 text-ink-3">
                <Ico icon=LuCircleAlert size=28/>
            </span>
            <h1 class="m-0 text-base font-semibold text-ink">{format!("No {} at this address", f.noun)}</h1>
            <p class="m-0 max-w-[40ch] text-[0.875rem] text-ink-2">
                {format!("The link may be old, or the {} may have been taken down.", f.noun)}
            </p>
            // <A>, not a plain anchor: these are Leptos routes, so the router
            // handles them client-side and there is nothing for rel to opt out
            // of. Two ways on, because a 404 has two likely causes -- a stale
            // link (browse) and a mistyped one (search).
            <div class="mt-1 flex flex-wrap items-center justify-center gap-2">
                <A href="/" attr:class="btn">"Back to the collection"</A>
                <A href="/search" attr:class="btn-quiet">{format!("Search for {}", f.a_noun())}</A>
            </div>
            <p class="m-0 text-[0.8125rem] text-ink-3">"404"</p>
        </section>
    }
}
