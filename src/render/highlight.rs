//! The dependency-free highlighter, ported from render_trace.py's `highlight`.
//!
//! The original is one `re.sub` with an ordered alternation
//! (strings → comments → numbers → keywords → flags) over ALREADY-ESCAPED text.
//! Order is the whole design: strings and comments must win before keywords, or
//! `if` inside a string gets coloured. Rather than add a regex crate, this is a
//! single left-to-right scanner with the same precedence and the same quirk —
//! it tokenizes escaped text, so string delimiters are `&quot;` / `&#x27;`.

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "do", "done", "case", "esac", "return",
    "def", "class", "import", "from", "use", "my", "sub", "int", "char", "void", "static", "const",
    "struct", "typedef", "include", "define", "null", "NULL", "true", "false", "and", "or", "not",
    "in",
];

pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `text` is RAW; the result is escaped HTML with `tok-*` spans.
pub fn highlight(text: &str, lang: &str) -> String {
    let esc_text = esc(text);
    if lang == "text" || lang == "output" || lang.is_empty() {
        return esc_text;
    }
    tokenize(&esc_text)
}

fn span(cls: &str, body: &str) -> String {
    format!("<span class=\"tok-{cls}\">{body}</span>")
}

fn starts_at(b: &[char], i: usize, pat: &str) -> bool {
    let pc: Vec<char> = pat.chars().collect();
    i + pc.len() <= b.len() && b[i..i + pc.len()] == pc[..]
}

fn find_from(b: &[char], from: usize, pat: &str) -> Option<usize> {
    let pc: Vec<char> = pat.chars().collect();
    if from + pc.len() > b.len() {
        return None;
    }
    (from..=(b.len() - pc.len())).find(|&j| b[j..j + pc.len()] == pc[..])
}

fn tokenize(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let n = b.len();
    let mut out = String::with_capacity(s.len() + 256);
    let mut i = 0usize;

    let prev = |i: usize| -> Option<char> { if i == 0 { None } else { Some(b[i - 1]) } };

    while i < n {
        let c = b[i];

        // 1. strings — delimited by ESCAPED quotes, since we tokenize escaped
        //    text. Lazy to the next closing delimiter, so a body containing
        //    other entities does not terminate the string early.
        let mut consumed = false;
        for (open, close, cls) in [
            ("&quot;", "&quot;", "str"),
            ("&#x27;", "&#x27;", "str"),
        ] {
            if starts_at(&b, i, open) {
                let olen = open.chars().count();
                if let Some(end) = find_from(&b, i + olen, close) {
                    let take = end + close.chars().count() - i;
                    out.push_str(&span(cls, &b[i..i + take].iter().collect::<String>()));
                    i += take;
                    consumed = true;
                    break;
                }
            }
        }
        if consumed {
            continue;
        }
        if c == '`' {
            if let Some(end) = find_from(&b, i + 1, "`") {
                let take = end + 1 - i;
                out.push_str(&span("str", &b[i..i + take].iter().collect::<String>()));
                i += take;
                continue;
            }
        }

        // 2. comments — to end of line, or a lazy /* */ block.
        if c == '#' || starts_at(&b, i, "//") || starts_at(&b, i, "/*") {
            if starts_at(&b, i, "/*") {
                if let Some(end) = find_from(&b, i + 2, "*/") {
                    let take = end + 2 - i;
                    out.push_str(&span("com", &b[i..i + take].iter().collect::<String>()));
                    i += take;
                    continue;
                }
            } else {
                let mut j = i;
                while j < n && b[j] != '\n' {
                    j += 1;
                }
                out.push_str(&span("com", &b[i..j].iter().collect::<String>()));
                i = j;
                continue;
            }
        }

        // 3. numbers — \b\d+\b, so digits glued to letters (an entity body, a
        //    variable name) are left alone.
        if c.is_ascii_digit() && !prev(i).map(is_word).unwrap_or(false) {
            let mut j = i;
            while j < n && b[j].is_ascii_digit() {
                j += 1;
            }
            if !b.get(j).map(|c| is_word(*c)).unwrap_or(false) {
                out.push_str(&span("num", &b[i..j].iter().collect::<String>()));
                i = j;
                continue;
            }
        }

        // 4. keywords — a whole word only.
        if c.is_ascii_alphabetic() && !prev(i).map(is_word).unwrap_or(false) {
            let mut j = i;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == '_') {
                j += 1;
            }
            let word: String = b[i..j].iter().collect();
            if KEYWORDS.contains(&word.as_str()) && !b.get(j).map(|c| is_word(*c)).unwrap_or(false) {
                out.push_str(&span("kw", &word));
                i = j;
                continue;
            }
        }

        // 5. flags — `-{1,2}name`, but only after whitespace (the original's
        //    lookbehind), so `a-b` is not a flag.
        if c == '-'
            && prev(i).map(|p| p.is_whitespace()).unwrap_or(true)
            && (b.get(i + 1) == Some(&'-') || b.get(i + 1).map(|x| x.is_ascii_alphabetic()).unwrap_or(false))
        {
            let mut j = i;
            while b.get(j) == Some(&'-') {
                j += 1;
            }
            if b.get(j).map(|x| x.is_ascii_alphabetic()).unwrap_or(false) {
                while j < n && (b[j].is_ascii_alphanumeric() || b[j] == '-' || b[j] == '_') {
                    j += 1;
                }
                out.push_str(&span("flag", &b[i..j].iter().collect::<String>()));
                i = j;
                continue;
            }
        }

        out.push(c);
        i += 1;
    }
    out
}

