use crate::app::SitePreview;
use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};
use leptos_router::components::A;

use crate::seo::absolute;

/// The prose article wrapper every page here shares: a readable measure and
/// the heading/list rhythm, once.
const ARTICLE: &str = "max-w-[70ch] pt-5 [&_h2]:mb-2 [&_h2]:mt-7 [&_h2]:text-[1.2rem] [&_h2]:font-semibold [&_li]:my-1 [&_li]:leading-relaxed [&_li]:text-ink-2 [&_p]:leading-relaxed [&_p]:text-ink-2 [&_ul]:my-2 [&_ul]:list-disc [&_ul]:pl-5 [&_a]:text-accent [&_a]:underline [&_a]:underline-offset-2";

/// `/about`. The page that says what this is, in the words a search engine and
/// a person both need: what one of these is here, what can be uploaded, how it
/// is moderated, how to embed one. It exists because the gallery itself
/// carries almost no text -- by design -- and a site with no prose anywhere
/// has nothing for a search result snippet to quote.
///
/// The opening is the flavor's own (`SITE_ABOUT`, one paragraph per `\n`);
/// everything after it is the same for every gallery, with the noun filled
/// in. A flavor with no `SITE_ABOUT` gets one honest paragraph built from its
/// name and tagline rather than a blank.
#[component]
pub fn About() -> impl IntoView {
    let f = crate::flavor::get();
    let (noun, nouns, name) = (f.noun.clone(), f.nouns.clone(), f.name.clone());
    let intro: Vec<String> = if f.about.is_empty() {
        vec![format!(
            "{name} is a gallery of {nouns}: pictures, animated GIFs and short video clips, \
             uploaded by anyone and kept in one place. It is free to browse, free to upload \
             to, and free to share -- every {noun} has a page of its own, a link that unfurls \
             properly in Discord, Slack, iMessage and X, and an embed code that works on any \
             site."
        )]
    } else {
        f.about.clone()
    };
    let belongs = if f.subject.is_empty() {
        format!("{}: the original, its edits and reactions, and everything that grows out of it, as a picture, GIF or clip.", f.noun_title())
    } else {
        format!("Memes, edits, reaction images, GIFs and clips of {}, and the variants that grow out of them.", f.subject)
    };
    view! {
        <Title text=format!("What is {name}? {}", f.tagline)/>
        <SitePreview/>
        <Meta
            name="description"
            content=format!("{} What belongs here, how uploads and moderation work, and how to embed {} anywhere.", f.description, f.a_noun())
        />
        <Link rel="canonical" href=absolute("/about")/>
        // FAQPage structured data: the questions below, verbatim, so a search
        // engine can quote the answers. Hand-built like every JSON-LD here.
        <script type="application/ld+json" inner_html=about_faq_json_ld()/>

        <article class=ARTICLE>
            <h1 class="m-0 mb-1.5 text-[2rem] font-bold tracking-tight">{format!("What is {name}?")}</h1>
            <p class="m-0 mb-6 text-[0.85rem] italic text-ink-3">{f.tagline.clone()}</p>

            {intro.into_iter().map(|p| view! { <p>{p}</p> }).collect_view()}

            <h2>"What belongs here"</h2>
            <ul>
                <li>{belongs}</li>
                <li>"Images up to 12 MB (PNG, JPG, WEBP, GIF) and clips up to 60 MB (MP4, WEBM)."</li>
                <li>"Originals wherever possible: crop out screenshots, browser chrome and app interfaces first."</li>
                <li>"Only things you have the right to share. See the " <A href="/tos">"terms"</A> " and the " <A href="/dmca">"DMCA page"</A> "."</li>
            </ul>

            <h2>"How uploads work"</h2>
            <p>
                "No account is needed. Pick a file, give it a name, and it is live within seconds. \
                Signing in with Google is optional: it puts your first name on your uploads and on \
                the " <A href="/leaderboard">"leaderboard"</A> ", and it lets you like and report. \
                Exact duplicates are refused automatically so the archive stays a collection \
                rather than a pile."
            </p>

            <h2>"How moderation works"</h2>
            <p>
                {format!("Nothing is screened before it appears. Anyone signed in can report {}; three \
                reports hide it from the gallery automatically, and every report lands in a queue a \
                human reviews. Anything illegal, hateful, sexual, violent or harassing is removed. \
                This is a gallery, not a place to attack people.", f.a_noun())}
            </p>

            <h2>"Sharing and embedding"</h2>
            <p>
                {format!("Every {noun} page carries Open Graph and Twitter card tags, schema.org structured \
                data, and an oEmbed endpoint, so a pasted link shows the picture (or plays the clip) \
                wherever it lands. The share button on {} copies the link or opens your share \
                sheet; the embed button copies an iframe snippet. There is also a plain JSON API: ", f.a_noun())}
                <code>"GET /api/v1/items"</code> " lists everything, newest first."
            </p>

            <h2>"Contact"</h2>
            <p>
                {format!("Report the specific {noun} for anything about a specific {noun}. For everything else, \
                including takedowns, see ")} <A href="/dmca">"the DMCA page"</A> "."
            </p>
        </article>
    }
}

