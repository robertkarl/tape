//! Tool badges: one emoji per tool, and for bash one per PIPELINE STAGE, so a
//! bash/bash/bash run reads as 🔎🔎🔨🐍 instead of a wall of identical rows.
//! Ported from render_trace.py (which mirrors pppp/src/main.rs).

use serde_json::Value;

pub const EMOJI_SEARCH: &str = "🔎";
pub const EMOJI_CUT: &str = "✂️";
pub const EMOJI_BUILD: &str = "🔨";
pub const EMOJI_PY: &str = "🐍";
pub const EMOJI_NET: &str = "🛜";
pub const EMOJI_LAUNCH: &str = "🚀";
pub const EMOJI_GIT: &str = "🔀";
pub const EMOJI_MKDIR: &str = "📁";
pub const EMOJI_RM: &str = "🗑️";
pub const EMOJI_DOC: &str = "📄";
pub const EMOJI_PERL: &str = "🐫";
pub const EMOJI_TMUX: &str = "🖥️";
pub const EMOJI_KILL: &str = "🔪";
pub const EMOJI_SLEEP: &str = "⏳";
pub const EMOJI_HEREDOC: &str = "⬇️";
pub const EMOJI_WRENCH: &str = "🔧";
pub const EMOJI_READ: &str = "📖";
pub const EMOJI_WRITE: &str = "📝";
pub const EMOJI_EDIT: &str = "✏️";
pub const EMOJI_SAY: &str = "💬";

fn bash_emoji(verb: &str) -> &'static str {
    match verb {
        "ls" | "grep" | "find" | "cat" | "head" | "tail" | "rg" | "which" | "env" | "wc" => {
            EMOJI_SEARCH
        }
        "sed" | "awk" | "cut" | "tr" => EMOJI_CUT,
        "make" | "gcc" | "cc" | "./autogen.sh" | "./configure" | "cmake" | "g++" => EMOJI_BUILD,
        "python3" | "python" | "pytest" => EMOJI_PY,
        "nc" | "curl" | "telnet" | "wget" => EMOJI_NET,
        "nohup" | "memcached" => EMOJI_LAUNCH,
        "git" => EMOJI_GIT,
        "mkdir" => EMOJI_MKDIR,
        "rm" => EMOJI_RM,
        "printf" | "echo" => EMOJI_DOC,
        "perl" => EMOJI_PERL,
        "tmux" => EMOJI_TMUX,
        "pkill" | "kill" | "killall" => EMOJI_KILL,
        "sleep" => EMOJI_SLEEP,
        _ => EMOJI_WRENCH,
    }
}

/// Tools whose "key" is a filesystem path, so the timeline shows the basename
/// rather than a shared prefix that pushes the filename out of the rail.
pub const FILE_TOOLS: &[&str] = &[
    "read",
    "write",
    "write_file",
    "create_file",
    "edit",
    "edit_file",
    "str_replace",
];

fn tokens(cmd: &str) -> Vec<&str> {
    cmd.split_whitespace().collect()
}

/// Leading verb of a bash command, stepping past `cd X &&` and `FOO=bar`.
pub fn bash_verb(cmd: &str) -> String {
    let mut cmd = cmd.trim().to_string();
    if cmd.is_empty() {
        return "(empty)".into();
    }
    let mut first = match tokens(&cmd).first() {
        Some(f) => (*f).to_string(),
        None => return "(empty)".into(),
    };
    if first == "cd" && cmd.contains("&&") {
        let rest = cmd.splitn(2, "&&").nth(1).unwrap_or("").trim().to_string();
        first = tokens(&rest)
            .first()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "cd".into());
        cmd = rest;
    }
    // Step past env-var prefixes (FOO=bar cmd). Python's str.isupper() is True
    // only with at least one cased character, so `=1` alone does not qualify.
    loop {
        let Some((prefix, _)) = first.split_once('=') else { break };
        let letters: Vec<char> = prefix.chars().filter(|c| !c.is_ascii_digit() && *c != '_').collect();
        let all_upper = !letters.is_empty() && letters.iter().all(|c| c.is_uppercase());
        if !all_upper || tokens(&cmd).len() <= 1 {
            break;
        }
        cmd = cmd.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim().to_string();
        first = match tokens(&cmd).first() {
            Some(f) => f.to_string(),
            None => break,
        };
    }
    first
}

