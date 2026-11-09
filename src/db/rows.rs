//! Converts an archived [`Post`] into the rows `archive.db` stores for it.
//!
//! The backup sink and the JSON import both go through [`build_rows`], so the
//! two paths derive every column the same way.

use std::collections::HashMap;

use anyhow::Context as _;
use crabrave::handlers::blog::{Post, TrailItem};
use crabrave::npf::ContentBlock;

use crate::media::best_image_variant;
use crate::notes::PostNotes;

/// Which part of a blog a post was archived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collection {
    Posts,
    Drafts,
    Queue,
    Submissions,
    Likes,
}

impl Collection {
    /// The collections archived into their own subdirectory.
    pub const SPECIAL: [Collection; 4] = [
        Collection::Drafts,
        Collection::Queue,
        Collection::Submissions,
        Collection::Likes,
    ];

    /// The value stored in the `collection` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Collection::Posts => "posts",
            Collection::Drafts => "drafts",
            Collection::Queue => "queue",
            Collection::Submissions => "submissions",
            Collection::Likes => "likes",
        }
    }

    /// The archive subdirectory holding this collection's files; `None` for the
    /// blog's own posts, which live at the archive root.
    pub fn dir_name(self) -> Option<&'static str> {
        match self {
            Collection::Posts => None,
            other => Some(other.as_str()),
        }
    }
}

/// How a row's data was obtained, which decides whether it may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSource {
    /// Built from the API response, before media rewriting.
    Api,
    /// Built from an archived `--json` file, whose media is already rewritten.
    JsonImport,
}

impl RowSource {
    /// The value stored in the `source` column.
    pub fn as_str(self) -> &'static str {
        match self {
            RowSource::Api => "api",
            RowSource::JsonImport => "json-import",
        }
    }
}

/// Everything [`build_rows`] needs to describe one archived post.
#[derive(Debug)]
pub struct PostRecord<'a> {
    /// The post before media rewriting, so media rows can record original URLs.
    pub post: &'a Post,
    pub notes: Option<&'a PostNotes>,
    pub collection: Collection,
    /// The archived file, relative to the archive root.
    pub file_path: Option<&'a str>,
    /// Remote URL to local path, as returned by `download_batch_media`; empty
    /// for imports, whose posts were rewritten before they were written.
    pub url_map: &'a HashMap<String, String>,
    pub source: RowSource,
}

/// A `posts` row; `archived_at` is stamped when the row is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRow {
    pub id: i64,
    pub collection: Collection,
    pub blog_name: String,
    pub timestamp: i64,
    pub date: String,
    pub original_type: Option<String>,
    pub state: Option<String>,
    pub post_url: Option<String>,
    pub short_url: Option<String>,
    pub slug: Option<String>,
    pub summary: Option<String>,
    pub note_count: i64,
    pub is_reblog: bool,
    pub parent_blog: Option<String>,
    pub parent_post_url: Option<String>,
    pub root_blog: Option<String>,
    pub root_post_url: Option<String>,
    pub body: String,
    pub trail_text: String,
    pub tags: String,
    pub authors: String,
    pub file_path: Option<String>,
    pub source: RowSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrailRow {
    pub position: i64,
    pub blog: Option<String>,
    pub source_post_id: Option<String>,
    pub is_root: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRow {
    pub position: i64,
    /// `None` for the post's own content.
    pub trail_position: Option<i64>,
    pub kind: &'static str,
    pub external: bool,
    pub url: Option<String>,
    pub path: Option<String>,
    pub mime: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub alt_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub position: i64,
    pub note_type: String,
    pub blog_name: Option<String>,
    pub timestamp: Option<i64>,
    pub text: Option<String>,
    pub tags: Option<String>,
}

/// One post's rows across every table it touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostRows {
    pub post: PostRow,
    pub tags: Vec<String>,
    pub trail: Vec<TrailRow>,
    pub media: Vec<MediaRow>,
    pub notes: Vec<NoteRow>,
}