/// The About page's questions and answers as `schema.org/FAQPage`, so the
/// same words that are on the page can be quoted under a search result.
fn about_faq_json_ld() -> String {
    let f = crate::flavor::get();
    let host = f
        .origin
        .split_once("://")
        .map(|(_, h)| h)
        .unwrap_or(&f.origin);
    let (name, noun, a_noun) = (&f.name, &f.noun, f.a_noun());
    let qa = [
        (
            format!("What is {name}?"),
            format!("{name}: {} {} Anyone can upload, browse, share and embed at {host}.", f.tagline, f.description),
        ),
        (
            format!("How do I contribute {a_noun}?"),
            format!("Open {host}/upload, pick a PNG, JPG, WEBP or GIF up to 12 MB or an MP4 or WEBM clip up to 60 MB, name it, and it is live within seconds. No account is needed."),
        ),
        (
            format!("Is {name} free?"),
            "Yes. Browsing, uploading, sharing and embedding are free, and there are no ads.".to_string(),
        ),
        (
            format!("How is {name} moderated?"),
            format!("Uploads publish immediately. Signed-in visitors can report {a_noun}; three reports hide it automatically and every report is reviewed by a human. Illegal, hateful, sexual, violent or harassing content is removed."),
        ),
        (
            format!("Can I embed {a_noun} on my own site?"),
            format!("Yes. Every {noun} has an embed button that copies an iframe snippet, plus Open Graph tags and an oEmbed endpoint so a pasted link unfurls in Discord, Slack, iMessage and X."),
        ),
    ];
    let items: Vec<String> = qa
        .iter()
        .map(|(q, a)| {
            format!(
                "{{\"@type\":\"Question\",\"name\":\"{}\",\"acceptedAnswer\":{{\"@type\":\"Answer\",\"text\":\"{}\"}}}}",
                crate::seo::json_escape(q),
                crate::seo::json_escape(a)
            )
        })
        .collect();
    format!(
        "{{\"@context\":\"https://schema.org\",\"@type\":\"FAQPage\",\"mainEntity\":[{}]}}",
        items.join(",")
    )
}

