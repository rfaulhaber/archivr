//! The archive's post index (`--index-file`): a persistent metadata file
//! merged across runs, rendered to an `index.html` grouping every archived
//! post by month.
//!
//! The metadata outlives runs (like `.archivr-state.json`) because the HTML
//! post files themselves don't carry enough machine-readable data to rebuild
//! a listing: an incremental or resumed run only sees the posts it fetched,
//! so entries from earlier runs must come from this file. Archives created
//! before `--index-file` existed backfill by running one full (non
//! -incremental) backup, which re-processes every post in the window.

use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use crabrave::handlers::blog::Post;
use serde::{Deserialize, Serialize};

use crate::PostTimestamp;
use crate::template::html_escape;

pub const INDEX_METADATA_PATH: &str = ".archivr-index.json";
pub const INDEX_HTML_PATH: &str = "index.html";

/// The listing-relevant facts about one archived post.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub timestamp: PostTimestamp,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchiveIndex {
    pub blog_name: String,
    /// Keyed by post id; re-archiving a post overwrites its entry.
    pub entries: HashMap<String, IndexEntry>,
}

impl ArchiveIndex {
    pub fn new(blog_name: &str) -> Self {
        Self {
            blog_name: blog_name.to_owned(),
            entries: HashMap::new(),
        }
    }

