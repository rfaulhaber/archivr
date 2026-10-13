//! Fetching a post's notes (likes, reblogs, replies) for archival.
//!
//! Tumblr's notes endpoint pages newest-first via a `before_timestamp` cursor
//! that comes back in the response's `_links`; each page is fetched with the
//! shared retry/rate-limit policy from [`crate::api`].

use crate::api::ClientSource;
use crabrave::handlers::blog::{Note, NoteMode, NotesResponse, RollupNoteGroup};
use serde::Serialize;

use crate::api;

/// Hard cap on notes pages fetched for one post. Real posts fit comfortably
/// (Tumblr serves dozens of notes per page, so this covers hundreds of
/// thousands of notes); its purpose is bounding a server whose pagination
/// cursor never runs dry — strictly-decreasing cursors alone would let a
/// hostile server hold the fetch for ~2^63 pages.
const MAX_NOTES_PAGES: u32 = 10_000;

/// All notes captured for one post, in the API's newest-first order.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PostNotes {
    /// Server-reported total, which can exceed the number archived when
    /// `--notes-limit` truncates the fetch.
    pub total_notes: u64,
    pub notes: Vec<Note>,
    /// Aggregated note groups Tumblr excludes from the primary list, each
    /// carrying its own totals and a sample of the collapsed notes.
    pub rollup_notes: Vec<RollupNoteGroup>,
}

/// Decides the next `before_timestamp` cursor from a notes response, or
/// `None` when pagination is done. The cursor must strictly decrease so a
/// misbehaving server cannot loop the fetch forever; the API serves it as a
/// string, and one that doesn't parse as a timestamp ends pagination too.
fn next_cursor(current: Option<i64>, response: &NotesResponse) -> Option<i64> {
    let next: i64 = response
        .links
        .as_ref()?
        .next
        .as_ref()?
        .query_params
        .before_timestamp
        .parse()
        .ok()?;
    match current {
        Some(cur) if next >= cur => None,
        _ => Some(next),
    }
}

/// Fetches every note on a post (or the newest `limit`, if set), following
/// the server's pagination cursor until it runs out.
pub async fn fetch_post_notes(
    source: &impl ClientSource,
    blog_name: &str,
    post_id: &str,
    limit: Option<u64>,
    quiet: bool,
) -> anyhow::Result<PostNotes> {
    let mut out = PostNotes::default();
    let mut cursor: Option<i64> = None;
    let mut pages: u32 = 0;

    loop {
        if pages >= MAX_NOTES_PAGES {
            log::warn!("post {post_id}: notes fetch truncated after {MAX_NOTES_PAGES} pages");
            if !quiet {
                api::say(format_args!(
                    "  Warning: notes for post {post_id} truncated after \
                     {MAX_NOTES_PAGES} pages; archiving what was fetched."
                ))?;
            }
            break;
        }
        pages += 1;

        let response: NotesResponse = api::send_with_retries(
            source,
            |client| {
                let mut builder = client
                    .blogs(blog_name.to_owned())
                    .notes(post_id.to_owned())
                    .mode(NoteMode::All);
                if let Some(ts) = cursor {
                    builder = builder.before_timestamp(ts);
                }
                builder.send()
            },
            quiet,
        )
        .await?;

        out.total_notes = out.total_notes.max(response.total_notes);
        if response.notes.is_empty() && response.rollup_notes.is_empty() {
            break;
        }

        let next = next_cursor(cursor, &response);
        out.notes.extend(response.notes);
        out.rollup_notes.extend(response.rollup_notes);

        if let Some(l) = limit
            && out.notes.len() as u64 >= l
        {
            out.notes.truncate(usize::try_from(l).unwrap_or(usize::MAX));
            break;
        }

        match next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn response_with_cursor(before_timestamp: Option<i64>) -> NotesResponse {
        let links = before_timestamp.map(|ts| {
            serde_json::json!({
                "next": {
                    "href": "/v2/blog/x/notes",
                    "query_params": { "id": "1", "before_timestamp": ts }
                }
            })
        });
        serde_json::from_value(serde_json::json!({
            "notes": [],
            "total_notes": 0,
            "_links": links,
        }))
        .unwrap()
    }

    #[test]
    fn next_cursor_follows_decreasing_timestamps() {
        let resp = response_with_cursor(Some(500));
        assert_eq!(next_cursor(None, &resp), Some(500));
        assert_eq!(next_cursor(Some(600), &resp), Some(500));
    }

    #[test]
    fn next_cursor_stops_without_links() {
        let resp = response_with_cursor(None);
        assert_eq!(next_cursor(None, &resp), None);
        assert_eq!(next_cursor(Some(100), &resp), None);
    }

    #[test]
    fn next_cursor_refuses_non_decreasing_cursor() {
        // A server echoing the same (or a later) cursor must terminate the
        // loop, not spin it.
        let resp = response_with_cursor(Some(500));
        assert_eq!(next_cursor(Some(500), &resp), None);
        assert_eq!(next_cursor(Some(400), &resp), None);
    }

    fn response_with_raw_cursor(before_timestamp: serde_json::Value) -> NotesResponse {
        serde_json::from_value(serde_json::json!({
            "notes": [],
            "total_notes": 0,
            "_links": {
                "next": {
                    "href": "/v2/blog/x/notes",
                    "query_params": { "id": "1", "before_timestamp": before_timestamp }
                }
            },
        }))
        .unwrap()
    }

    #[test]
    fn next_cursor_parses_string_cursor() {
        // The live API quotes the cursor: "500", not 500.
        let resp = response_with_raw_cursor(serde_json::json!("500"));
        assert_eq!(next_cursor(None, &resp), Some(500));
        assert_eq!(next_cursor(Some(600), &resp), Some(500));
    }

    #[test]
    fn next_cursor_stops_on_unparseable_cursor() {
        let resp = response_with_raw_cursor(serde_json::json!("not-a-timestamp"));
        assert_eq!(next_cursor(None, &resp), None);
    }
}