/// `/dmca`. A copyright complaint route exists in public because the site
/// hosts user uploads it did not make: having a designated agent and a
/// written procedure is what the safe harbour turns on, and search engines
/// look for the page too.
#[component]
pub fn Dmca() -> impl IntoView {
    view! {
        <Title text=format!("Copyright and takedowns — {}", crate::flavor::get().name)/>
        <SitePreview/>
        <Meta
            name="description"
            content=format!("How to ask {} to remove {} that infringes your copyright, and what happens next.", crate::flavor::get().name, crate::flavor::get().a_noun())
        />
        <Link rel="canonical" href=absolute("/dmca")/>

        <article class=ARTICLE>
            <h1 class="m-0 mb-1.5 text-[2rem] font-bold tracking-tight">"Copyright & takedowns"</h1>
            <p class="m-0 mb-6 text-[0.85rem] italic text-ink-3">"DMCA notice and takedown procedure."</p>

            <p>
                {format!("{} hosts content uploaded by its visitors and does not review it \
                before it appears. If something here infringes a copyright you own or represent, \
                tell us and it will be taken down.", crate::flavor::get().name)}
            </p>

            <h2>"The quickest way"</h2>
            <p>
                {format!("Open the {}, press the flag, choose \"stolen / copyright (DMCA)\" and say who you \
                are and what you own in the message box. Reports go straight to the moderation queue.", crate::flavor::get().noun)}
            </p>

            <h2>"A formal notice"</h2>
            {
                // No agent configured means no address to print: the flag on
                // the page is then the only route, and the copy says so
                // rather than inventing an inbox.
                let email = crate::flavor::get().contact_email.clone();
                if email.is_empty() {
                    view! { <p>"If you need a formal DMCA notice on record, send it through the report flag with your details in the message, including:"</p> }.into_any()
                } else {
                    let href = format!("mailto:{email}");
                    view! { <p>"If you need a formal DMCA notice on record, email the designated agent at " <a href=href>{email}</a> " with:"</p> }.into_any()
                }
            }
            <ul>
                <li>{format!("the URL of each {} you want removed;", crate::flavor::get().noun)}</li>
                <li>"a description of the work you say is infringed;"</li>
                <li>"your name, address, phone number and email;"</li>
                <li>"a statement that you believe in good faith the use is not authorised by the owner, its agent or the law;"</li>
                <li>"a statement, under penalty of perjury, that the information is accurate and that you are the owner or authorised to act for the owner;"</li>
                <li>"your physical or electronic signature."</li>
            </ul>

            <h2>"What happens next"</h2>
            <p>
                {format!("Valid notices are acted on promptly: the {} is hidden, then deleted along with \
                its files. Uploaders who repeatedly infringe lose the ability to upload. If you \
                uploaded something that was removed and believe that was a mistake, reply to the \
                same address with a counter-notice.", crate::flavor::get().noun)}
            </p>
        </article>
    }
}

/// `/privacy`. Plain-language and specific about the actual subprocessors this
/// site uses (D1, R2, Cloudflare Tunnel, Google OAuth) -- a generic boilerplate
/// policy would be actively misleading about what really happens to a
/// visitor's data here.
///
/// A good-faith draft covering what the site actually does, not a substitute
/// for legal review.
#[component]
pub fn Privacy() -> impl IntoView {
    view! {
        <Title text=format!("Privacy policy — {}", crate::flavor::get().name)/>
        <SitePreview/>
        <Meta
            name="description"
            content=format!("What {} collects, why, and who it's shared with.", crate::flavor::get().name)
        />
        <Link rel="canonical" href=absolute("/privacy")/>

        <article class=ARTICLE>
            <h1 class="m-0 mb-1.5 text-[2rem] font-bold tracking-tight">"Privacy"</h1>
            <p class="m-0 mb-6 text-[0.85rem] italic text-ink-3">"Last updated: September 2026."</p>

            <p>
                {format!("This is a plain-language description of what {} actually does with \
                data. It is a good-faith description, not a substitute for legal review.", crate::flavor::get().name)}
            </p>

            <h2>"What we collect"</h2>
            <ul>
                <li>
                    "Files you upload -- images, GIFs and videos -- and the title you give them. \
                    Uploads are public by default and stay that way unless removed (see \
                    \"Removal\" below). Images are re-encoded on the way in, which strips camera \
                    and location metadata; GIFs and videos are stored as uploaded, so strip your \
                    own metadata from those if it matters to you."
                </li>
                <li>
                    "If you sign in with Google: your name, email and avatar, used to attribute \
                    uploads to you (first name only, publicly), to run the leaderboard, and to \
                    make likes and reports one-per-person. We don't request anything beyond basic \
                    profile info, and your email is never shown to anyone."
                </li>
                <li>"Standard server logs (IP address, timestamps, request paths) for security and abuse response."</li>
                <li>"Aggregate, cookieless analytics through Cloudflare Web Analytics: page views, referrers, browser and country. No individual is identified."</li>
            </ul>

            <h2>"What we don't do"</h2>
            <ul>
                <li>"We don't sell data, to anyone, ever."</li>
                <li>"We don't run advertising or ad-tech trackers."</li>
                <li>"We don't require an account to upload or browse."</li>
            </ul>

            <h2>"Who else sees it"</h2>
            <p>"Infrastructure providers that store or move data on our behalf, none of whom we permit to use it for their own purposes:"</p>
            <ul>
                <li>"Cloudflare -- database (D1), media storage (R2), network routing (Tunnel), and analytics."</li>
                <li>"Google -- only if you choose to sign in, for authentication."</li>
            </ul>

            <h2>"Content moderation"</h2>
            <p>
                "Uploads are published immediately. Nothing analyses what a file contains before \
                it goes live. What we do check is whether the file is identical to one already \
                here, using a hash, which tells us nothing about the content itself. Moderation \
                is after the fact and depends on reports."
            </p>

            <h2>"Removal"</h2>
            <p>
                {format!("Report any {} you believe shouldn't be here using the flag on its page. Three \
                reports hide it automatically pending review. If you uploaded something and want \
                it taken down, report it yourself with a note, or use the contact route on the ", crate::flavor::get().noun)} <A href="/dmca">"DMCA page"</A> "."
            </p>

            <h2>"Children's privacy"</h2>
            <p>"This site is not directed at children under 13, and we don't knowingly collect data from them."</p>

            <h2>"Changes"</h2>
            <p>"If this policy changes in a way that matters, the date at the top of this page will change too."</p>
        </article>
    }
}