/// Split a bash command into stages on |, &&, ; (top level, quote-aware), so
/// `grep x | grep y | sed` -> ["grep x", "grep y", "sed"].
pub fn split_stages(cmd: &str) -> Vec<String> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut stages: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        let next2: String = chars[i..].iter().take(2).collect();
        if let Some(q) = quote {
            buf.push(ch);
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            buf.push(ch);
        } else if ch == '|' && next2 != "||" {
            stages.push(std::mem::take(&mut buf));
        } else if next2 == "&&" || next2 == "||" || next2 == ";;" {
            stages.push(std::mem::take(&mut buf));
            buf = String::new();
            i += 1;
        } else if ch == ';' {
            stages.push(std::mem::take(&mut buf));
        } else {
            buf.push(ch);
        }
        i += 1;
    }
    stages.push(buf);
    stages
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Heredoc redirection (`cmd << 'EOF' ... EOF`) — a distinct, frequently
/// botched pattern, flagged with its own icon regardless of the leading verb.
/// The delimiter must start with a letter/underscore, which excludes the
/// arithmetic left-shift `$((1 << 4))` where a digit follows `<<`.
fn has_heredoc(s: &str) -> bool {
    let b: Vec<char> = s.chars().collect();
    for i in 0..b.len() {
        if b[i] != '<' || i + 1 >= b.len() || b[i + 1] != '<' {
            continue;
        }
        let mut j = i + 2;
        if j < b.len() && b[j] == '-' {
            j += 1;
        }
        while j < b.len() && b[j].is_whitespace() {
            j += 1;
        }
        if j < b.len() && (b[j] == '\'' || b[j] == '"') {
            j += 1;
        }
        if j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == '_') {
            return true;
        }
    }
    false
}

/// (emoji-chain, label) for a bash command, one icon per pipeline stage.
/// `grep x | grep y | sed z` -> ('🔎🔎✂️', 'grep|grep|sed').
///
/// The emoji chain is computed per stage exactly as the original does, but the
/// timeline shows ONE icon (the first): a chain of five wrenches is visual
/// noise, and the verb chain rides along in the label and the row's title.
pub fn bash_chain(cmd: &str) -> (String, String) {
    let mut stages = split_stages(cmd);
    // Drop leading `cd DIR` navigation stages — noise, not an action. A lone
    // `cd` with nothing after it stays, so it isn't lost entirely.
    while stages.len() > 1
        && stages
            .first()
            .map(|s| tokens(s).first() == Some(&"cd"))
            .unwrap_or(false)
    {
        stages.remove(0);
    }
    if stages.is_empty() {
        return (EMOJI_WRENCH.into(), "(empty)".into());
    }
    let mut verbs: Vec<String> = Vec::new();
    let mut emojis: Vec<&str> = Vec::new();
    for s in &stages {
        if has_heredoc(s) {
            verbs.push("heredoc".into());
            emojis.push(EMOJI_HEREDOC);
        } else {
            let v = bash_verb(s);
            verbs.push(v.clone());
            emojis.push(bash_emoji(&v));
        }
    }
    // Cap so a giant one-liner doesn't produce a mile of labels.
    const CAP: usize = 6;
    let mut shown: Vec<String> = verbs.iter().take(CAP).cloned().collect();
    if verbs.len() > CAP {
        shown.push("…".into());
    }
    (emojis[0].to_string(), shown.join("|"))
}

/// (emoji, label) for a tool call. bash varies by its pipeline of verbs.
pub fn tool_icon(name: &str, args: &Value) -> (String, String) {
    let n = name.to_lowercase();
    match n.as_str() {
        "read" => (EMOJI_READ.into(), "read".into()),
        "write" | "write_file" | "create_file" => (EMOJI_WRITE.into(), "write".into()),
        "edit" | "edit_file" | "str_replace" => (EMOJI_EDIT.into(), "edit".into()),
        "bash" | "shell" | "sh" => bash_chain(command_of(args)),
        _ => (EMOJI_WRENCH.into(), n),
    }
}

