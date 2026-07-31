# TODO

Phased work list from the 2026-07-30 full review (multi-agent, adversarially verified).
Work lowest-numbered items first; later phases assume earlier fixes have landed.

## Phase 1 — Data-loss fixes

### 1. [DONE 2026-07-30] Fix buffered HTML post loss on interrupt

main.rs:344-356 — in HTML mode each rendered post sits in `buffered` until the next post is rendered, but `job.offset += batch_len; job.save()` (main.rs:355) persists an offset past the batch's last post while it's still memory-only. No signal handler exists (tokio lacks the `signal` feature), so Ctrl-C skips the flush at main.rs:371 and --resume permanently skips that post. Fix: write each post to disk immediately WITH the OLDER_NAV_PLACEHOLDER, then rewrite the file once the older neighbor is known (the placeholder-replacement machinery in template.rs already supports this). Alternative/additional: tokio::signal::ctrl_c handling. Note: building the index page (P4 task) would let the nav-buffering scheme be retired entirely.

### 2. [DONE 2026-07-30] Atomic writes for media and state files (temp + rename)

Two verified bugs, one fix. (1) images.rs:94 treats bare file existence as "already downloaded", and images.rs:152 writes non-atomically — a crash mid-write leaves a truncated image that every future run (including repair re-runs) skips forever. (2) job.rs:49 and state.rs:27 write .archivr-job.json/.archivr-state.json non-atomically. Write to `{dest}.part`/tempfile and rename into place (atomic on same filesystem). One shared helper covers all three call sites.

### 3. [DONE 2026-07-30] Track incremental high-water mark across all batches

main.rs:274-278 takes newest_observed from the FIRST non-empty batch only, on the assumption Tumblr returns newest-first. Research verdict: Tumblr's docs never state the sort order for `after`-filtered queries (only that `after` is "best used with sort=asc"), and crabrave 1.1.1 exposes no sort param, so the assumption is unverifiable. Fix (order-independent, ~2 lines): fold max(timestamp) over every batch instead of just the first. Also clamp against now() so a single future-dated post (Tumblr allows arbitrary post dates) can't permanently poison the mark.

### 4. [DONE 2026-07-30] Apply prior-state floor only for matching blog

main.rs:175-186 — the blog_name guard lives inside the `if config.incremental` branch (main.rs:61), but the save path reads prior_state unconditionally. A non-incremental backup of blog B into a directory holding blog A's .archivr-state.json writes state labeled B carrying max(B_newest, A_highwater); if A's mark is higher, B's next --incremental silently fetches nothing. Fix: only apply the .max() floor when prior_state.blog_name == config.blog_name.

## Phase 2 — Robustness, security, and performance

### 5. [DONE 2026-07-30] Auto-retry on rate limit with backoff

main.rs:251-263 already matches CrabError::RateLimit { retry_after } but aborts with a "use --resume" message. Instead: tokio::time::sleep(retry_after) and continue the loop (bounded retries; fall back to the current message if retry_after is absent/huge — tumblr-utils refuses to sleep >1h and treats daily-limit exhaustion as fatal). This was #1 on the README's planned list (automatic rate limit retry with backoff).

### 6. [DONE 2026-07-30] Add timeouts and transient-error retries to network calls

No timeout exists on API page fetches (main.rs:249) or image downloads (images.rs:146) — one stalled connection hangs the backup silently forever. Add per-request timeouts (reqwest client-level if crabrave allows, else tokio::time::timeout wrappers) and a small retry policy for transient 5xx/connect errors (tumblr-utils uses Retry(3) on {500,502,503,504} with a 90s timeout as a reference point).

### 7. [DONE 2026-07-30] Globalize download concurrency and overlap with page fetching

images.rs:86 creates the 6-permit semaphore inside download_images, which main.rs:296 awaits per post — so a post with one image downloads one image at a time and the API loop blocks during downloads. Fix: one shared semaphore across the run, and decouple downloads from post processing (spawned task pool or channel of URLs) so page fetching and media downloads overlap. Verified high-impact for image-heavy multi-thousand-post blogs. Related cheap fix while in here: in --directories mode each post gets a private media dir (main.rs:288-294), defeating the hash-name dedup that flat mode gets — consider a shared media store with per-post relative links.

### 8. [DONE 2026-07-30] Redact secrets from log output

main.rs:32 logs `args: {:?}` at debug level — Args derives Debug, so RUST_LOG=debug prints consumer_secret (exactly the mode users enable when filing bug reports). Fix: manual Debug impl for Args that redacts consumer_key/consumer_secret, or log selected fields. Also auth.rs:188: the malformed-cookie-line warning logs the raw line including the session cookie value — log only the domain/field-count, not the values.

### 9. [DONE 2026-07-30] Harden OAuth callback flow

(1) auth.rs:135-137 — a callback without a state parameter only log::warns and proceeds; since Tumblr always echoes state, a state-less callback is by definition not from Tumblr (a drive-by web page can hit 127.0.0.1:6263 and deliver an attacker-controlled code). Make missing state an error. (2) lib.rs:67-73 — any stray local request the parser can't extract a code from (port scanner, extension probe) propagates an error and aborts the whole auth flow; respond 404 and keep listening instead. (3) Minor: lib.rs:43 bind failure (port in use) yields a bare OS error — add context suggesting another archivr instance may be running.