/// Builds the rows for one post. Fails only when the post id doesn't fit an
/// `i64`, which `ensure_numeric_id` makes unreachable for API posts.
pub fn build_rows(rec: &PostRecord<'_>) -> anyhow::Result<PostRows> {
    let post = rec.post;
    let id = post
        .id
        .parse::<i64>()
        .with_context(|| format!("post id {:?} is not a 64-bit integer", post.id))?;

    let root_id = post.reblogged_root_id.as_deref();
    let mut trail: Vec<TrailRow> = post
        .trail
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let source_post_id = trail_item_post_id(item);
            TrailRow {
                position: row_position(index),
                blog: non_empty(item.blog.as_ref().and_then(|b| b.name.as_deref())),
                source_post_id: source_post_id.map(str::to_owned),
                is_root: item.is_root_item
                    || (source_post_id.is_some() && source_post_id == root_id),
                text: trail_item_text(item),
            }
        })
        .collect();
    // Posts as archivr fetches them carry no root marker: `reblogged_root_*`
    // needs `reblog_info`, which archivr doesn't request, and real trail items
    // arrive with `is_root_item` false. The trail is ordered root first, so
    // with nothing named the first item is the root; a named root that isn't
    // in the trail is not guessed at.
    if root_id.is_none()
        && let Some(first) = trail.first_mut()
        && !post.trail.iter().any(|item| item.is_root_item)
    {
        first.is_root = true;
    }

    // The `reblogged_from_*` fields are absent for the same reason, so an
    // archived reblog survives only as `parent_post_url` and the trail. The
    // trail ends at the nearest ancestor that added content, which is the
    // parent unless it reblogged without commenting, so it is the last resort.
    let parent_post_url = non_empty(post.reblogged_from_url.as_deref())
        .or_else(|| non_empty(post.parent_post_url.as_deref()));
    let parent_url = parent_post_url.as_deref().and_then(parse_post_url);
    // The trail item for the parent post names its blog as Tumblr knows it,
    // where the URL may carry a custom domain or a private blog's numeric id.
    let parent_blog = non_empty(post.reblogged_from_name.as_deref())
        .or_else(|| {
            let id = parent_url.as_ref()?.post_id?;
            let item = trail
                .iter()
                .find(|t| t.source_post_id.as_deref() == Some(id))?;
            item.blog.clone()
        })
        .or_else(|| parent_url.as_ref().map(|url| url.blog.to_owned()))
        .or_else(|| trail.last().and_then(|t| t.blog.clone()));
    let root_blog = non_empty(post.reblogged_root_name.as_deref()).or_else(|| {
        trail
            .iter()
            .find(|t| t.is_root)
            .and_then(|t| t.blog.clone())
    });

    let trail_text = trail
        .iter()
        .map(|t| t.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut authors: Vec<&str> = Vec::new();
    for name in trail.iter().filter_map(|t| t.blog.as_deref()) {
        if !authors.contains(&name) {
            authors.push(name);
        }
    }

    let post_row = PostRow {
        id,
        collection: rec.collection,
        blog_name: post.blog_name.clone(),
        timestamp: post.timestamp,
        date: format_date(post.timestamp),
        original_type: non_empty(post.original_type.as_deref()),
        state: non_empty(post.state.as_deref()),
        post_url: non_empty(Some(&post.post_url)),
        short_url: non_empty(post.short_url.as_deref()),
        slug: non_empty(post.slug.as_deref()),
        summary: non_empty(post.summary.as_deref()),
        note_count: i64::try_from(post.note_count).unwrap_or(i64::MAX),
        is_reblog: post.reblogged_from_name.is_some()
            || parent_post_url.is_some()
            || !trail.is_empty(),
        parent_blog,
        parent_post_url,
        root_blog,
        root_post_url: non_empty(post.reblogged_root_url.as_deref()),
        body: blocks_text(&post.content),
        trail_text,
        tags: post.tags.join("\n"),
        authors: authors.join("\n"),
        file_path: rec.file_path.map(str::to_owned),
        source: rec.source,
    };

    Ok(PostRows {
        post: post_row,
        tags: post.tags.clone(),
        trail,
        media: media_rows(post, rec.url_map),
        notes: rec.notes.map(note_rows).unwrap_or_default(),
    })
}

/// A trail item's plain text. Legacy items carry only `content_raw` HTML.
fn trail_item_text(item: &TrailItem) -> String {
    match &item.content_raw {
        Some(raw) if item.content.is_empty() => strip_html(raw),
        _ => blocks_text(&item.content),
    }
}

fn trail_item_post_id(item: &TrailItem) -> Option<&str> {
    item.post
        .as_ref()
        .and_then(|p| p.id.as_deref())
        .filter(|id| !id.is_empty())
}

/// Media rows in document order: the post's own content, then each trail item.
fn media_rows(post: &Post, url_map: &HashMap<String, String>) -> Vec<MediaRow> {
    let mut rows = Vec::new();
    push_block_media(&post.content, None, url_map, &mut rows);
    for (index, item) in post.trail.iter().enumerate() {
        push_block_media(&item.content, Some(row_position(index)), url_map, &mut rows);
    }
    rows
}

fn push_block_media(
    blocks: &[ContentBlock],
    trail_position: Option<i64>,
    url_map: &HashMap<String, String>,
    rows: &mut Vec<MediaRow>,
) {
    for block in blocks {
        // The embed URL only applies to video/audio: images have no external form.
        let (kind, media, embed_url, alt_text) = match block {
            ContentBlock::Image {
                media, alt_text, ..
            } => (
                "image",
                best_image_variant(media),
                None,
                alt_text.as_deref(),
            ),
            ContentBlock::Video { media, url, .. } => {
                ("video", media.as_deref(), url.as_deref(), None)
            }
            ContentBlock::Audio { media, url, .. } => {
                ("audio", media.as_deref(), url.as_deref(), None)
            }
            _ => continue,
        };
        let position = row_position(rows.len());
        let row = if let Some(m) = media.filter(|m| !m.url.is_empty()) {
            let (url, path) = resolve_media_url(&m.url, url_map);
            MediaRow {
                position,
                trail_position,
                kind,
                external: false,
                url,
                path,
                mime: non_empty(m.media_type.as_deref()),
                width: m.width.map(i64::from),
                height: m.height.map(i64::from),
                alt_text: non_empty(alt_text),
            }
        } else if let Some(url) = embed_url.filter(|u| !u.is_empty()) {
            MediaRow {
                position,
                trail_position,
                kind,
                external: true,
                url: Some(url.to_owned()),
                path: None,
                mime: None,
                width: None,
                height: None,
                alt_text: None,
            }
        } else {
            continue;
        };
        rows.push(row);
    }
}

/// Splits a chosen media URL into the `(url, path)` columns.
///
/// A rewritten post (an import) carries only the local path, so the original
/// URL is unknown and stays `NULL` rather than borrowing one from another
/// variant, which would pair a URL with a file of a different size.
fn resolve_media_url(
    url: &str,
    url_map: &HashMap<String, String>,
) -> (Option<String>, Option<String>) {
    if let Some(local) = url_map.get(url) {
        (Some(url.to_owned()), Some(archive_relative(local)))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        (Some(url.to_owned()), None)
    } else {
        (None, Some(archive_relative(url)))
    }
}

