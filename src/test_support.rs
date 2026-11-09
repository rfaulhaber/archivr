//! Fixtures shared by unit tests across modules.

/// The smallest post JSON that deserializes into a `Post`, with `id_string` set
/// to `id`. Tests mutate the returned value (content, trail, tags, ...) before
/// deserializing it.
pub fn post_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id_string": id,
        "blog_name": "test",
        "post_url": format!("https://test.tumblr.com/post/{id}"),
        "type": "text",
        "timestamp": 1700000000,
        "scheduled_publish_time": 0,
        "note_count": 0,
        "reblog_key": "abc",
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
        "tags": [],
        "layout": []
    })
}