/// The command text of a shell-ish call. Tools disagree: pi/Claude store a
/// string under `command`, Codex stores an argv array under `cmd`.
pub fn command_of(args: &Value) -> &str {
    match args.get("command") {
        Some(Value::String(s)) => s,
        _ => match args.get("cmd") {
            Some(Value::String(s)) => s,
            _ => "",
        },
    }
}

pub fn guess_lang(name: &str, s: &str) -> &'static str {
    let n = name.to_lowercase();
    if matches!(n.as_str(), "bash" | "shell" | "sh" | "local_shell_call" | "container_call") {
        return "bash";
    }
    if s.contains("#!/usr/bin/perl") || s.contains("use strict") {
        return "perl";
    }
    let lead = s.trim_start();
    if lead.starts_with("def ") || lead.starts_with("import ") || lead.starts_with("from ") {
        return "python";
    }
    if s.contains("#include") || s.contains("int main") || s.contains("static ") || s.contains("void ") {
        return "c";
    }
    if s.starts_with("*** Begin Patch")
        || s.starts_with("diff ")
        || s.starts_with("--- ")
        || s.starts_with("+++ ")
        || s.starts_with("@@")
    {
        return "diff";
    }
    "text"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_verb_steps_past_cd_and_env_prefixes() {
        assert_eq!(bash_verb("cd /tmp && grep -r x ."), "grep");
        assert_eq!(bash_verb("FOO=bar BAR_BAZ=1 python3 x.py"), "python3");
        assert_eq!(bash_verb("cd /tmp"), "cd");
        assert_eq!(bash_verb(""), "(empty)");
        // `=1` has no cased character, so it is not an env prefix.
        assert_eq!(bash_verb("1=2 ls"), "1=2");
    }

    #[test]
    fn stages_are_quote_aware() {
        assert_eq!(
            split_stages("grep 'a | b' | sed -e 's/x/y/'"),
            vec!["grep 'a | b'", "sed -e 's/x/y/'"]
        );
        assert_eq!(split_stages("a || b"), vec!["a", "b"]);
        assert_eq!(split_stages("a && b; c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn chain_varies_by_verb_and_drops_leading_cd() {
        let (emoji, label) = bash_chain("grep x | grep y | sed z");
        assert_eq!(emoji, "🔎");
        assert_eq!(label, "grep|grep|sed");
        let (_, label) = bash_chain("cd /repo && make");
        assert_eq!(label, "make");
        // A heredoc wins over the stage's verb.
        let (emoji, label) = bash_chain("cat > f << EOF\nhi\nEOF");
        assert_eq!(emoji, "⬇️");
        assert_eq!(label, "heredoc");
        // Arithmetic left-shift is not a heredoc.
        assert_eq!(bash_chain("echo $((1 << 4))").1, "echo");
    }

    #[test]
    fn chain_is_capped() {
        let cmd = "ls | ls | ls | ls | ls | ls | ls";
        let label = bash_chain(cmd).1;
        assert_eq!(label.split('|').count(), 7, "six verbs plus the ellipsis");
        assert!(label.ends_with('…'));
    }

    #[test]
    fn icons_per_tool() {
        assert_eq!(tool_icon("Read", &json!({"file_path": "/x"})).0, "📖");
        assert_eq!(tool_icon("Edit", &json!({})).0, "✏️");
        assert_eq!(tool_icon("Write", &json!({})).0, "📝");
        assert_eq!(tool_icon("Bash", &json!({"command": "git push"})).0, "🔀");
        assert_eq!(tool_icon("WebFetch", &json!({})).0, "🔧");
        // Codex stores argv, not a string.
        assert_eq!(command_of(&json!({"cmd": ["ls", "-la"]})), "");
    }

    #[test]
    fn guess_lang_covers_the_three_tools() {
        assert_eq!(guess_lang("bash", "ls"), "bash");
        assert_eq!(guess_lang("read", "def f():\n  pass"), "python");
        assert_eq!(guess_lang("apply_patch", "*** Begin Patch\n--- a"), "diff");
        assert_eq!(guess_lang("read", "hello"), "text");
    }
}