### 10. [DONE 2026-07-30] Surface silent failure modes to the user

Three verified silent degradations (log::warn is invisible without RUST_LOG): (1) main.rs:109-113 — unreadable .archivr-job.json makes --resume restart at offset 0 while still printing "Resuming with saved parameters"; print the fallback to stdout. (2) images.rs:79-84 — media-dir creation failure turns --save-images into a silent no-op CDN-linked archive; print to stdout or hard-error since it contradicts an explicit flag. (3) main.rs:328 — validate post.id is numeric before using it as a filesystem path component and nav href (cheap defense-in-depth against a hostile/proxied API). Also consider: corrupt auth.json currently hard-errors with no recovery hint (auth.rs:234) — fall back to interactive auth with a notice.

### 11. [DONE 2026-07-30] Pin pagination window / move to before-timestamp cursoring

main.rs:236-239,355 — pure offset pagination addresses a mutating list: deletions inside the already-fetched prefix permanently and silently skip a post at the boundary (verified; insertions only cause harmless re-fetches). JobState.started_at (job.rs:12) is written/restored but never read — it looks like an unfinished `before` pin. Minimal fix: pin `before = started_at` on new jobs (crabrave accepts before+offset together). Correct fix (tumblr-utils' model): paginate by before = oldest_seen_timestamp each page, decrementing on ties to avoid same-second skips; also resolves the same-second exclusive-boundary caveat documented at main.rs:62-66. Design change — do after P1 fixes land.

Implementation notes (2026-07-30): went with the full cursor model, window pinned at
started_at+1, `--after`/incremental became a client-side floor (before-windows have
guaranteed order; server-side `after` order is undocumented), same-second runs page
by before+offset with a one-post overlap stride, and re-included boundary posts are
deduped by a seen-id set seeded from the job file across resumes. Nav-chain handoff
(last page ids + last post href/file) is persisted so `--resume` neither severs nor
orphans links. Legacy `offset` job files re-scan the window (files reused; old
per-post media hard-linked into the shared store). Research correction: upstream
bbolli/tumblr-utils actually uses pure offset pagination; the cebtenzzre fork
follows Tumblr's server-provided next-page links and assumes `before` is exclusive.
Known accepted limits: multiple deletions inside one >20-post same-second window
between fetches can still skip (API limitation); the window pin trusts the local
clock, so posts stamped ahead of local time surface on the next run.

## Phase 3 — Leapfrog features

### 12. [DONE 2026-07-31] Notes backup via API (--save-notes)

Leapfrog feature: tumblr-utils scrapes post HTML pages for notes (subprocess, breaks on Safe Mode interstitials); archivr holds an OAuth token and can use crabrave's real API — blogs(id).notes(post_id) with NoteMode (All/Likes/Conversation/Rollup/ReblogsWithTags) and before_timestamp pagination. Add --save-notes (+ optional --notes-limit), render as a <details> block in the default template, include note data in --json output. Was already on the README planned list; the API route makes it strictly better than the inspiration.

Implementation notes (2026-07-31): fetch_page's timeout/retry/rate-limit policy was
extracted into api::send_with_retries (generic over crabrave builders) and notes
pagination lives in notes.rs, following the server's `_links` before_timestamp
cursor with a strict-decrease guard against cursor loops. Notes fetch is skipped
when note_count == 0. A 4xx on one post's notes warns and archives the post
without notes (api::is_client_api_error distinguishes this from transient/rate
-limit failures, which still abort resumably). JSON output embeds the collection
under a `notes` key; --save-notes/--notes-limit persist in the job file for
--resume. rollup_notes are captured alongside the primary list; --notes-limit
caps only the primary list. Post-review hardening (adversarially verified with
a live mock server): a MAX_NOTES_PAGES cap bounds a hostile strictly-decreasing
cursor (which the decrease guard alone bounds only at ~2^63 pages); an
unparseable body outliving the retry budget (WAF HTML on a 4xx) degrades that
post's notes instead of aborting the run; and envelope-level 429s are excluded
from the per-post-skippable class so they stay fatal.

### 13. [DONE 2026-07-31] Drafts, queue, and submissions backup

Feature tumblr-utils cannot offer at all (no OAuth): back up the authenticated user's drafts, queued posts, and submissions — the only copies that exist anywhere. crabrave surface: blogs(id).drafts() (before_id/filter), blogs(id).queue(), blogs(id).submissions(). Suggest flags like --drafts/--queue/--submissions (or a --private umbrella) writing to subdirectories, reusing the existing render/JSON pipeline.

Implementation notes (2026-07-31): --drafts/--queue/--submissions are additive to
the normal backup, writing to drafts/ / queue/ / submissions/ subdirectories via
the shared render/JSON/image pipeline (media links point back into the shared
store one level up; nav links disabled — the older-slot placeholder is closed
immediately). Drafts paginate by before_id, queue/submissions by offset, each
with a seen-id no-progress guard; the offset stride overlaps by one post as
deletion insurance (the queue shrinks under us whenever Tumblr publishes from
it on schedule), mirroring advance_window. Known accepted limits: `--resume`
restores the job's saved flag set, so adding --drafts/--queue/--submissions on
a resume of a job saved without them is silently ignored (consistent with all
other job params); published/deleted drafts leave stale files behind (archival
bias); specials never fetch notes even for a reverted-to-draft post that once
had them. No pagination state is persisted: collections
are small and mutate freely (queued posts publish themselves), so every run
re-fetches wholesale; specials run after the main window but before job-file
deletion, so a failure there leaves --resume able to re-run them. Research
correction vs the sketch above: crabrave has no API-key-only path — one OAuth
client serves everything, so no auth changes were needed.

## Phase 4 — Parity features

### 14. Likes backup (--likes)

README planned feature, confirmed feasible: crabrave users().likes() (authenticated user's likes, limit/offset/before/after) or blogs(id).likes() (any blog's public likes; tumblr-utils aborts on 403 when likes aren't public). Note tumblr-utils tracks incremental state for likes by liked_timestamp rather than post timestamp since like-order isn't post-ID-ordered — .archivr-state.json will need a parallel field or per-mode state.

### 15. Index page generation (--index-file)

README planned feature; pure local rendering (no API needed). tumblr-utils reference: index.html plus paginated monthly archive pages, optional per-tag index (--tag-index, tag dirs keyed by digest). Building this also retires the nav-buffering scheme (see P1 buffered-post task) — navigation can live in the index instead of prev/next links on each post, which is how tumblr-utils avoids the buffering hazard entirely. Consider regenerating the index at the end of every run including --incremental/--resume, which also heals the severed nav chains at resume/incremental seams (verified finding, main.rs:371).

### 16. Tag and type filtering

README planned --include-tags: PostsBuilder.tag(...) is server-side but single-tag; comma-separated lists need client-side filtering after fetch (tumblr-utils filters client-side too). While in there, add the unplanned cheap win --type: PostsBuilder.post_type(...) already exists (text/photo/quote/link/chat/audio/video). Also consider --no-reblog/--only-reblog via reblogged_from_name — trivial client-side.

### 17. Native video/audio download (--save-video, --save-audio)

README planned feature. Native Tumblr media: reuse the images.rs pattern (collect/download/rewrite) applied to Video/Audio ContentBlock MediaObject urls — same hash-prefixed filenames, same url_map rewrite. External embeds (YouTube/Vimeo): tumblr-utils shells out to yt-dlp; offer an opt-in --save-video-external that subprocesses yt-dlp if present (pass --cookies-file through). Depends on the P2 download-concurrency rework landing first for sane throughput.

## Phase 5 — Cheap wins and polish

### 18. CLI ergonomics batch (count/skip/progress/exit codes/multi-blog)

Cheap wins from the tumblr-utils comparison, all CLI-layer or already-available data: --count/--skip (limit/offset params exist on PostsBuilder); progress display "N / total posts" (crabrave already parses total_posts — currently never read); scripting-friendly distinct exit codes (tumblr-utils: 0 backed up / 1 nothing / 2 usage / 3 interrupted / 4 HTTP errors); multiple positional blog names per invocation with aggregated failure reporting.

### 19. Archive-fidelity batch (avatar/mtime/media-list/id-file/wayback)

Second cheap-wins batch: avatar backup via blogs(id).avatar(size) (one call, one file); set post-file mtimes to the post timestamp (archives sort naturally — tumblr-utils does this by default); --media-list newline-JSON export mapping post id → media URLs; --id-file / single-post selection via blogs(id).post(id).get(); Internet Archive fallback — on 403/404 for a media URL retry https://web.archive.org/web/0/{url} (genuinely valuable for old blogs whose CDN assets are gone).

### 20. Trust and concurrency polish (--no-embeds, lock file)

(1) embed_html passes into archives raw by design (template.rs:627-633, 706-712) — verified as a deliberate but undocumented trust decision with no opt-out; add a --no-embeds flag (drop or link-ify embeds) and a README sentence documenting the trust model. Do NOT escape by default — that would break all legitimate YouTube/Vimeo embeds. (2) Add a lock file in the output dir so two concurrent runs don't interleave .archivr-job.json writes. (3) Low-priority: UTF-16 surrogate-pair split in render_formatted_text can yield U+FFFD (template.rs:151) — snap range boundaries to code-point boundaries.

### 21. Housekeeping (CLAUDE.md, crabrave doc bug)

(1) Project CLAUDE.md is stale: says there is no src/ directory (there is) and omits state.rs, images.rs, template.rs from the architecture list — update it. (2) Report/fix upstream in crabrave 1.1.1: after()'s doc-comment is a copy-paste of before()'s ("Returns posts before this timestamp", handlers/blog.rs:792); while there, consider adding a `sort` query param to PostsBuilder — its absence is why archivr can't pin ordering for after-filtered incremental queries (see P1 high-water task).
