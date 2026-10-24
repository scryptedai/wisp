//! Post-process markdown HTML for offline `file://` export.

use pulldown_cmark::{CowStr, Event, Tag};

/// Strip dangerous URL schemes and inline handlers from rendered markdown HTML.
pub fn sanitize_markdown_html(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = html.to_string();
    for bad in ["javascript:", "data:", "vbscript:", "file:"] {
        if lower.contains(bad) {
            out = out.replace(bad, "blocked:");
            out = out.replace(&bad.to_ascii_uppercase(), "blocked:");
        }
    }
    for tag in ["<script", "</script", "onerror=", "onclick="] {
        if lower.contains(tag) {
            out = out.replace('<', "&lt;");
            break;
        }
    }
    out
}

pub fn markdown_link_allowed(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') {
        return true;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    if url.starts_with('/') && !url.starts_with("//") {
        return true;
    }
    if !lower.contains(':') {
        return true;
    }
    false
}

fn escape_html_text_owned(s: &str) -> CowStr<'static> {
    let needs = s.contains(['&', '<', '>']);
    if !needs {
        return CowStr::Boxed(s.to_string().into_boxed_str());
    }
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    CowStr::Boxed(out.into_boxed_str())
}

/// Map pulldown events so raw HTML and unsafe links cannot reach the export.
pub fn sanitize_markdown_events<'a, I>(events: I) -> impl Iterator<Item = Event<'a>>
where
    I: Iterator<Item = Event<'a>>,
{
    events.map(|event| match event {
        Event::Html(text) | Event::InlineHtml(text) => {
            Event::Text(escape_html_text_owned(text.as_ref()))
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let dest_url = if markdown_link_allowed(&dest_url) {
                dest_url
            } else {
                CowStr::Borrowed("")
            };
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            })
        }
        other => other,
    })
}
