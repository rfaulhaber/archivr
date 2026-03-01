# archivr

A Tumblr backup tool.


## Prerequisites

### Register a new Tumblr application

In order to interact with the Tumblr API, archivr needs an OAuth consumer key and secret.

1. In Tumblr, go to Settings > Apps
2. Click on the "Register" link at the bottom
3. Click the green "Register application" button
4. Fill out the following fields:
   1. **Application name**: archivr (this doesn't really matter, this is for your reference)
   2. **Application website**: codeberg.org/ryf/archivr (again, doesn't really matter)
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

### CLI flags

| Flag | Short | Description |
|------|-------|-------------|
| `--consumer-key` | | Tumblr OAuth consumer key |
| `--consumer-secret` | | Tumblr OAuth consumer secret |
| `--config-file` | | Path to a JSON config file with `blog_name`, `consumer_key`, and `consumer_secret` |
| `--output-dir` | `-o` | Output directory (defaults to `./{blog_name}`) |
| `--json` | | Save posts as raw JSON instead of HTML |
| `--template` | `-t` | Custom Jinja template for HTML output (exclusive with `--json`) |
| `--directories` | `-d` | Create a subdirectory for each post |
| `--save-images` | | Download post images locally instead of linking to CDN |
| `--before` | | Only fetch posts before this date (Unix timestamp or RFC3339) |
| `--after` | | Only fetch posts after this date (Unix timestamp or RFC3339) |
| `--resume` | | Resume a previously interrupted backup |
| `--quiet` | `-q` | Suppress progress output |
| `--reauth` | | Force re-authentication, ignoring saved tokens |
| `--cookies-file` | | Path to a Netscape/Mozilla-format cookies file for dashboard access |
| `--dashboard` | | Use Tumblr's internal dashboard API (requires `--cookies-file`) |

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

#### Post fields

Access these as `{{ post.field_name }}`:

| Field | Type | Description |
|---|---|---|
| `id` | int | The post ID |
| `blog_name` | string | Name of the blog |
| `post_url` | string | Full URL to the post on Tumblr |
| `post_type` | string | Post type (e.g. `"text"`, `"photo"`) |
| `original_type` | string | Original post type before conversion |
| `timestamp` | int | Unix timestamp |
| `date` | string | Human-readable date |
| `content` | list | Content blocks (see below) |
| `trail` | list | Reblog trail items |
| `tags` | list | List of tag strings |
| `summary` | string | Post summary text |
| `note_count` | int | Number of notes |
| `slug` | string | URL slug |
| `short_url` | string | Short URL |
| `reblog_key` | string | Reblog key |
| `state` | string | Post state (e.g. `"published"`) |
| `reblogged_from_name` | string? | Blog name this was reblogged from |
| `reblogged_from_url` | string? | URL of the blog this was reblogged from |
| `reblogged_root_name` | string? | Original post's blog name |
| `reblogged_root_url` | string? | Original post's blog URL |
| `liked` | bool | Whether you liked the post |
| `followed` | bool | Whether you follow the blog |

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

- Incremental backups (only fetch posts newer than the last run)
- Video and audio downloading (`--save-video`, `--save-audio`)
- Liked posts backup (`--likes`)
- Tag filtering (`--include-tags`)
- Notes backup (`--save-notes`)
- Index page generation (`--index-file`)
- Automatic rate limit retry with backoff