/// `/tos`.
#[component]
pub fn Terms() -> impl IntoView {
    view! {
        <Title text=format!("Terms of service — {}", crate::flavor::get().name)/>
        <SitePreview/>
        <Meta name="description" content=format!("The rules for using and contributing to {}.", crate::flavor::get().name)/>
        <Link rel="canonical" href=absolute("/tos")/>

        <article class=ARTICLE>
            <h1 class="m-0 mb-1.5 text-[2rem] font-bold tracking-tight">"Terms"</h1>
            <p class="m-0 mb-6 text-[0.85rem] italic text-ink-3">"Last updated: September 2026."</p>

            <p>{format!("By using {}, you agree to these terms. They're written in plain language on purpose.", crate::flavor::get().name)}</p>

            <h2>"What you can upload"</h2>
            <ul>
                <li>"Only files you have the right to share. Don't upload someone else's copyrighted work as your own."</li>
                <li>"Nothing illegal. Nothing that depicts real minors in a sexual context. No genuine explicit content."</li>
                <li>"No harassment, threats, gore, or content that exists to attack a real person rather than to be a meme."</li>
                <li>{format!("No spam, and nothing that is not a good-faith {} -- that's what the report reasons are for.", crate::flavor::get().noun)}</li>
            </ul>

            <h2>"The license you grant"</h2>
            <p>
                {format!("By uploading, you grant {} a non-exclusive, worldwide, royalty-free \
                license to host, display, and distribute the file as part of the site and its public \
                API and embeds (oEmbed, Open Graph previews, downloads). You keep whatever rights you \
                had in it -- this isn't a transfer of ownership, just permission to run the site.", crate::flavor::get().name)}
            </p>

            <h2>"Moderation"</h2>
            <p>
                "Uploads are published without prior review and can be reported by anyone signed \
                in. We may remove or hide any content, at any time, for any reason. Because nothing \
                screens an upload before it appears, content that breaks these rules can be visible \
                until someone reports it."
            </p>

            <h2>"No warranty"</h2>
            <p>"The site is provided \"as is,\" with no warranty of any kind."</p>

            <h2>"Limitation of liability"</h2>
            <p>
                {format!("To the fullest extent the law allows, {} and its operator aren't liable \
                for damages arising from your use of the site or content on it.", crate::flavor::get().name)}
            </p>

            <h2>"Changes"</h2>
            <p>"These terms may change as the site does. Continued use after a change means you accept the new terms."</p>

            <h2>"Contact"</h2>
            <p>"For takedown requests and legal notices, see the " <A href="/dmca">"DMCA page"</A> {format!(". For anything about a specific {}, report it -- that routes straight to moderation.", crate::flavor::get().noun)}</p>
        </article>
    }
}

#[cfg(test)]
mod tests {
    use super::about_faq_json_ld;

    #[test]
    fn faq_json_ld_is_valid_json_with_five_questions() {
        let v: serde_json::Value =
            serde_json::from_str(&about_faq_json_ld()).expect("FAQ JSON-LD parses");
        assert_eq!(v["@type"], "FAQPage");
        assert_eq!(v["mainEntity"].as_array().map(Vec::len), Some(5));
    }
}
