//! Post-process markdown HTML for offline `file://` export.

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
    // pulldown with default options does not emit raw `<script>`; belt-and-suspenders.
    for tag in ["<script", "</script", "onerror=", "onclick="] {
        if lower.contains(tag) {
            out = out.replace('<', "&lt;");
            break;
        }
    }
    out
}
