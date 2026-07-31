# archivr

A Tumblr backup tool.

## Installing

Binary distributions are available on the [GitHub Releases page](https://github.com/rfaulhaber/archivr/releases).

You may also use `cargo install`, Nix, or compile the binary yourself.

With `cargo`, you can run

```sh
cargo install archivr
```

This repository also provides a Nix flake. You can run the command by using:

```sh
# or `nix shell` to add it to a shell
nix run github:rfaulhaber/archivr
```

You can also add it to your Nix configuration that way.

## Prerequisites

### Register a new Tumblr application

In order to interact with the Tumblr API, archivr needs an OAuth consumer key and secret.

1. In Tumblr, go to Settings > Apps
2. Click on the "Register" link at the bottom
3. Click the green "Register application" button
4. Fill out the following fields:
   1. **Application name**: archivr (this doesn't really matter, this is for your reference)
   2. **Application website**: github.com/rfaulhaber/archivr (again, doesn't really matter)
   3. **Application description**: Tumblr backup tool
   4. **Administrative contact email**: your email
   5. **Default callback URL**: `http://localhost:6263/callback`
   6. **OAuth2 redirect URLs**: `http://localhost:6263/redirect`
5. Click "Save changes"

## Usage

```sh
archivr <BLOG_NAME> --consumer-key <YOUR CONSUMER KEY> --consumer-secret <YOUR CONSUMER SECRET>
```

This will kick off a job to back up an entire blog.

Several blogs can be backed up in one invocation — each goes to its own output directory (subdirectories of `--output-dir` if one is given, `./{blog_name}` otherwise), and a failure on one blog doesn't stop the others:

```sh
archivr blog-one blog-two blog-three --consumer-key KEY --consumer-secret SECRET
```

### CLI flags

| Flag                | Short | Description                                                                        |
|---------------------|-------|------------------------------------------------------------------------------------|
| `--consumer-key`    |       | Tumblr OAuth consumer key                                                          |
| `--consumer-secret` |       | Tumblr OAuth consumer secret                                                       |
| `--config-file`     |       | Path to a JSON config file with `blog_name`, `consumer_key`, and `consumer_secret` |
| `--output-dir`      | `-o`  | Output directory (defaults to `./{blog_name}`)                                     |
| `--json`            |       | Save posts as raw JSON instead of HTML                                             |
| `--template`        | `-t`  | Custom Jinja template for HTML output (exclusive with `--json`)                    |
| `--directories`     | `-d`  | Create a subdirectory for each post                                                |
| `--save-images`     |       | Download post images locally instead of linking to CDN                             |
| `--save-notes`      |       | Fetch and archive each post's notes (likes, reblogs, replies)                      |
| `--notes-limit`     |       | Maximum notes to fetch per post, newest first (requires `--save-notes`)            |
| `--drafts`          |       | Also back up the blog's drafts (blog must belong to the authenticated user)        |
| `--queue`           |       | Also back up the blog's queued posts (blog must belong to the authenticated user)  |
| `--submissions`     |       | Also back up the blog's submissions (blog must belong to the authenticated user)   |
| `--likes`           |       | Also back up the blog's liked posts (likes must be public, or the blog yours)      |
| `--include-tags`    |       | Only back up posts carrying at least one of these comma-separated tags             |
| `--type`            |       | Only back up posts of this type (`text`, `photo`, `quote`, `link`, `chat`, `audio`, `video`, `answer`) |
| `--no-reblogs`      |       | Skip reblogs, backing up only original posts                                       |
| `--only-reblogs`    |       | Back up only reblogs                                                               |
| `--save-video`      |       | Download native Tumblr video files locally instead of linking to CDN               |
| `--save-audio`      |       | Download native Tumblr audio files locally instead of linking to CDN               |
| `--save-video-external` |   | Download external video embeds (YouTube, Vimeo, ...) with `yt-dlp`, if installed   |
| `--index-file`      |       | Generate an `index.html` linking all archived posts, grouped by month              |
| `--count`           |       | Back up at most N posts (newest first)                                             |
| `--skip`            |       | Skip the N newest posts before backing up                                          |
| `--save-avatar`     |       | Also download the blog's avatar into the output directory                          |
| `--media-list`      |       | Write `media-list.jsonl` mapping each archived post's id to its media URLs         |
| `--post-id`         |       | Back up only these posts (comma-separated ids) instead of paginating               |
| `--id-file`         |       | Back up only the post ids listed in this file (one per line)                       |
| `--wayback`         |       | Retry dead media URLs (403/404) through the Internet Archive's Wayback Machine     |
| `--no-embeds`       |       | Render external embeds (YouTube, Vimeo, ...) as plain links instead of provider HTML |
| `--before`          |       | Only fetch posts before this date (Unix timestamp or RFC3339)                      |
| `--after`           |       | Only fetch posts after this date (Unix timestamp or RFC3339)                       |
| `--resume`          |       | Resume a previously interrupted backup                                             |
| `--incremental`     |       | Fetch only posts newer than the last successful backup                             |
| `--quiet`           | `-q`  | Suppress progress output                                                           |
| `--reauth`          |       | Force re-authentication, ignoring saved tokens                                     |
| `--cookies-file`    |       | Path to a Netscape/Mozilla-format cookies file for dashboard access                |
| `--dashboard`       |       | Use Tumblr's internal dashboard API (requires `--cookies-file`)                    |
| `--headless`        |       | Manual auth flow for environments without a browser (servers, containers)          |

### Headless / server usage

When running archivr on a remote server, in a container, or anywhere without a browser, the default OAuth flow won't work because the `localhost` redirect can't reach your machine. Use `--headless` to authenticate manually:

```sh
archivr my-blog --consumer-key KEY --consumer-secret SECRET --headless
```

This will:

1. Print an authorization URL
2. You open that URL in a browser on any machine and authorize with Tumblr
3. Tumblr redirects your browser to `http://localhost:6263/redirect?code=...` — the page will fail to load, but the full URL will be visible in your browser's address bar
4. Copy the URL from the address bar and paste it into the terminal
5. archivr extracts the authorization code and completes authentication

The resulting token is saved to disk, so subsequent runs don't need `--headless` again unless the token expires and can't be refreshed.

### Incremental backups

After an initial full backup, you can keep an archive up to date by re-running with `--incremental`:

```sh
archivr my-blog --consumer-key KEY --consumer-secret SECRET --incremental
```

archivr records the timestamp of the newest post archived in `.archivr-state.json` (inside the output directory). On the next incremental run, only posts newer than that timestamp are fetched. The state file persists across runs and is updated on every successful backup.

If no prior state file exists (e.g. the first time you use `--incremental`), archivr falls back to a full backup to establish the baseline.

`--incremental` is mutually exclusive with `--resume` and `--after`. If a previous run was interrupted, finish it with `--resume` first, then run `--incremental` next time.

### Notes backup

`--save-notes` archives each post's notes — likes, reblogs (including any added commentary and tags), and replies — through the Tumblr API:

```sh
archivr my-blog --consumer-key KEY --consumer-secret SECRET --save-notes
```

In HTML output, notes appear in a collapsible section at the bottom of each post. In `--json` output, they're embedded in each post's file under a `notes` key (with `total_notes`, `notes`, and `rollup_notes` fields).

Each post's notes cost at least one extra API request (more for heavily-noted posts, which are paginated), so backups of large blogs run noticeably longer and eat into Tumblr's rate limits sooner — rate-limit waits are handled automatically the same way as regular post fetches. For popular posts with tens of thousands of notes, `--notes-limit N` caps the fetch at the `N` newest notes.

### Drafts, queue, and submissions

Because archivr authenticates with OAuth, it can back up posts that never appear on the public blog — often the only copies that exist anywhere:

```sh
archivr my-blog --consumer-key KEY --consumer-secret SECRET --drafts --queue --submissions
```

Each flag adds a collection to the normal backup, written to its own subdirectory of the output dir (`drafts/`, `queue/`, `submissions/`) in the same format (HTML or `--json`) as the rest of the archive. The blog must belong to the authenticated account — Tumblr rejects these requests for blogs you don't own.

These collections are small and change freely between runs (queued posts leave the queue when published), so they're re-fetched in full on every run, including `--incremental` ones. Files for posts that have since been deleted or published stay on disk.

### Likes

`--likes` adds the blog's liked posts to the backup, written to a `likes/` subdirectory the same way as the collections above. Any blog's likes can be backed up as long as they're shared publicly; for your own blog they work regardless of that setting.

Tumblr's API only exposes the most recent likes (roughly the last 1000) through this endpoint. When the blog reports more likes than were reachable, archivr prints a notice with both numbers.

### Filtering

`--include-tags art,gif` backs up only posts carrying at least one of the listed tags (case-insensitive). A single tag is filtered server-side, saving API quota; longer lists are filtered client-side after fetching. `--type video` restricts the backup to one post type, and `--no-reblogs` / `--only-reblogs` split original posts from reblogs.

Filters combine with `--incremental`, but the state file records which filters a backup ran with, and the incremental baseline is only trusted between runs using the same filters — mixing filtered and unfiltered runs in one output directory would otherwise leave silent gaps. A one-off filtered run into an existing archive leaves the archive's incremental state untouched.

### Video and audio

`--save-video` and `--save-audio` download posts' native Tumblr media files into the shared `media/videos/` and `media/audio/` stores, rewriting the archived HTML/JSON to point at the local copies — the same treatment `--save-images` gives images.

Externally hosted embeds (YouTube, Vimeo, ...) can't be fetched directly. With `--save-video-external`, archivr hands their URLs to [yt-dlp](https://github.com/yt-dlp/yt-dlp) (if installed) and stores the results under `media/videos/external/`, deduplicated across runs. These files are kept alongside the archive; the post HTML keeps the original embed. A `--cookies-file` is passed through to yt-dlp for embeds that need authentication.

### Index page

`--index-file` generates an `index.html` at the archive root listing every archived post, grouped by month with dates, titles, and tags, plus links to any special-collection subdirectories. The index is regenerated at the end of every run — including `--incremental` and `--resume` ones — from a persistent `.archivr-index.json` metadata file that accumulates across runs.

For archives created before this flag existed, run one full (non-incremental) backup with `--index-file` to populate the metadata.

### Partial backups

`--count N` stops after archiving the `N` newest posts, and `--skip N` skips the newest `N` before archiving starts; both persist across `--resume`. Because a count/skip run covers only a slice of the blog, it never updates the incremental state file — mixing it into an `--incremental` workflow would leave silent gaps (both flags conflict with `--incremental` for the same reason).

`--post-id 123,456` or `--id-file ids.txt` (one id per line; `#` comments allowed) fetch exactly the listed posts through the normal pipeline — media download, notes, index — with no pagination at all. An id that can't be fetched (deleted, wrong blog) prints a warning and is skipped.

### Archive fidelity

Each archived post file's modification time is set to the post's timestamp, so archives sort naturally by post date in a file manager.

`--save-avatar` stores the blog's avatar as `avatar.{ext}` (extension detected from the image bytes) in the output directory.

`--media-list` writes `media-list.jsonl`: one JSON object per archived post (`{"id": ..., "urls": [...]}`) listing every media URL the post carries — images, native video/audio, and external embeds — before any rewriting to local paths. The file describes one run: a fresh run truncates it, `--resume` appends. Useful for feeding a separate downloader.

`--wayback` retries media URLs that come back 403/404 through the Internet Archive's Wayback Machine (`web.archive.org`). For old blogs whose CDN assets are long gone, a snapshot is often the only copy left.

### Embeds and the trust model

Everything archivr writes into an archive is HTML-escaped or sanitized — with one deliberate exception: provider embed HTML (the `embed_html` Tumblr relays from YouTube, Vimeo, Spotify, ...) is passed through raw, because escaping it would break every legitimate embed. If you'd rather not carry third-party markup in your archive, `--no-embeds` strips it before rendering — embeds become plain links to the source URL. This applies to custom templates too: with the flag set, `embed_html` is absent from the template context.

### Exit codes

For scripting, archivr exits with:

| Code | Meaning                                                        |
|------|----------------------------------------------------------------|
| 0    | Backup completed and archived at least one item                |
| 1    | Backup completed but archived nothing (e.g. no new posts)      |
| 2    | Usage or configuration error                                   |
| 3    | Interrupted (Ctrl-C); `--resume` continues where it left off   |
| 4    | API or network failure                                         |
| 5    | Other runtime error (disk, rendering, ...)                     |

A lock file (`.archivr-lock`) in each output directory prevents two archivr instances from writing to the same archive at once; the lock is released automatically when the process exits, however it exits.

### Job config file

You can specify all of the CLI arguments in a config file as well, passing `--config-file <PATH>` instead.

### Custom templates

By default, archivr renders each post as a self-contained HTML file using a built-in template. You can override this with your own [Jinja](https://jinja.palletsprojects.com/) template:

```sh
archivr my-blog --consumer-key KEY --consumer-secret SECRET --template my-template.html
```

> **Note:** The `--template` (`-t`) flag is mutually exclusive with `--json`. When `--json` is set, posts are saved as raw JSON and no template rendering occurs.

Templates are rendered with [minijinja](https://github.com/mitsuhiko/minijinja), which supports standard Jinja2 syntax — `{{ }}` for expressions, `{% %}` for control flow, and `{# #}` for comments.

#### Template context

Your template receives the following variables:

| Variable | Type | Description |
|---|---|---|
| `post` | object | The full post object (see fields below) |
| `is_reblog` | bool | `true` if the post was reblogged from another blog |
| `is_original` | bool | `true` if the post is original content |
| `newer_href` | string? | Relative URL to the next-newer post (for navigation links) |
| `notes` | object? | The post's captured notes when `--save-notes` is active: `total` (server-reported count), `notes`, and `rollup_notes` (lists of note objects with `note_type`, `timestamp`, `date`, `blog_name`, `blog_url`, `reply_text`, `added_text`, `reblog_parent_blog_name`, and `tags`) |

#### Post fields

Access these as `{{ post.field_name }}`:

| Field                 | Type    | Description                             |
|-----------------------|---------|-----------------------------------------|
| `id`                  | int     | The post ID                             |
| `blog_name`           | string  | Name of the blog                        |
| `post_url`            | string  | Full URL to the post on Tumblr          |
| `post_type`           | string  | Post type (e.g. `"text"`, `"photo"`)    |
| `original_type`       | string  | Original post type before conversion    |
| `timestamp`           | int     | Unix timestamp                          |
| `date`                | string  | Human-readable date                     |
| `content`             | list    | Content blocks (see below)              |
| `trail`               | list    | Reblog trail items                      |
| `tags`                | list    | List of tag strings                     |
| `summary`             | string  | Post summary text                       |
| `note_count`          | int     | Number of notes                         |
| `slug`                | string  | URL slug                                |
| `short_url`           | string  | Short URL                               |
| `reblog_key`          | string  | Reblog key                              |
| `state`               | string  | Post state (e.g. `"published"`)         |
| `reblogged_from_name` | string? | Blog name this was reblogged from       |
| `reblogged_from_url`  | string? | URL of the blog this was reblogged from |
| `reblogged_root_name` | string? | Original post's blog name               |
| `reblogged_root_url`  | string? | Original post's blog URL                |
| `liked`               | bool    | Whether you liked the post              |
| `followed`            | bool    | Whether you follow the blog             |

#### Content blocks

Each item in `post.content` (and in each trail item's `content`) is an object with a `type` field. The possible types and their fields are:

**`text`** — A text block.
- `text` (string) — The text content (may contain HTML).
- `subtype` (string?) — Style hint: `"heading1"`, `"heading2"`, `"quote"`, `"indented"`, `"chat"`, etc.

**`image`** — An image block.
- `media` (list) — Each entry has `url`, `width`, `height`, and `media_type`.
- `alt_text` (string?) — Alt text for the image.
- `caption` (string?) — Image caption.

**`video`** — A video block.
- `media` (list?) — Each entry has `url`, `width`, `height`, and `media_type`.
- `url` (string?) — External video URL (when no direct media).
- `provider` (string?) — Video provider name (e.g. `"youtube"`).
- `embed_html` (string?) — Embeddable HTML from the provider.
- `duration` (number?) — Duration in seconds.

**`audio`** — An audio block.
- `media` (list?) — Each entry has `url` and `media_type`.
- `url` (string?) — External audio URL.
- `provider` (string?) — Audio provider name.
- `title` (string?) — Track title.
- `artist` (string?) — Artist name.
- `album` (string?) — Album name.
- `embed_html` (string?) — Embeddable HTML.

**`link`** — A link block.
- `url` (string) — The link URL.
- `title` (string?) — Link title.
- `description` (string?) — Link description.

**`paywall`** — A paywall/premium content marker.
- `text` (string?) — Display text (defaults to "Premium content").

#### Trail items

Each item in `post.trail` has:

| Field | Type | Description |
|---|---|---|
| `content` | list | Content blocks (same types as above) |
| `blog` | object? | Blog info with `name`, `url`, and `uuid` |
| `post` | object? | Post info with `id` |
| `is_root_item` | bool | Whether this is the root trail item |

#### The `render_block()` function

Templates have access to a built-in `render_block(block)` function that converts a content block into the default HTML representation. This lets you customize the overall page layout while reusing the default rendering for individual blocks:

```jinja
{# Loop through content blocks, using the built-in renderer for each one #}
{% for block in post.content %}
  {{ render_block(block) }}
{% endfor %}
```

You can also selectively override rendering for specific block types:

```jinja
{% for block in post.content %}
  {% if block.type == "image" %}
    {# Custom image rendering #}
    {% for m in block.media %}
      <img src="{{ m.url }}" alt="{{ block.alt_text }}" loading="lazy">
    {% endfor %}
  {% else %}
    {{ render_block(block) }}
  {% endif %}
{% endfor %}
```

#### Example: minimal custom template

```jinja
<!DOCTYPE html>
<html>
<head><title>{{ post.blog_name }} - {{ post.id }}</title></head>
<body>
  <h1>{{ post.blog_name }}</h1>
  <p>{{ post.date }} · {{ post.note_count }} notes</p>

  {% if is_reblog %}
    <p>Reblogged from {{ post.reblogged_from_name }}</p>
  {% endif %}

  {% for item in post.trail %}
    <blockquote>
      {% if item.blog %}<strong>{{ item.blog.name }}:</strong>{% endif %}
      {% for block in item.content %}
        {{ render_block(block) }}
      {% endfor %}
    </blockquote>
  {% endfor %}

  {% for block in post.content %}
    {{ render_block(block) }}
  {% endfor %}

  {% for tag in post.tags %}
    <span>#{{ tag }}</span>
  {% endfor %}
</body>
</html>
```

## Planned features

The following features are not yet implemented but are planned for future releases:

- Per-tag index pages
- EXIF tagging of downloaded images