    /// Loads the metadata file, or `Ok(None)` when none exists yet.
    pub fn load(path: &Utf8Path) -> anyhow::Result<Option<Self>> {
        if !fs_err::exists(path).unwrap_or(false) {
            return Ok(None);
        }
        let content = fs_err::read_to_string(path)?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    pub fn save(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let content = serde_json::to_string(self)?;
        crate::fsutil::write_atomic(path, content.as_bytes())?;
        Ok(())
    }

    pub fn metadata_path(output_dir: &Utf8Path) -> Utf8PathBuf {
        output_dir.join(INDEX_METADATA_PATH)
    }

    pub fn html_path(output_dir: &Utf8Path) -> Utf8PathBuf {
        output_dir.join(INDEX_HTML_PATH)
    }

    pub fn record(&mut self, post: &Post) {
        let summary = post
            .summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.entries.insert(
            post.id.clone(),
            IndexEntry {
                timestamp: post.timestamp,
                summary,
                tags: post.tags.clone(),
            },
        );
    }

    /// Renders the index page: every recorded post grouped by month, newest
    /// first, plus links to whichever special-collection subdirectories
    /// exist. `directories`/`json` control how post hrefs are formed and must
    /// match the archive's layout.
    pub fn render_html(&self, directories: bool, json: bool, special_dirs: &[&str]) -> String {
        let mut posts: Vec<(&String, &IndexEntry)> = self.entries.iter().collect();
        // Newest first; id breaks same-second ties deterministically.
        posts.sort_by(|(id_a, a), (id_b, b)| {
            b.timestamp.cmp(&a.timestamp).then_with(|| id_b.cmp(id_a))
        });

        let mut body = String::new();
        let mut current_month: Option<String> = None;
        for (id, entry) in posts {
            let month = chrono::DateTime::from_timestamp(entry.timestamp, 0)
                .map(|dt| dt.format("%B %Y").to_string())
                .unwrap_or_else(|| "Unknown date".to_owned());
            if current_month.as_deref() != Some(month.as_str()) {
                if current_month.is_some() {
                    body.push_str("</ul>\n");
                }
                body.push_str(&format!("<h2>{}</h2>\n<ul class=\"posts\">\n", html_escape(&month)));
                current_month = Some(month);
            }

            let date = chrono::DateTime::from_timestamp(entry.timestamp, 0)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "????-??-??".to_owned());
            let href = post_href(id, directories, json);
            let label = entry
                .summary
                .clone()
                .unwrap_or_else(|| format!("Post {id}"));
            body.push_str(&format!(
                "<li><span class=\"date\">{date}</span> <a href=\"{href}\">{label}</a>",
                href = html_escape(&href),
                label = html_escape(&label)
            ));
            if !entry.tags.is_empty() {
                body.push_str(" <span class=\"tags\">");
                for tag in &entry.tags {
                    body.push_str(&format!("<span class=\"tag\">#{}</span> ", html_escape(tag)));
                }
                body.push_str("</span>");
            }
            body.push_str("</li>\n");
        }
        if current_month.is_some() {
            body.push_str("</ul>\n");
        }

        let mut specials = String::new();
        if !special_dirs.is_empty() {
            specials.push_str("<nav class=\"specials\">Also archived: ");
            for (i, dir) in special_dirs.iter().enumerate() {
                if i > 0 {
                    specials.push_str(" &middot; ");
                }
                specials.push_str(&format!(
                    "<a href=\"{dir}/\">{dir}</a>",
                    dir = html_escape(dir)
                ));
            }
            specials.push_str("</nav>\n");
        }

        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{blog} - archive</title>
  <style>
    body {{ font-family: system-ui, sans-serif; max-width: 650px; margin: 2rem auto; padding: 0 1rem; line-height: 1.6; color: #333; }}
    h1 {{ font-size: 1.4rem; }}
    h2 {{ font-size: 1.1rem; border-bottom: 1px solid #ddd; padding-bottom: 0.25rem; margin-top: 2rem; }}
    .count {{ color: #666; font-size: 0.9rem; }}
    .specials {{ color: #666; font-size: 0.9rem; margin-top: 0.5rem; }}
    ul.posts {{ list-style: none; padding: 0; margin: 0; }}
    ul.posts li {{ padding: 0.2rem 0; }}
    .date {{ color: #888; font-size: 0.85rem; font-family: monospace; margin-right: 0.5rem; }}
    a {{ color: #0066cc; text-decoration: none; }}
    a:hover {{ text-decoration: underline; }}
    .tags {{ font-size: 0.8rem; }}
    .tag {{ color: #888; }}
  </style>
</head>
<body>
  <h1>{blog}</h1>
  <p class="count">{count} archived posts</p>
{specials}{body}</body>
</html>
"#,
            blog = html_escape(&self.blog_name),
            count = self.entries.len(),
        )
    }
}

/// Href of a post file relative to the index page at the archive root.
/// Explicit `index.*` filenames (rather than a bare `id/`) keep the links
/// working when the archive is browsed over `file://`, with no server to
/// resolve directory defaults.
fn post_href(id: &str, directories: bool, json: bool) -> String {
    let ext = if json { "json" } else { "html" };
    if directories {
        format!("{id}/index.{ext}")
    } else {
        format!("{id}.{ext}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn entry(ts: PostTimestamp, summary: Option<&str>, tags: &[&str]) -> IndexEntry {
        IndexEntry {
            timestamp: ts,
            summary: summary.map(str::to_owned),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    fn sample_index() -> ArchiveIndex {
        let mut idx = ArchiveIndex::new("test-blog");
        // 2023-11-14 and 2023-12-01, deliberately inserted out of order.
        idx.entries.insert("100".to_owned(), entry(1_699_920_000, Some("older post"), &["art"]));
        idx.entries.insert("200".to_owned(), entry(1_701_388_800, Some("newer post"), &[]));
        idx
    }

    #[test]
    fn post_href_modes() {
        assert_eq!(post_href("1", false, false), "1.html");
        assert_eq!(post_href("1", false, true), "1.json");
        assert_eq!(post_href("1", true, false), "1/index.html");
        assert_eq!(post_href("1", true, true), "1/index.json");
    }

    #[test]
    fn render_groups_by_month_newest_first() {
        let html = sample_index().render_html(false, false, &[]);
        let december = html.find("December 2023").unwrap();
        let november = html.find("November 2023").unwrap();
        assert!(december < november);
        let newer = html.find("newer post").unwrap();
        let older = html.find("older post").unwrap();
        assert!(newer < older);
        assert!(html.contains(r#"<a href="200.html">newer post</a>"#));
        assert!(html.contains("#art"));
        assert!(html.contains("2 archived posts"));
    }

    #[test]
    fn render_escapes_untrusted_fields() {
        let mut idx = ArchiveIndex::new("<b>blog</b>");
        idx.entries.insert(
            "1".to_owned(),
            entry(1_700_000_000, Some("<script>x</script>"), &["<i>t</i>"]),
        );
        let html = idx.render_html(false, false, &[]);
        assert!(!html.contains("<script>x"));
        assert!(!html.contains("<b>blog</b>"));
        assert!(!html.contains("<i>t</i>"));
    }

    #[test]
    fn render_links_special_dirs() {
        let html = sample_index().render_html(false, false, &["drafts", "likes"]);
        assert!(html.contains(r#"<a href="drafts/">drafts</a>"#));
        assert!(html.contains(r#"<a href="likes/">likes</a>"#));
    }

    #[test]
    fn record_prefers_nonempty_summary() {
        let json = serde_json::json!({
            "id_string": "123",
            "blog_name": "test",
            "post_url": "https://test.tumblr.com/post/123",
            "type": "text",
            "timestamp": 1700000000,
            "scheduled_publish_time": 0,
            "note_count": 0,
            "reblog_key": "abc",
            "summary": "   ",
            "is_blocks_post_format": true,
            "followed": false,
            "liked": false,
            "can_like": false,
            "can_reblog": false,
            "can_reply": false,
            "can_send_in_message": false,
            "can_mute": false,
            "display_avatar": false,
            "interactability_reblog": "",
            "is_blazed": false,
            "is_blaze_pending": false,
            "can_ignite": false,
            "can_blaze": false,
            "muted": false,
            "mute_end_timestamp": 0,
            "content": [],
            "trail": [],
            "tags": ["a"],
            "layout": []
        });
        let post: Post = serde_json::from_value(json).unwrap();
        let mut idx = ArchiveIndex::new("test");
        idx.record(&post);
        let entry = idx.entries.get("123").unwrap();
        // Whitespace-only summaries fall back to the id-based label at render
        // time rather than storing an empty string.
        assert_eq!(entry.summary, None);
        assert_eq!(entry.tags, vec!["a"]);
        assert_eq!(entry.timestamp, 1_700_000_000);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir();
        let dir = Utf8Path::from_path(&dir).unwrap();
        let path = dir.join(format!("archivr-test-index-{}.json", std::process::id()));

        let idx = sample_index();
        idx.save(&path).unwrap();
        let loaded = ArchiveIndex::load(&path).unwrap().unwrap();
        assert_eq!(loaded.blog_name, "test-blog");
        assert_eq!(loaded.entries.len(), 2);
        fs_err::remove_file(&path).unwrap();
    }

    #[test]
    fn load_missing_file_is_none() {
        let loaded = ArchiveIndex::load(Utf8Path::new("/nonexistent/archivr-index.json")).unwrap();
        assert!(loaded.is_none());
    }
}
