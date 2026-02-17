//! Jinja templating for post output
//!
//! This module provides the templating infrastructure for formatting Tumblr posts.
//! Users can provide custom Jinja templates or use the built-in default.

use crabrave::handlers::blog::{Post, TrailItem};
use crabrave::npf::ContentBlock;
use minijinja::{Environment, Value, context};

/// Default HTML template for rendering posts
pub const DEFAULT_TEMPLATE: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{{ post.blog_name }} - {{ post.id }}</title>
  <style>
    body { font-family: system-ui, sans-serif; max-width: 650px; margin: 2rem auto; padding: 0 1rem; line-height: 1.6; color: #333; }
    .meta { color: #666; font-size: 0.9rem; margin-bottom: 1rem; }
    .meta a { color: #0066cc; text-decoration: none; }
    .meta a:hover { text-decoration: underline; }
    .tags { margin-top: 1rem; }
    .tag { display: inline-block; background: #f0f0f0; padding: 0.2rem 0.5rem; margin: 0.2rem; border-radius: 3px; font-size: 0.85rem; color: #555; }
    .content-block { margin: 1rem 0; }
    .content-block img { max-width: 100%; height: auto; display: block; }
    .content-block.heading1 { font-size: 1.5rem; font-weight: bold; }
    .content-block.heading2 { font-size: 1.25rem; font-weight: bold; }
    .content-block.quote { border-left: 3px solid #ccc; padding-left: 1rem; font-style: italic; color: #555; }
    .content-block.indented { margin-left: 2rem; }
    .content-block.chat { font-family: monospace; background: #f5f5f5; padding: 0.5rem; }
    .trail { margin-bottom: 1.5rem; }
    .trail-item { border-left: 3px solid #ddd; padding-left: 1rem; margin: 1rem 0; }
    .trail-author { font-weight: bold; margin-bottom: 0.5rem; color: #0066cc; }
    .video-embed, .audio-embed { margin: 1rem 0; }
    .video-embed video { max-width: 100%; }
    .audio-info { margin-bottom: 0.5rem; }
    .audio-title { font-weight: bold; }
    .audio-artist { color: #666; }
    .link-block { border: 1px solid #ddd; padding: 1rem; border-radius: 4px; background: #fafafa; }
    .link-block a { font-weight: bold; color: #0066cc; text-decoration: none; }
    .link-block a:hover { text-decoration: underline; }
    .link-block p { margin: 0.5rem 0 0 0; color: #666; }
    .poll { border: 1px solid #ddd; padding: 1rem; border-radius: 4px; background: #fafafa; }
    .poll-question { font-weight: bold; margin-bottom: 0.75rem; }
    .poll-answers { list-style: none; padding: 0; margin: 0; }
    .poll-answers li { background: #e9e9e9; padding: 0.5rem 0.75rem; margin: 0.3rem 0; border-radius: 3px; }
    .poll-meta { font-size: 0.85rem; color: #888; margin-top: 0.5rem; }
    .unsupported-block { color: #999; font-style: italic; }
    figure { margin: 1rem 0; }
    figcaption { font-size: 0.9rem; color: #666; margin-top: 0.5rem; }
    .post-nav { display: flex; justify-content: space-between; margin-top: 2rem; padding-top: 1rem; border-top: 1px solid #ddd; }
    .post-nav a { color: #0066cc; text-decoration: none; }
    .post-nav a:hover { text-decoration: underline; }
    .post-nav:not(:has(a)) { display: none; }
  </style>
</head>
<body>
  <article>
    <header class="meta">
      <a href="{{ post.post_url }}">{{ post.blog_name }}</a>
      {%- if post.date %} &middot; {{ post.date }}{% endif %}
      {%- if post.note_count %} &middot; {{ post.note_count }} notes{% endif %}
    </header>

    {%- if post.trail %}
    <div class="trail">
      {%- for item in post.trail %}
      <div class="trail-item">
        {%- if item.blog and item.blog.name %}
        <div class="trail-author">{{ item.blog.name }}</div>
        {%- endif %}
        {%- for block in item.content %}
        {{ render_block(block) }}
        {%- endfor %}
      </div>
      {%- endfor %}
    </div>
    {%- endif %}

    <div class="content">
      {%- for block in post.content %}
      {{ render_block(block) }}
      {%- endfor %}
    </div>

    {%- if post.tags %}
    <div class="tags">
      {%- for tag in post.tags %}
      <span class="tag">#{{ tag }}</span>
      {%- endfor %}
    </div>
    {%- endif %}
  </article>
  <nav class="post-nav">
    {%- if newer_href %}<a href="{{ newer_href }}">&larr; Newer</a>{%- else %}<span></span>{%- endif %}
    <!-- ARCHIVR:OLDER_NAV -->
  </nav>
</body>
</html>
"##;

/// HTML comment placeholder inserted by the template, replaced after the next post is known
pub const OLDER_NAV_PLACEHOLDER: &str = "<!-- ARCHIVR:OLDER_NAV -->";

/// Builds the HTML for an "Older" navigation link, to be substituted for [`OLDER_NAV_PLACEHOLDER`].
pub fn build_older_nav_link(href: &str) -> String {
    format!(r#"<a href="{href}">Older &rarr;</a>"#)
}

/// Renders a single ContentBlock to HTML
///
/// This function can be used for programmatic rendering outside of templates.
pub fn render_content_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text, subtype, .. } => {
            let class = subtype.as_deref().unwrap_or("");
            format!(r#"<div class="content-block {class}">{text}</div>"#)
        }
        ContentBlock::Image {
            media,
            alt_text,
            caption,
            ..
        } => {
            let mut html = String::new();
            let Some(m) = media.first() else {
                return html;
            };
            let alt = alt_text.as_deref().unwrap_or("");
            let width_attr = m
                .width
                .map(|w| format!(r#" width="{w}""#))
                .unwrap_or_default();
            html.push_str(&format!(
                r#"<figure class="content-block image"><img src="{url}" alt="{alt}"{width_attr}>"#,
                url = m.url
            ));
            if let Some(cap) = caption {
                html.push_str(&format!("<figcaption>{cap}</figcaption>"));
            }
            html.push_str("</figure>");
            html
        }
        ContentBlock::Video {
            media,
            url,
            embed_html,
            duration,
            ..
        } => {
            let mut html = String::from(r#"<div class="content-block video-embed">"#);
            if let Some(embed) = embed_html {
                html.push_str(embed);
            } else if let Some(media_list) = media {
                for m in media_list {
                    let width_attr = m
                        .width
                        .map(|w| format!(r#" width="{w}""#))
                        .unwrap_or_default();
                    let duration_str = duration
                        .map(|d| format!(" ({}s)", d as i64))
                        .unwrap_or_default();
                    html.push_str(&format!(
                        r#"<video controls src="{url}"{width_attr}></video>{duration_str}"#,
                        url = m.url
                    ));
                }
            } else if let Some(video_url) = url {
                html.push_str(&format!(r#"<a href="{video_url}">Video link</a>"#));
            }
            html.push_str("</div>");
            html
        }
        ContentBlock::Audio {
            media,
            url,
            title,
            artist,
            album,
            embed_html,
            ..
        } => {
            let mut html = String::from(r#"<div class="content-block audio-embed">"#);

            // Audio metadata
            if title.is_some() || artist.is_some() || album.is_some() {
                html.push_str(r#"<div class="audio-info">"#);
                if let Some(t) = title {
                    html.push_str(&format!(r#"<span class="audio-title">{t}</span>"#));
                }
                if let Some(a) = artist {
                    html.push_str(&format!(r#" <span class="audio-artist">by {a}</span>"#));
                }
                if let Some(alb) = album {
                    html.push_str(&format!(r#" <span class="audio-album">({alb})</span>"#));
                }
                html.push_str("</div>");
            }

            if let Some(embed) = embed_html {
                html.push_str(embed);
            } else if let Some(media_object) = media {
                html.push_str(&format!(
                    r#"<audio controls src="{}"></audio>"#,
                    media_object.url
                ));
            } else if let Some(audio_url) = url {
                html.push_str(&format!(r#"<a href="{audio_url}">Audio link</a>"#));
            }
            html.push_str("</div>");
            html
        }
        ContentBlock::Link {
            url,
            title,
            description,
            ..
        } => {
            let display_title = title.as_deref().unwrap_or(url);
            let mut html = format!(
                r#"<div class="content-block link-block"><a href="{url}">{display_title}</a>"#
            );
            if let Some(desc) = description {
                html.push_str(&format!("<p>{desc}</p>"));
            }
            html.push_str("</div>");
            html
        }
        ContentBlock::Paywall { text, .. } => {
            let msg = text.as_deref().unwrap_or("Premium content");
            format!(r#"<div class="content-block paywall">{msg}</div>"#)
        }
        ContentBlock::Poll {
            question,
            answers,
            settings,
            ..
        } => {
            let mut html = String::from(r#"<div class="content-block poll">"#);
            html.push_str(&format!(r#"<div class="poll-question">{question}</div>"#));
            html.push_str(r#"<ul class="poll-answers">"#);
            for answer in answers {
                html.push_str(&format!("<li>{}</li>", answer.answer_text));
            }
            html.push_str("</ul>");
            if let Some(s) = settings {
                let mut meta_parts: Vec<String> = Vec::new();
                if s.multiple_choice {
                    meta_parts.push("Multiple choice".to_string());
                }
                if let Some(status) = &s.close_status {
                    meta_parts.push(status.clone());
                }
                if !meta_parts.is_empty() {
                    html.push_str(&format!(
                        r#"<div class="poll-meta">{}</div>"#,
                        meta_parts.join(" · ")
                    ));
                }
            }
            html.push_str("</div>");
            html
        }
        ContentBlock::Unknown | _ => {
            r#"<div class="content-block unsupported-block">Unsupported block type</div>"#
                .to_string()
        }
    }
}

/// Converts a Post to a minijinja Value for template rendering
fn post_to_value(post: &Post) -> Value {
    // Convert content blocks to a serializable format
    let content: Vec<Value> = post.content.iter().map(content_block_to_value).collect();
    let trail: Vec<Value> = post.trail.iter().map(trail_item_to_value).collect();
    let tags: Vec<Value> = post.tags.iter().map(|t| Value::from(t.as_str())).collect();

    context! {
        id => post.id,
        blog_name => post.blog_name,
        post_url => post.post_url,
        post_type => post.post_type,
        original_type => post.original_type,
        timestamp => post.timestamp,
        date => post.date,
        content => content,
        trail => trail,
        tags => tags,
        summary => post.summary,
        note_count => post.note_count,
        slug => post.slug,
        short_url => post.short_url,
        reblog_key => post.reblog_key,
        state => post.state,
        reblogged_from_name => post.reblogged_from_name,
        reblogged_from_url => post.reblogged_from_url,
        reblogged_root_name => post.reblogged_root_name,
        reblogged_root_url => post.reblogged_root_url,
        liked => post.liked,
        followed => post.followed,
    }
}

/// Converts a ContentBlock to a minijinja Value
fn content_block_to_value(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text { text, subtype, .. } => {
            context! {
                type => "text",
                text => text,
                subtype => subtype,
            }
        }
        ContentBlock::Image {
            media,
            alt_text,
            caption,
            ..
        } => {
            let Some(m) = media
                .iter()
                .find(|obj| matches!(obj.has_original_dimensions, Some(true)))
                .or_else(|| media.first())
            else {
                return context! {};
            };

            let media_values = vec![context! {
                url => m.url,
                width => m.width,
                height => m.height,
                media_type => m.media_type,
            }];
            context! {
                type => "image",
                media => media_values,
                alt_text => alt_text,
                caption => caption,
            }
        }
        ContentBlock::Video {
            media,
            url,
            provider,
            embed_html,
            duration,
            ..
        } => {
            let media_values: Vec<Value> = media
                .as_ref()
                .map(|m| {
                    m.iter()
                        .map(|media_obj| {
                            context! {
                                url => media_obj.url,
                                width => media_obj.width,
                                height => media_obj.height,
                                media_type => media_obj.media_type,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            context! {
                type => "video",
                media => media_values,
                url => url,
                provider => provider,
                embed_html => embed_html,
                duration => duration,
            }
        }
        ContentBlock::Audio {
            media,
            url,
            provider,
            artist,
            album,
            title,
            embed_html,
            ..
        } => {
            let media_values: Value = media
                .as_ref()
                .map(|m| {
                    context! {
                        url => m.url,
                        media_type => m.media_type,
                    }
                })
                .unwrap_or_default();
            context! {
                type => "audio",
                media => media_values,
                url => url,
                provider => provider,
                artist => artist,
                album => album,
                title => title,
                embed_html => embed_html,
            }
        }
        ContentBlock::Link {
            url,
            title,
            description,
            ..
        } => {
            context! {
                type => "link",
                url => url,
                title => title,
                description => description,
            }
        }
        ContentBlock::Paywall { text, .. } => {
            context! {
                type => "paywall",
                text => text,
            }
        }
        ContentBlock::Poll {
            client_id,
            question,
            answers,
            settings,
            created_at,
            timestamp,
        } => {
            let answer_values: Vec<Value> = answers
                .iter()
                .map(|a| {
                    context! {
                        client_id => a.client_id,
                        answer_text => a.answer_text,
                    }
                })
                .collect();
            let settings_value = settings.as_ref().map(|s| {
                context! {
                    multiple_choice => s.multiple_choice,
                    close_status => s.close_status,
                    expire_after => s.expire_after,
                    source => s.source,
                }
            });
            context! {
                type => "poll",
                client_id => client_id,
                question => question,
                answers => answer_values,
                settings => settings_value,
                created_at => created_at,
                timestamp => timestamp,
            }
        }
        ContentBlock::Unknown | _ => {
            context! {
                type => "unknown",
            }
        }
    }
}

/// Converts a TrailItem to a minijinja Value
fn trail_item_to_value(item: &TrailItem) -> Value {
    let content: Vec<Value> = item.content.iter().map(content_block_to_value).collect();

    let blog = item.blog.as_ref().map(|b| {
        context! {
            name => b.name,
            url => b.url,
            uuid => b.uuid,
        }
    });

    let post = item.post.as_ref().map(|p| {
        context! {
            id => p.id,
        }
    });

    context! {
        content => content,
        blog => blog,
        post => post,
        is_root_item => item.is_root_item,
    }
}

/// Template renderer for Tumblr posts
pub struct PostRenderer<'a> {
    env: Environment<'a>,
    template_name: &'static str,
}

impl<'a> PostRenderer<'a> {
    /// Creates a new renderer with the default template
    pub fn new() -> Self {
        Self::with_template(DEFAULT_TEMPLATE)
    }

    /// Creates a new renderer with a custom template string
    pub fn with_template(template_source: &'static str) -> Self {
        let mut env = Environment::new();

        // Add the render_block function
        env.add_function("render_block", |block: Value| -> String {
            render_block_from_value(&block)
        });

        env.add_template("post", template_source)
            .unwrap_or_else(|e| {
                log::error!("Failed to add template: {}", e);
            });

        Self {
            env,
            template_name: "post",
        }
    }

    /// Creates a new renderer loading a template from a file
    pub fn from_file(path: &camino::Utf8Path) -> anyhow::Result<Self> {
        let template_source = fs_err::read_to_string(path)?;
        // We need to leak the string to get a 'static lifetime
        // This is acceptable since templates are typically loaded once
        let leaked: &'static str = Box::leak(template_source.into_boxed_str());
        Ok(Self::with_template(leaked))
    }

    /// Renders a post to HTML
    ///
    /// `newer_href` is an optional relative link to the next-newer post for navigation.
    pub fn render(&self, post: &Post, newer_href: Option<&str>) -> anyhow::Result<String> {
        let template = self.env.get_template(self.template_name)?;
        let post_value = post_to_value(post);
        let ctx = context! {
            post => post_value,
            is_reblog => post.reblogged_from_name.is_some(),
            is_original => post.reblogged_from_name.is_none(),
            newer_href => newer_href,
        };
        let result = template.render(ctx)?;
        Ok(result)
    }
}

impl Default for PostRenderer<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Renders a block from a minijinja Value (used by the render_block template function)
fn render_block_from_value(block: &Value) -> String {
    let block_type = block
        .get_attr("type")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    match block_type.as_str() {
        "text" => {
            let text = block
                .get_attr("text")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let subtype = block
                .get_attr("subtype")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            format!(r#"<div class="content-block {subtype}">{text}</div>"#)
        }
        "image" => {
            let mut html = String::new();
            if let Ok(media) = block.get_attr("media")
                && let Some(len) = media.len()
            {
                for i in 0..len {
                    if let Ok(m) = media.get_item(&Value::from(i)) {
                        let url = m
                            .get_attr("url")
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let alt = block
                            .get_attr("alt_text")
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let width = m
                            .get_attr("width")
                            .ok()
                            .and_then(|v| v.as_i64())
                            .map(|w| format!(r#" width="{w}""#))
                            .unwrap_or_default();
                        html.push_str(&format!(
                            r#"<figure class="content-block image"><img src="{url}" alt="{alt}"{width}>"#
                        ));
                        if let Ok(caption) = block.get_attr("caption")
                            && let Some(cap) = caption.as_str()
                        {
                            html.push_str(&format!("<figcaption>{cap}</figcaption>"));
                        }
                        html.push_str("</figure>");
                    }
                }
            }
            html
        }
        "video" => {
            let mut html = String::from(r#"<div class="content-block video-embed">"#);
            if let Ok(embed) = block.get_attr("embed_html")
                && let Some(embed_str) = embed.as_str()
            {
                html.push_str(embed_str);
                html.push_str("</div>");
                return html;
            }
            if let Ok(media) = block.get_attr("media")
                && let Some(len) = media.len()
            {
                for i in 0..len {
                    if let Ok(m) = media.get_item(&Value::from(i)) {
                        let url = m
                            .get_attr("url")
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        let width = m
                            .get_attr("width")
                            .ok()
                            .and_then(|v| v.as_i64())
                            .map(|w| format!(r#" width="{w}""#))
                            .unwrap_or_default();
                        html.push_str(&format!(
                            r#"<video controls src="{url}"{width}></video>"#
                        ));
                    }
                }
            } else if let Ok(url) = block.get_attr("url")
                && let Some(url_str) = url.as_str()
            {
                html.push_str(&format!(r#"<a href="{url_str}">Video link</a>"#));
            }
            html.push_str("</div>");
            html
        }
        "audio" => {
            let mut html = String::from(r#"<div class="content-block audio-embed">"#);

            let title = block
                .get_attr("title")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            let artist = block
                .get_attr("artist")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            let album = block
                .get_attr("album")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()));

            if title.is_some() || artist.is_some() || album.is_some() {
                html.push_str(r#"<div class="audio-info">"#);
                if let Some(t) = &title {
                    html.push_str(&format!(r#"<span class="audio-title">{t}</span>"#));
                }
                if let Some(a) = &artist {
                    html.push_str(&format!(r#" <span class="audio-artist">by {a}</span>"#));
                }
                if let Some(alb) = &album {
                    html.push_str(&format!(r#" <span class="audio-album">({alb})</span>"#));
                }
                html.push_str("</div>");
            }

            if let Ok(embed) = block.get_attr("embed_html")
                && let Some(embed_str) = embed.as_str()
            {
                html.push_str(embed_str);
                html.push_str("</div>");
                return html;
            }
            if let Ok(media) = block.get_attr("media")
                && let Some(len) = media.len()
            {
                for i in 0..len {
                    if let Ok(m) = media.get_item(&Value::from(i)) {
                        let url = m
                            .get_attr("url")
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        html.push_str(&format!(r#"<audio controls src="{url}"></audio>"#));
                    }
                }
            } else if let Ok(url) = block.get_attr("url")
                && let Some(url_str) = url.as_str()
            {
                html.push_str(&format!(r#"<a href="{url_str}">Audio link</a>"#));
            }
            html.push_str("</div>");
            html
        }
        "link" => {
            let url = block
                .get_attr("url")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let title = block
                .get_attr("title")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| url.clone());
            let mut html =
                format!(r#"<div class="content-block link-block"><a href="{url}">{title}</a>"#);
            if let Ok(desc) = block.get_attr("description")
                && let Some(desc_str) = desc.as_str()
            {
                html.push_str(&format!("<p>{desc_str}</p>"));
            }
            html.push_str("</div>");
            html
        }
        "paywall" => {
            let text = block
                .get_attr("text")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "Premium content".to_string());
            format!(r#"<div class="content-block paywall">{text}</div>"#)
        }
        "poll" => {
            let question = block
                .get_attr("question")
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default();
            let mut html = String::from(r#"<div class="content-block poll">"#);
            html.push_str(&format!(r#"<div class="poll-question">{question}</div>"#));
            html.push_str(r#"<ul class="poll-answers">"#);
            if let Ok(answers) = block.get_attr("answers")
                && let Some(len) = answers.len()
            {
                for i in 0..len {
                    if let Ok(a) = answers.get_item(&Value::from(i)) {
                        let text = a
                            .get_attr("answer_text")
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_default();
                        html.push_str(&format!("<li>{text}</li>"));
                    }
                }
            }
            html.push_str("</ul>");
            if let Ok(settings) = block.get_attr("settings") {
                let mut meta_parts: Vec<String> = Vec::new();
                if let Ok(mc) = settings.get_attr("multiple_choice")
                    && mc.is_true()
                {
                    meta_parts.push("Multiple choice".to_string());
                }
                if let Ok(status) = settings.get_attr("close_status")
                    && let Some(s) = status.as_str()
                {
                    meta_parts.push(s.to_string());
                }
                if !meta_parts.is_empty() {
                    html.push_str(&format!(
                        r#"<div class="poll-meta">{}</div>"#,
                        meta_parts.join(" · ")
                    ));
                }
            }
            html.push_str("</div>");
            html
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_text_block() {
        let block = ContentBlock::Text {
            text: "Hello, world!".to_string(),
            subtype: None,
            formatting: None,
        };
        let html = render_content_block(&block);
        assert!(html.contains("Hello, world!"));
        assert!(html.contains("content-block"));
    }

    #[test]
    fn test_render_text_block_with_subtype() {
        let block = ContentBlock::Text {
            text: "A heading".to_string(),
            subtype: Some("heading1".to_string()),
            formatting: None,
        };
        let html = render_content_block(&block);
        assert!(html.contains("heading1"));
        assert!(html.contains("A heading"));
    }

    #[test]
    fn test_post_renderer_default() {
        let renderer = PostRenderer::new();
        assert_eq!(renderer.template_name, "post");
    }
}
