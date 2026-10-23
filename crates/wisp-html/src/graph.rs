use crate::model::InputRow;

/// Bounded direct-input provenance sketch (SVG only, no external resources).
pub fn render_provenance_svg(
    section_id: &str,
    version_id: &str,
    inputs: &[InputRow],
    max_nodes: u32,
) -> (String, bool) {
    let take = inputs.len().min(max_nodes as usize);
    let truncated = inputs.len() > take;
    let w = 320;
    let row_h = 28;
    let h = 48 + take * row_h + 24;
    let mut svg = String::new();
    svg.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" role="img" aria-label="provenance graph">
<text x="8" y="18" font-size="12" font-family="system-ui,sans-serif">section {sec}</text>
<text x="8" y="34" font-size="10" fill="{gray}" font-family="system-ui,sans-serif">version {ver}</text>
"#,
        w = w,
        h = h,
        sec = xml_text(section_id),
        ver = xml_text(version_id),
        gray = "#555",
    ));
    let mut y = 48;
    for inp in inputs.iter().take(take) {
        svg.push_str(&format!(
            r#"<line x1="160" y1="40" x2="160" y2="{y}" stroke="{stroke}" stroke-width="1"/>
<rect x="12" y="{y2}" width="296" height="22" rx="4" fill="{fill}" stroke="{border}"/>
<text x="20" y="{y3}" font-size="11" font-family="ui-monospace,monospace">{label}</text>
<text x="20" y="{y4}" font-size="9" fill="{gray}" font-family="system-ui,sans-serif">{state}</text>
"#,
            y = y,
            y2 = y - 16,
            y3 = y - 2,
            y4 = y + 8,
            stroke = "#999",
            fill = "#f6f8fa",
            border = "#ccc",
            gray = "#555",
            label = xml_text(&truncate(&inp.label, 42)),
            state = xml_text(&inp.state),
        ));
        y += row_h;
    }
    if truncated {
        svg.push_str(&format!(
            r#"<text x="8" y="{y}" font-size="10" fill="{red}" font-family="system-ui,sans-serif">… truncated</text>"#,
            y = y + 8,
            red = "#8a1f1f",
        ));
    }
    svg.push_str("</svg>");
    (svg, truncated)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>())
}

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