/// Codex applies patches as TEXT (`*** Begin Patch`), which the original
/// renderer had no notion of. Colour the +/- lines so a patch reads as a diff
/// rather than a wall of grey.
pub fn patch(text: &str) -> String {
    let mut out = String::new();
    for (k, line) in text.split('\n').enumerate() {
        if k > 0 {
            out.push('\n');
        }
        let e = esc(line);
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("*** Update File")
            || line.starts_with("*** Add File") || line.starts_with("*** Delete File")
        {
            out.push_str(&format!("<span class=\"pl-file\">{e}</span>"));
        } else if line.starts_with('+') {
            out.push_str(&format!("<span class=\"pl-add\">{e}</span>"));
        } else if line.starts_with('-') {
            out.push_str(&format!("<span class=\"pl-del\">{e}</span>"));
        } else if line.starts_with("@@") || line.starts_with("***") {
            out.push_str(&format!("<span class=\"pl-hunk\">{e}</span>"));
        } else {
            out.push_str(&e);
        }
    }
    out
}

/// Is this text a patch worth colouring?
pub fn looks_like_patch(s: &str) -> bool {
    let lead = s.trim_start();
    lead.starts_with("*** Begin Patch")
        || (lead.starts_with("--- ") && lead.contains("+++ "))
        || (lead.starts_with("@@") && (lead.contains("\n+") || lead.contains("\n-")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_all_five() {
        assert_eq!(
            esc(r#"<a href="x">&'z"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#x27;z"
        );
    }

    #[test]
    fn strings_win_over_keywords_inside_them() {
        // The whole point of the alternation order.
        let h = highlight("echo \"if this then that\"", "bash");
        assert!(h.contains("<span class=\"tok-str\">&quot;if this then that&quot;</span>"));
        assert!(!h.contains("tok-kw"), "keywords inside a string stay put");
    }

    #[test]
    fn comments_win_to_end_of_line() {
        let h = highlight("ls # for if true", "bash");
        assert!(h.contains("<span class=\"tok-com\"># for if true</span>"));
        assert!(!h.contains("tok-kw"));
    }

    #[test]
    fn keywords_numbers_and_flags_are_touched() {
        let h = highlight("if x 42 --force", "bash");
        assert!(h.contains("<span class=\"tok-kw\">if</span>"));
        assert!(h.contains("<span class=\"tok-num\">42</span>"));
        assert!(h.contains("<span class=\"tok-flag\">--force</span>"));
    }

    #[test]
    fn word_boundaries_hold() {
        // `if` inside `diff` and digits inside an entity must not be coloured.
        let h = highlight("diff &#x27;x&#x27;", "bash");
        assert!(!h.contains("tok-kw"), "diff is not if");
        assert!(!h.contains("tok-num"), "27 inside an entity is not a number");
        // `a-b` is not a flag: the original's lookbehind wants whitespace.
        let h = highlight("a-b", "bash");
        assert!(!h.contains("tok-flag"), "a-b is not a flag");
        let h = highlight("ls -f", "bash");
        assert!(h.contains("<span class=\"tok-flag\">-f</span>"));
    }

    #[test]
    fn text_and_output_are_escaped_only() {
        assert_eq!(highlight("<b>if</b>", "text"), "&lt;b&gt;if&lt;/b&gt;");
    }

    #[test]
    fn patch_lines_are_classified() {
        let p = patch("*** Begin Patch\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-old\n+new");
        assert!(p.contains("pl-file"));
        assert!(p.contains("<span class=\"pl-add\">+new</span>"));
        assert!(p.contains("<span class=\"pl-del\">-old</span>"));
        assert!(looks_like_patch("*** Begin Patch\n-x"));
        assert!(!looks_like_patch("just output"));
    }
}