fn note_rows(notes: &PostNotes) -> Vec<NoteRow> {
    notes
        .notes
        .iter()
        .enumerate()
        .map(|(index, note)| NoteRow {
            position: row_position(index),
            note_type: note.note_type.clone(),
            blog_name: non_empty(Some(&note.blog_name)),
            timestamp: Some(note.timestamp),
            text: non_empty(note.reply_text.as_deref())
                .or_else(|| non_empty(note.added_text.as_deref())),
            tags: (!note.tags.is_empty()).then(|| note.tags.join("\n")),
        })
        .collect()
}

fn row_position(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Formats a Unix timestamp as RFC 3339 UTC, or an empty string when it is out
/// of the range chrono can represent.
fn format_date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// The searchable plain text of a block list, one line per contributing piece.
/// Only fields a reader would see as text contribute; media, layout, and
/// unrecognized block types do not.
pub fn blocks_text(blocks: &[ContentBlock]) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => lines.push(text),
            // The URL and site name are searchable too: a link post can be
            // nothing but a bare link block.
            ContentBlock::Link {
                url,
                title,
                description,
                author,
                site_name,
                ..
            } => {
                lines.extend(
                    [title, description, site_name, author]
                        .into_iter()
                        .flatten()
                        .map(String::as_str),
                );
                lines.push(url);
            }
            ContentBlock::Poll {
                question, answers, ..
            } => {
                lines.push(question);
                lines.extend(answers.iter().map(|a| a.answer_text.as_str()));
            }
            ContentBlock::Audio {
                title,
                artist,
                album,
                ..
            } => lines.extend(
                [title, artist, album]
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            ),
            ContentBlock::Image {
                alt_text, caption, ..
            } => lines.extend(
                [alt_text, caption]
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            ),
            ContentBlock::Paywall { title, text, .. } => {
                lines.extend([title, text].into_iter().flatten().map(String::as_str));
            }
            _ => {}
        }
    }
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

/// Reduces legacy HTML to plain text: tags are dropped, `<br>`, `</p>`, and
/// `</div>` become newlines, and character references are decoded.
pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                if tag_breaks_line(&rest[1..end]) {
                    out.push('\n');
                }
                rest = &rest[end + 1..];
            } else {
                // A lone `<` is text, not a tag.
                out.push('<');
                rest = &rest[1..];
            }
        } else if let Some((decoded, len)) = decode_entity(rest) {
            out.push(decoded);
            rest = &rest[len..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// Whether a tag (the text between `<` and `>`) ends a line of text.
fn tag_breaks_line(tag: &str) -> bool {
    let closing = tag.starts_with('/');
    let name = tag.trim_start_matches('/');
    let name = &name[..name
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(name.len())];
    name.eq_ignore_ascii_case("br")
        || (closing && (name.eq_ignore_ascii_case("p") || name.eq_ignore_ascii_case("div")))
}

/// Decodes the character reference at the start of `s` (which begins with `&`),
/// returning the character and the byte length consumed.
fn decode_entity(s: &str) -> Option<(char, usize)> {
    // The longest reference worth decoding is `&#x10FFFF;`; bounding the search
    // keeps text full of bare `&` linear.
    let end = s.bytes().take(12).position(|b| b == b';')?;
    let decoded = match &s[1..end] {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "nbsp" => ' ',
        numeric => {
            let digits = numeric.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((decoded, end + 1))
}

/// The blog and post a post URL points at.
#[derive(Debug, PartialEq, Eq)]
struct PostUrl<'a> {
    blog: &'a str,
    post_id: Option<&'a str>,
}

/// Reads a post URL in the forms Tumblr returns: a blog subdomain, a custom
/// domain, or `www.tumblr.com/blog/view/<name>/<id>` and `/blog/<name>?<id>`
/// for blogs without a working subdomain. A custom domain is the only name
/// such a URL carries.
fn parse_post_url(url: &str) -> Option<PostUrl<'_>> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/')?;
    let (blog, post_id) = if host.eq_ignore_ascii_case("www.tumblr.com") {
        let mut parts = path.strip_prefix("blog/")?.split(['/', '?']);
        let first = parts.next()?;
        let blog = if first == "view" {
            parts.next()?
        } else {
            first
        };
        (blog, parts.next())
    } else {
        let post = path.strip_prefix("post/");
        let subdomain = bare_blog_name(host);
        let blog = if subdomain.len() != host.len() {
            subdomain
        } else if post.is_some() {
            // A custom domain names a blog only when the path is a post's.
            host
        } else {
            return None;
        };
        (blog, post.and_then(|post| post.split('/').next()))
    };
    (!blog.is_empty()).then_some(PostUrl {
        blog,
        post_id: post_id.filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())),
    })
}

/// Strips the `../` depth prefixes archived post files use to reach the shared
/// `media/` store, leaving a path relative to the archive root.
pub fn archive_relative(path: &str) -> String {
    let mut rest = path;
    while let Some(stripped) = rest.strip_prefix("../").or_else(|| rest.strip_prefix("./")) {
        rest = stripped;
    }
    rest.to_owned()
}

/// Whether two blog names refer to the same blog. The CLI argument and the
/// API's `blog_name` can differ in case and in a `.tumblr.com` suffix.
pub fn blog_names_match(a: &str, b: &str) -> bool {
    bare_blog_name(a).eq_ignore_ascii_case(bare_blog_name(b))
}

