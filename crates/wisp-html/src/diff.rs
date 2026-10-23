use similar::{ChangeTag, TextDiff};

/// Line-oriented diff for section history (plain text).
pub fn line_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();
    for change in diff.iter_all_changes() {
        let (prefix, line) = match change.tag() {
            ChangeTag::Equal => continue,
            ChangeTag::Delete => ("- ", change.value()),
            ChangeTag::Insert => ("+ ", change.value()),
        };
        out.push_str(prefix);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Word-level diff (whitespace tokens) for section history.
pub fn word_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_words: Vec<&str> = old.split_whitespace().collect();
    let new_words: Vec<&str> = new.split_whitespace().collect();
    let mut out = String::new();
    let max = old_words.len().max(new_words.len());
    for i in 0..max {
        match (old_words.get(i), new_words.get(i)) {
            (Some(a), Some(b)) if a == b => {
                out.push_str(b);
                out.push(' ');
            }
            (Some(a), Some(b)) => {
                out.push_str(&format!("[-{a}-] {{+{b}+}} "));
            }
            (Some(a), None) => {
                out.push_str(&format!("[-{a}-] "));
            }
            (None, Some(b)) => {
                out.push_str(&format!("{{+{b}+}} "));
            }
            (None, None) => {}
        }
    }
    out.trim().to_string()
}
