/// Simple line-oriented diff for section history (plain text).
pub fn line_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = String::new();
    let max = old_lines.len().max(new_lines.len());
    for i in 0..max {
        let o = old_lines.get(i);
        let n = new_lines.get(i);
        match (o, n) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                out.push_str("- ");
                out.push_str(a);
                out.push('\n');
                out.push_str("+ ");
                out.push_str(b);
                out.push('\n');
            }
            (Some(a), None) => {
                out.push_str("- ");
                out.push_str(a);
                out.push('\n');
            }
            (None, Some(b)) => {
                out.push_str("+ ");
                out.push_str(b);
                out.push('\n');
            }
            (None, None) => {}
        }
    }
    out
}