fn bare_blog_name(name: &str) -> &str {
    const SUFFIX: &str = ".tumblr.com";
    match name
        .len()
        .checked_sub(SUFFIX.len())
        .and_then(|at| name.split_at_checked(at))
    {
        Some((stem, tail)) if tail.eq_ignore_ascii_case(SUFFIX) => stem,
        _ => name,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_support::post_json;
    use serde_json::{Value, json};

    fn post_with(edit: impl FnOnce(&mut Value)) -> Post {
        let mut value = post_json("123");
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    fn rows_with_map(post: &Post, url_map: &HashMap<String, String>) -> PostRows {
        build_rows(&PostRecord {
            post,
            notes: None,
            collection: Collection::Posts,
            file_path: None,
            url_map,
            source: RowSource::Api,
        })
        .unwrap()
    }

    fn rows_for(post: &Post) -> PostRows {
        rows_with_map(post, &HashMap::new())
    }

    fn text(s: &str) -> Value {
        json!({"type": "text", "text": s})
    }

    /// An image block with one variant per `(url, has_original_dimensions)`.
    fn image(variants: &[(&str, bool)]) -> Value {
        let media: Vec<Value> = variants
            .iter()
            .map(|(url, original)| {
                json!({
                    "url": url,
                    "type": "image/jpeg",
                    "width": 640,
                    "height": 480,
                    "has_original_dimensions": original,
                })
            })
            .collect();
        json!({"type": "image", "media": media})
    }

    fn trail_item(blog: &str, post_id: &str, content: Value) -> Value {
        json!({"content": content, "blog": {"name": blog}, "post": {"id": post_id}})
    }

    fn note(value: Value) -> crabrave::handlers::blog::Note {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn text_blocks_join_with_newlines() {
        let post = post_with(|v| v["content"] = json!([text("a"), text("b")]));
        assert_eq!(blocks_text(&post.content), "a\nb");
        assert_eq!(rows_for(&post).post.body, "a\nb");
    }

    #[test]
    fn empty_pieces_contribute_no_lines() {
        let post = post_with(|v| v["content"] = json!([text("a"), text(""), text("b")]));
        assert_eq!(blocks_text(&post.content), "a\nb");
    }

    #[test]
    fn link_poll_audio_contribute_text() {
        let post = post_with(|v| {
            v["content"] = json!([
                {
                    "type": "link",
                    "url": "https://example.com",
                    "title": "Link title",
                    "description": "Link description",
                },
                {
                    "type": "poll",
                    "client_id": "p",
                    "question": "Which?",
                    "answers": [
                        {"client_id": "1", "answer_text": "This"},
                        {"client_id": "2", "answer_text": "That"},
                    ],
                },
                {
                    "type": "audio",
                    "url": "https://example.com/song",
                    "title": "Song",
                    "artist": "Artist",
                    "album": "Album",
                },
            ]);
        });
        assert_eq!(
            blocks_text(&post.content),
            "Link title\nLink description\nhttps://example.com\nWhich?\nThis\nThat\nSong\nArtist\nAlbum"
        );
    }

    /// A link block as archived from a real post: a `site_name` and a `url`
    /// with no description.
    #[test]
    fn link_site_name_and_url_are_searchable() {
        let post = post_with(|v| {
            v["content"] = json!([
                {
                    "type": "link",
                    "url": "https://archive.is/dl0Lv",
                    "title": "Shale chiefs warn Trump",
                    "site_name": "archive.is",
                    "author": "A. Writer",
                },
                text("US shale bosses have warned"),
            ]);
        });
        assert_eq!(
            blocks_text(&post.content),
            "Shale chiefs warn Trump\narchive.is\nA. Writer\nhttps://archive.is/dl0Lv\nUS shale bosses have warned"
        );
    }

    /// A reblogged embed whose only content is a bare link block; without its
    /// URL the trail item would have no text at all.
    #[test]
    fn bare_link_block_contributes_its_url() {
        let post = post_with(|v| {
            v["trail"] = json!([trail_item(
                "a",
                "1",
                json!([{"type": "link", "url": "https://vine.co/v/OTFjhQb5tdz/embed/simple"}])
            )]);
        });
        assert_eq!(
            rows_for(&post).post.trail_text,
            "https://vine.co/v/OTFjhQb5tdz/embed/simple"
        );
    }

    #[test]
    fn image_alt_text_is_searchable() {
        let post = post_with(|v| {
            let mut img = image(&[("https://cdn.example/a.jpg", true)]);
            img["alt_text"] = json!("Photo of a BlackBerry Curve on a car dashboard.");
            img["caption"] = json!("Curve");
            v["trail"] = json!([trail_item("a", "1", json!([img]))]);
        });
        assert_eq!(
            rows_for(&post).post.trail_text,
            "Photo of a BlackBerry Curve on a car dashboard.\nCurve"
        );
    }

    #[test]
    fn image_caption_and_paywall_contribute_text() {
        let post = post_with(|v| {
            let mut img = image(&[("https://cdn.example/a.jpg", true)]);
            img["caption"] = json!("A caption");
            v["content"] = json!([
                img,
                {"type": "paywall", "subtype": "cta", "title": "Support me", "text": "Subscribe"},
            ]);
        });
        assert_eq!(
            blocks_text(&post.content),
            "A caption\nSupport me\nSubscribe"
        );
    }

    #[test]
    fn other_blocks_contribute_nothing() {
        let post = post_with(|v| {
            v["content"] = json!([
                image(&[("https://cdn.example/a.jpg", true)]),
                {"type": "video", "url": "https://youtu.be/x"},
                {"type": "futureblock", "text": "unmodeled"},
            ]);
        });
        assert_eq!(blocks_text(&post.content), "");
    }

    #[test]
    fn legacy_trail_item_uses_stripped_content_raw() {
        let post = post_with(|v| {
            v["trail"] = json!([{
                "content": [],
                "content_raw": "<p>Hello &amp; <b>bye</b></p>",
                "blog": {"name": "a"},
                "post": {"id": "1"},
            }]);
        });
        let rows = rows_for(&post);
        assert_eq!(rows.trail[0].text, "Hello & bye");
        assert_eq!(rows.post.trail_text, "Hello & bye");
    }

    #[test]
    fn trail_text_joins_non_empty_items_with_blank_lines() {
        let post = post_with(|v| {
            v["trail"] = json!([
                trail_item("a", "1", json!([text("first")])),
                trail_item(
                    "b",
                    "2",
                    json!([image(&[("https://cdn.example/a.jpg", true)])])
                ),
                trail_item("c", "3", json!([text("third")])),
            ]);
        });
        assert_eq!(rows_for(&post).post.trail_text, "first\n\nthird");
    }

    #[test]
    fn strip_html_breaks_lines_and_decodes_entities() {
        assert_eq!(
            strip_html(
                "<div>a<br>b<br/>c</div><p>d</p>e &lt;x&gt; &quot;q&quot; it&#39;s&nbsp;ok &#x41;&#66;"
            ),
            "a\nb\nc\nd\ne <x> \"q\" it's ok AB"
        );
        // Decoding is single-pass: an escaped entity is not decoded twice.
        assert_eq!(strip_html("&amp;lt;"), "&lt;");
        assert_eq!(strip_html("fish & chips, 1 < 2"), "fish & chips, 1 < 2");
    }

    #[test]
    fn trail_root_from_reblogged_root_id() {
        let post = post_with(|v| {
            v["reblogged_root_id"] = json!("111");
            v["trail"] = json!([
                trail_item("a", "111", json!([])),
                trail_item("b", "222", json!([])),
            ]);
        });
        let roots: Vec<bool> = rows_for(&post).trail.iter().map(|t| t.is_root).collect();
        assert_eq!(roots, [true, false]);
    }

    #[test]
    fn trail_root_from_is_root_item() {
        let post = post_with(|v| {
            let mut first = trail_item("a", "111", json!([]));
            first["is_root_item"] = json!(true);
            v["trail"] = json!([first, trail_item("b", "222", json!([]))]);
        });
        let roots: Vec<bool> = rows_for(&post).trail.iter().map(|t| t.is_root).collect();
        assert_eq!(roots, [true, false]);
    }

    #[test]
    fn media_url_and_path_from_url_map() {
        let a = "https://64.media.tumblr.com/a.jpg";
        let b = "https://64.media.tumblr.com/b_250.jpg";
        let post = post_with(|v| v["content"] = json!([image(&[(b, false), (a, true)])]));
        let url_map = HashMap::from([(a.to_owned(), "../media/images/h_a.jpg".to_owned())]);

        let rows = rows_with_map(&post, &url_map);
        let media = &rows.media[0];
        assert_eq!(media.kind, "image");
        assert!(!media.external);
        assert_eq!(media.url.as_deref(), Some(a));
        assert_eq!(media.path.as_deref(), Some("media/images/h_a.jpg"));
        assert_eq!(media.mime.as_deref(), Some("image/jpeg"));
        assert_eq!((media.width, media.height), (Some(640), Some(480)));
    }

    #[test]
    fn remote_media_without_download_has_no_path() {
        let a = "https://64.media.tumblr.com/a.jpg";
        let post = post_with(|v| v["content"] = json!([image(&[(a, true)])]));
        let media = &rows_for(&post).media[0];
        assert_eq!(media.url.as_deref(), Some(a));
        assert_eq!(media.path, None);
    }

    #[test]
    fn imported_local_media_has_path_but_no_url() {
        let post = post_with(|v| {
            v["content"] = json!([image(&[
                ("https://64.media.tumblr.com/b_250.jpg", false),
                ("media/images/h_a.jpg", true),
            ])]);
        });
        let media = &rows_for(&post).media[0];
        assert_eq!(media.url, None);
        assert_eq!(media.path.as_deref(), Some("media/images/h_a.jpg"));
    }

    #[test]
    fn image_variant_falls_back_to_first() {
        let first = "https://64.media.tumblr.com/first.jpg";
        let post = post_with(|v| {
            v["content"] = json!([image(&[
                (first, false),
                ("https://64.media.tumblr.com/second.jpg", false),
            ])]);
        });
        assert_eq!(rows_for(&post).media[0].url.as_deref(), Some(first));
    }

    #[test]
    fn image_without_a_usable_url_has_no_row() {
        let post = post_with(|v| {
            v["content"] = json!([{"type": "image", "media": []}, image(&[("", true)])]);
        });
        assert!(rows_for(&post).media.is_empty());
    }

    #[test]
    fn external_video_is_marked_external() {
        let yt = "https://www.youtube.com/watch?v=abc";
        let post = post_with(|v| v["content"] = json!([{"type": "video", "url": yt}]));
        let media = &rows_for(&post).media[0];
        assert_eq!(media.kind, "video");
        assert!(media.external);
        assert_eq!(media.url.as_deref(), Some(yt));
        assert_eq!(media.path, None);
    }

    #[test]
    fn native_video_and_audio_use_their_media_objects() {
        let video = "https://va.media.tumblr.com/v.mp4";
        let audio = "https://a.tumblr.com/a.mp3";
        let post = post_with(|v| {
            v["content"] = json!([
                {"type": "video", "media": {"url": video, "type": "video/mp4", "width": 1280, "height": 720}},
                {"type": "audio", "media": {"url": audio, "type": "audio/mpeg"}, "url": "https://tumblr.com/a"},
            ]);
        });
        let url_map = HashMap::from([(audio.to_owned(), "../../media/audio/h_a.mp3".to_owned())]);

        let rows = rows_with_map(&post, &url_map);
        assert_eq!(rows.media.len(), 2);
        let (video_row, audio_row) = (&rows.media[0], &rows.media[1]);
        assert_eq!(video_row.kind, "video");
        assert!(!video_row.external);
        assert_eq!(video_row.url.as_deref(), Some(video));
        assert_eq!(video_row.path, None);
        assert_eq!(video_row.mime.as_deref(), Some("video/mp4"));
        assert_eq!((video_row.width, video_row.height), (Some(1280), Some(720)));

        assert_eq!(audio_row.kind, "audio");
        assert!(!audio_row.external);
        assert_eq!(audio_row.url.as_deref(), Some(audio));
        assert_eq!(audio_row.path.as_deref(), Some("media/audio/h_a.mp3"));
    }

    #[test]
    fn image_alt_text_is_kept() {
        let post = post_with(|v| {
            let mut img = image(&[("https://cdn.example/a.jpg", true)]);
            img["alt_text"] = json!("a cat");
            v["content"] = json!([img]);
        });
        assert_eq!(rows_for(&post).media[0].alt_text.as_deref(), Some("a cat"));
    }

    #[test]
    fn media_order_is_content_then_trail() {
        let img = |name: &str| image(&[(&format!("https://cdn.example/{name}.jpg"), true)]);
        let post = post_with(|v| {
            v["content"] = json!([img("own")]);
            v["trail"] = json!([
                trail_item("a", "1", json!([img("t0")])),
                trail_item("b", "2", json!([img("t1")])),
            ]);
        });
        let rows = rows_for(&post);
        let positions: Vec<i64> = rows.media.iter().map(|m| m.position).collect();
        let trail_positions: Vec<Option<i64>> =
            rows.media.iter().map(|m| m.trail_position).collect();
        let urls: Vec<&str> = rows.media.iter().filter_map(|m| m.url.as_deref()).collect();
        assert_eq!(positions, [0, 1, 2]);
        assert_eq!(trail_positions, [None, Some(0), Some(1)]);
        assert_eq!(
            urls,
            [
                "https://cdn.example/own.jpg",
                "https://cdn.example/t0.jpg",
                "https://cdn.example/t1.jpg"
            ]
        );
    }

    #[test]
    fn original_post_has_empty_trail_fields() {
        let rows = rows_for(&post_with(|_| {}));
        assert_eq!(rows.post.trail_text, "");
        assert_eq!(rows.post.authors, "");
        assert!(!rows.post.is_reblog);
        assert_eq!(rows.post.parent_blog, None);
        assert_eq!(rows.post.root_blog, None);
        assert!(rows.trail.is_empty());
    }

    #[test]
    fn reblog_fields_are_copied() {
        let post = post_with(|v| {
            v["reblogged_from_name"] = json!("parent");
            v["reblogged_from_url"] = json!("https://parent.tumblr.com/post/2");
            v["reblogged_root_name"] = json!("root");
            v["reblogged_root_url"] = json!("https://root.tumblr.com/post/1");
        });
        let row = rows_for(&post).post;
        assert!(row.is_reblog);
        assert_eq!(row.parent_blog.as_deref(), Some("parent"));
        assert_eq!(
            row.parent_post_url.as_deref(),
            Some("https://parent.tumblr.com/post/2")
        );
        assert_eq!(row.root_blog.as_deref(), Some("root"));
        assert_eq!(
            row.root_post_url.as_deref(),
            Some("https://root.tumblr.com/post/1")
        );
    }

    /// A real 2010 post, reduced: archivr doesn't request `reblog_info`, so
    /// Tumblr returns the reblog only as `parent_post_url` and a trail whose
    /// items carry no `is_root_item`, never the `reblogged_*` fields.
    #[test]
    fn reblog_is_derived_when_the_api_omits_reblogged_fields() {
        let post = post_with(|v| {
            v["blog_name"] = json!("algorithmist");
            v["original_type"] = json!("quote");
            v["parent_post_url"] =
                json!("https://stephenblackpool-deactivated201.tumblr.com/post/2493830667");
            v["trail"] = json!([{
                "content": [text("Do not believe in anything simply because you have heard it.")],
                "blog": {"name": "lamaschingonaa-deactivated20200"},
                "post": {"id": "2135100365"},
                "is_root_item": false,
            }]);
        });
        let rows = rows_for(&post);
        assert!(rows.post.is_reblog);
        assert_eq!(
            rows.post.parent_blog.as_deref(),
            Some("stephenblackpool-deactivated201")
        );
        assert_eq!(
            rows.post.parent_post_url.as_deref(),
            Some("https://stephenblackpool-deactivated201.tumblr.com/post/2493830667")
        );
        assert_eq!(
            rows.post.root_blog.as_deref(),
            Some("lamaschingonaa-deactivated20200")
        );
        assert_eq!(rows.post.root_post_url, None);
        assert!(rows.trail[0].is_root);
    }

    #[test]
    fn reblog_without_a_trail_still_names_its_parent() {
        let post = post_with(|v| {
            v["parent_post_url"] = json!("https://parent.tumblr.com/post/2/a-slug");
        });
        let row = rows_for(&post).post;
        assert!(row.is_reblog);
        assert_eq!(row.parent_blog.as_deref(), Some("parent"));
        assert_eq!(row.root_blog, None);
    }

    #[test]
    fn trail_alone_marks_a_reblog_and_names_the_nearest_trail_blog() {
        let post = post_with(|v| {
            v["trail"] = json!([
                trail_item("root", "1", json!([text("first")])),
                trail_item("middle", "2", json!([text("second")])),
            ]);
        });
        let row = rows_for(&post).post;
        assert!(row.is_reblog);
        assert_eq!(row.parent_blog.as_deref(), Some("middle"));
        assert_eq!(row.root_blog.as_deref(), Some("root"));
        assert_eq!(row.parent_post_url, None);
    }

    #[test]
    fn explicit_reblog_fields_win_over_derived_ones() {
        let post = post_with(|v| {
            v["parent_post_url"] = json!("https://other.tumblr.com/post/9");
            v["reblogged_from_name"] = json!("parent");
            v["reblogged_from_url"] = json!("https://parent.tumblr.com/post/2");
            v["reblogged_root_name"] = json!("root");
            v["trail"] = json!([trail_item("elsewhere", "1", json!([]))]);
        });
        let row = rows_for(&post).post;
        assert_eq!(row.parent_blog.as_deref(), Some("parent"));
        assert_eq!(
            row.parent_post_url.as_deref(),
            Some("https://parent.tumblr.com/post/2")
        );
        assert_eq!(row.root_blog.as_deref(), Some("root"));
    }

    #[test]
    fn first_trail_item_is_the_root_only_when_the_source_names_none() {
        let two_items = || {
            json!([
                trail_item("a", "111", json!([])),
                trail_item("b", "222", json!([])),
            ])
        };
        let roots =
            |post: &Post| -> Vec<bool> { rows_for(post).trail.iter().map(|t| t.is_root).collect() };

        let unmarked = post_with(|v| v["trail"] = two_items());
        assert_eq!(roots(&unmarked), [true, false]);

        // A root the trail doesn't contain is not replaced by a guess.
        let unmatched = post_with(|v| {
            v["reblogged_root_id"] = json!("999");
            v["trail"] = two_items();
        });
        assert_eq!(roots(&unmatched), [false, false]);

        let flagged = post_with(|v| {
            let mut second = trail_item("b", "222", json!([]));
            second["is_root_item"] = json!(true);
            v["trail"] = json!([trail_item("a", "111", json!([])), second]);
        });
        assert_eq!(roots(&flagged), [false, true]);
    }

    /// A real post reblogged from a custom-domain blog: the URL names the
    /// domain, the trail item names the blog.
    #[test]
    fn parent_blog_prefers_the_trail_item_for_the_parent_post() {
        let post = post_with(|v| {
            v["parent_post_url"] = json!(
                "https://ianbrooks.me/post/3351656837/project-ariel-by-cody-ruch-why-must-you-always"
            );
            v["trail"] = json!([
                trail_item("someone", "3351000000", json!([text("art"),])),
                trail_item("ianbrooks", "3351656837", json!([text("by Cody Ruch")])),
            ]);
        });
        assert_eq!(
            rows_for(&post).post.parent_blog.as_deref(),
            Some("ianbrooks")
        );
    }

    #[test]
    fn parent_blog_falls_back_to_the_url_when_the_trail_lacks_the_parent() {
        let post = post_with(|v| {
            v["parent_post_url"] =
                json!("https://www.tumblr.com/blog/private_4580876076?4286111304");
            v["trail"] = json!([trail_item("root", "1", json!([text("x")]))]);
        });
        assert_eq!(
            rows_for(&post).post.parent_blog.as_deref(),
            Some("private_4580876076")
        );
    }

    #[test]
    fn parse_post_url_reads_every_url_shape_tumblr_returns() {
        let parsed = |url| parse_post_url(url).map(|u| (u.blog, u.post_id));
        for (url, expected) in [
            (
                "https://name.tumblr.com/post/12/slug",
                Some(("name", Some("12"))),
            ),
            ("http://name.tumblr.com/post/12", Some(("name", Some("12")))),
            ("https://name.tumblr.com/", Some(("name", None))),
            (
                "https://www.tumblr.com/blog/view/getlostinthesounds/2543117580",
                Some(("getlostinthesounds", Some("2543117580"))),
            ),
            (
                "https://www.tumblr.com/blog/private_5746131?2646770726",
                Some(("private_5746131", Some("2646770726"))),
            ),
            (
                "https://ianbrooks.me/post/2745994191/a-slug",
                Some(("ianbrooks.me", Some("2745994191"))),
            ),
            (
                "https://name.tumblr.com/post/not-a-number",
                Some(("name", None)),
            ),
            ("https://example.com/about", None),
            ("https://www.tumblr.com/dashboard", None),
            ("https://www.tumblr.com/blog/view", None),
            ("https://www.tumblr.com/post/1", None),
            ("no scheme", None),
        ] {
            assert_eq!(parsed(url), expected, "{url}");
        }
    }

    #[test]
    fn authors_are_distinct_trail_blogs_in_order() {
        let post = post_with(|v| {
            v["trail"] = json!([
                trail_item("b", "1", json!([])),
                trail_item("a", "2", json!([])),
                trail_item("b", "3", json!([])),
            ]);
        });
        assert_eq!(rows_for(&post).post.authors, "b\na");
    }

    #[test]
    fn trail_rows_carry_blog_and_source_post() {
        let post = post_with(|v| {
            v["trail"] = json!([
                trail_item("a", "1", json!([text("x")])),
                {"content": [], "blog": {"name": ""}},
            ]);
        });
        let trail = rows_for(&post).trail;
        assert_eq!(trail[0].position, 0);
        assert_eq!(trail[0].blog.as_deref(), Some("a"));
        assert_eq!(trail[0].source_post_id.as_deref(), Some("1"));
        assert_eq!(trail[1].position, 1);
        assert_eq!(trail[1].blog, None);
        assert_eq!(trail[1].source_post_id, None);
    }

    #[test]
    fn tags_are_rows_and_a_joined_column() {
        let post = post_with(|v| v["tags"] = json!(["art", "cats"]));
        let rows = rows_for(&post);
        assert_eq!(rows.tags, ["art", "cats"]);
        assert_eq!(rows.post.tags, "art\ncats");
    }

    #[test]
    fn date_is_rfc3339_utc() {
        let post = post_with(|v| v["timestamp"] = json!(1_793_206_230));
        let row = rows_for(&post).post;
        assert_eq!(row.timestamp, 1_793_206_230);
        assert_eq!(row.date, "2026-10-28T16:50:30Z");
    }

    #[test]
    fn large_id_parses_exactly() {
        let mut value = post_json("831737016835325952");
        value["note_count"] = json!(7);
        let post: Post = serde_json::from_value(value).unwrap();
        let row = rows_for(&post).post;
        assert_eq!(row.id, 831_737_016_835_325_952_i64);
        assert_eq!(row.note_count, 7);
    }

    #[test]
    fn non_numeric_id_is_an_error() {
        let post: Post = serde_json::from_value(post_json("../etc")).unwrap();
        let record = PostRecord {
            post: &post,
            notes: None,
            collection: Collection::Posts,
            file_path: None,
            url_map: &HashMap::new(),
            source: RowSource::Api,
        };
        assert!(build_rows(&record).is_err());
    }

    #[test]
    fn empty_optional_strings_become_none() {
        let post = post_with(|v| {
            v["slug"] = json!("");
            v["summary"] = json!("");
            v["state"] = json!("published");
            v["reblogged_from_name"] = json!("parent");
            v["reblogged_from_url"] = json!("");
        });
        let row = rows_for(&post).post;
        assert_eq!(row.slug, None);
        assert_eq!(row.summary, None);
        assert_eq!(row.state.as_deref(), Some("published"));
        assert_eq!(row.parent_post_url, None);
    }

    #[test]
    fn record_context_is_copied_to_the_row() {
        let post = post_with(|_| {});
        let record = PostRecord {
            post: &post,
            notes: None,
            collection: Collection::Drafts,
            file_path: Some("drafts/123.json"),
            url_map: &HashMap::new(),
            source: RowSource::JsonImport,
        };
        let row = build_rows(&record).unwrap().post;
        assert_eq!(row.collection, Collection::Drafts);
        assert_eq!(row.file_path.as_deref(), Some("drafts/123.json"));
        assert_eq!(row.source, RowSource::JsonImport);
    }

    #[test]
    fn note_rows_from_post_notes() {
        let post = post_with(|_| {});
        let notes = PostNotes {
            total_notes: 3,
            notes: vec![
                note(json!({
                    "type": "reply",
                    "timestamp": 100,
                    "blog_name": "replier",
                    "reply_text": "nice",
                })),
                note(json!({
                    "type": "reblog",
                    "timestamp": 200,
                    "blog_name": "reblogger",
                    "added_text": "so true",
                    "tags": ["a", "b"],
                })),
                note(json!({"type": "like", "timestamp": 300, "blog_name": "liker"})),
            ],
            rollup_notes: vec![crabrave::handlers::blog::RollupNoteGroup {
                notes: vec![note(
                    json!({"type": "like", "timestamp": 1, "blog_name": "rolled"}),
                )],
                total_notes: 1,
                total_likes: None,
                total_reblogs: None,
            }],
        };
        let rows = build_rows(&PostRecord {
            post: &post,
            notes: Some(&notes),
            collection: Collection::Posts,
            file_path: None,
            url_map: &HashMap::new(),
            source: RowSource::Api,
        })
        .unwrap();

        // Rollup notes are excluded, so only the three primary notes appear.
        assert_eq!(rows.notes.len(), 3);
        let (reply, reblog, like) = (&rows.notes[0], &rows.notes[1], &rows.notes[2]);
        assert_eq!((reply.position, reply.note_type.as_str()), (0, "reply"));
        assert_eq!(reply.blog_name.as_deref(), Some("replier"));
        assert_eq!(reply.timestamp, Some(100));
        assert_eq!(reply.text.as_deref(), Some("nice"));
        assert_eq!(reply.tags, None);

        assert_eq!(reblog.position, 1);
        assert_eq!(reblog.text.as_deref(), Some("so true"));
        assert_eq!(reblog.tags.as_deref(), Some("a\nb"));

        assert_eq!(like.text, None);
        assert_eq!(like.tags, None);
    }

    #[test]
    fn no_notes_means_no_note_rows() {
        assert!(rows_for(&post_with(|_| {})).notes.is_empty());
    }

    #[test]
    fn blog_names_match_ignores_case_and_tumblr_suffix() {
        assert!(blog_names_match("Algorithmist", "algorithmist.tumblr.com"));
        assert!(blog_names_match("algorithmist.tumblr.com", "ALGORITHMIST"));
        assert!(blog_names_match("same", "same"));
        assert!(!blog_names_match("a", "b"));
        assert!(!blog_names_match("a.tumblr.com", "a.example.com"));
    }

    #[test]
    fn archive_relative_strips_parent_segments() {
        assert_eq!(
            archive_relative("../../media/audio/x.mp3"),
            "media/audio/x.mp3"
        );
        assert_eq!(
            archive_relative("../media/images/x.jpg"),
            "media/images/x.jpg"
        );
        assert_eq!(archive_relative("./media/x"), "media/x");
        assert_eq!(archive_relative("media/x"), "media/x");
    }

    #[test]
    fn collection_names_and_directories() {
        let names: Vec<&str> = [
            Collection::Posts,
            Collection::Drafts,
            Collection::Queue,
            Collection::Submissions,
            Collection::Likes,
        ]
        .iter()
        .map(|c| c.as_str())
        .collect();
        assert_eq!(names, ["posts", "drafts", "queue", "submissions", "likes"]);

        assert_eq!(Collection::Posts.dir_name(), None);
        let dirs: Vec<Option<&str>> = Collection::SPECIAL.iter().map(|c| c.dir_name()).collect();
        assert_eq!(
            dirs,
            [
                Some("drafts"),
                Some("queue"),
                Some("submissions"),
                Some("likes")
            ]
        );
        assert_eq!(RowSource::Api.as_str(), "api");
        assert_eq!(RowSource::JsonImport.as_str(), "json-import");
    }
}
