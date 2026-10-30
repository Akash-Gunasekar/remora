//! Rule-based cleanup, LLM gate and voice commands for Croaker (plan §6.2, §6.4 §3).

use std::collections::HashMap;

const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "er", "erm", "ah", "hmm", "mm"];
const CORRECTIONS: &[&str] = &["no actually", "no wait", "no make that", "make that", "i mean", "scratch that", "sorry i meant"];

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn bare(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

fn is_num(w: &str) -> bool {
    let b = bare(w);
    let digits = b.trim_end_matches("pm").trim_end_matches("am");
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit() || c == ':'))
        || ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"].contains(&b.as_str())
}

/// Decides whether the LLM is worth its latency for this dictation.
pub fn needs_llm(raw: &str, category: &str, mode: &str) -> bool {
    // Terminals never get rewritten: a "corrected" shell command is a wrong shell command.
    if category == "terminal" {
        return false;
    }
    if mode == "enhance" {
        return true;
    }
    if category == "ide_editor" {
        return false;
    }
    let n = norm(raw);
    let words: Vec<&str> = n.split(' ').collect();
    words.iter().any(|w| FILLERS.contains(w) || *w == "like")
        || n.contains("you know")
        || CORRECTIONS.iter().any(|c| n.contains(c))
        || n.contains("bullet") || n.contains("number one") || n.contains("first of all")
        || words.len() > 25
        || category == "email"
}

fn remove_fillers(words: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = bare(&words[i]);
        let next = words.get(i + 1).map(|s| bare(s)).unwrap_or_default();
        let prev = out.last().map(|s| bare(s)).unwrap_or_default();
        if FILLERS.contains(&w.as_str()) {
            carry_punct(&words[i], &mut out);
            i += 1;
            continue;
        }
        if w == "you" && next == "know" && (words[i + 1].ends_with(',') || i + 2 == words.len() || words[i].ends_with(',') || prev.is_empty()) {
            carry_punct(&words[i + 1], &mut out);
            i += 2;
            continue;
        }
        // "like" as filler: before a number, sentence-initial, or after so/and/it's/,
        if w == "like" && (is_num(words.get(i + 1).map(String::as_str).unwrap_or("")) || prev.is_empty()
            || ["so", "and", "it's", "its", "but"].contains(&prev.as_str())
            || out.last().is_some_and(|p| p.ends_with(',')))
        {
            carry_punct(&words[i], &mut out);
            i += 1;
            continue;
        }
        out.push(words[i].clone());
        i += 1;
    }
    out
}

/// Keeps sentence punctuation that was attached to a dropped filler ("so um, yes" → "so, yes").
fn carry_punct(dropped: &str, out: &mut [String]) {
    if let Some(last) = out.last_mut() {
        let p: String = dropped.chars().rev().take_while(|c| ".?!".contains(*c)).collect();
        if !p.is_empty() && !last.ends_with(|c: char| ".?!".contains(c)) {
            last.push_str(&p.chars().rev().collect::<String>());
        }
    }
}

/// "meet at 4, no actually 5" → "meet at 5"; "…, scratch that, …" drops the clause before it.
fn course_correct(words: Vec<String>) -> Vec<String> {
    let lower: Vec<String> = words.iter().map(|w| bare(w)).collect();
    for cue in CORRECTIONS {
        let cw: Vec<&str> = cue.split(' ').collect();
        let Some(pos) = (0..lower.len().saturating_sub(cw.len() - 1)).rev().find(|&i| {
            cw.iter().enumerate().all(|(k, c)| lower[i + k] == *c)
        }) else { continue };
        let before: Vec<String> = words[..pos].to_vec();
        let after: Vec<String> = words[pos + cw.len()..].to_vec();
        let fixed = if *cue == "scratch that" {
            let head = &before[..before.len().saturating_sub(1)];
            let cut = head.iter().rposition(|w| w.ends_with(['.', '?', '!'])).map(|i| i + 1).unwrap_or(0);
            [before[..cut].to_vec(), after].concat()
        } else if after.first().is_some_and(|w| is_num(w)) {
            match before.iter().rposition(|w| is_num(w)) {
                Some(j) => {
                    let mut b = before[..j].to_vec();
                    b.extend(after);
                    b
                }
                None => [before, after].concat(),
            }
        } else {
            // Replace as many trailing words as the correction has before its first clause break.
            let k = after.iter().position(|w| w.ends_with([',', '.', '?', '!'])).map(|i| i + 1).unwrap_or(after.len()).min(3);
            let keep = before.len().saturating_sub(k);
            [before[..keep].to_vec(), after].concat()
        };
        return course_correct(strip_dangling_commas(fixed));
    }
    words
}

fn strip_dangling_commas(mut v: Vec<String>) -> Vec<String> {
    for i in 0..v.len() {
        let next_is_word = v.get(i + 1).is_some();
        if v[i].ends_with(',') && (!next_is_word || v[i + 1].starts_with(',')) {
            v[i].pop();
        }
    }
    v
}

/// Spoken casing grammar for developers: "camel case user id" → userId.
pub fn apply_casing(text: &str) -> String {
    const STOP: &[&str] = &["the", "a", "an", "to", "in", "and", "or", "for", "with", "of", "is", "on", "from"];
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let two = if i + 1 < words.len() { format!("{} {}", bare(words[i]), bare(words[i + 1])) } else { String::new() };
        let style = match two.as_str() {
            "camel case" | "snake case" | "kebab case" | "pascal case" | "all caps" | "screaming snake" => Some(two.clone()),
            _ => None,
        };
        if let Some(style) = style {
            let mut j = i + 2;
            let mut parts: Vec<String> = Vec::new();
            let mut trail = String::new();
            while j < words.len() && parts.len() < 4 && !STOP.contains(&bare(words[j]).as_str()) {
                let b = bare(words[j]);
                if b.is_empty() { break; }
                parts.push(b);
                let w = words[j];
                j += 1;
                if w.ends_with(|c: char| !c.is_alphanumeric()) {
                    trail = w.chars().rev().take_while(|c| !c.is_alphanumeric()).collect::<String>().chars().rev().collect();
                    break;
                }
            }
            if parts.is_empty() {
                out.push(words[i].to_string());
                i += 1;
                continue;
            }
            let cap = |s: &str| s[..1].to_uppercase() + &s[1..];
            let id = match style.as_str() {
                "camel case" => parts[0].clone() + &parts[1..].iter().map(|p| cap(p)).collect::<String>(),
                "pascal case" => parts.iter().map(|p| cap(p)).collect(),
                "snake case" => parts.join("_"),
                "kebab case" => parts.join("-"),
                _ => parts.join("_").to_uppercase(),
            };
            out.push(id + &trail);
            i = j;
        } else {
            out.push(words[i].to_string());
            i += 1;
        }
    }
    out.join(" ")
}

fn spoken_formatting(text: &str) -> String {
    let mut s = format!(" {text} ");
    for (from, to) in [
        (" new paragraph ", "\n\n"), (" new line ", "\n"), (" next line ", "\n"),
        (" bullet point ", "\n- "), (" question mark ", "? "), (" exclamation mark ", "! "), (" full stop ", ". "),
    ] {
        let mut out = String::new();
        let lower = s.to_lowercase();
        let mut last = 0;
        for (i, _) in lower.match_indices(from) {
            if i < last { continue; }
            out.push_str(s[last..i].trim_end_matches([',', ' ']));
            out.push_str(to);
            last = i + from.len();
        }
        out.push_str(&s[last..]);
        s = format!(" {} ", out);
    }
    s.lines().map(|l| l.trim()).collect::<Vec<_>>().join("\n").trim().to_string()
}

fn capitalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    let words: Vec<&str> = text.split(' ').collect();
    for (wi, w) in words.iter().enumerate() {
        if wi > 0 { out.push(' '); }
        let mut cs = w.chars();
        let fixed = if *w == "i" || w.starts_with("i'") { format!("I{}", &w[1..]) } else { w.to_string() };
        let fixed = if start {
            match cs.next() {
                Some(c) if c.is_lowercase() => c.to_uppercase().collect::<String>() + &fixed[c.len_utf8()..],
                _ => fixed,
            }
        } else { fixed };
        start = fixed.ends_with(['.', '?', '!']);
        out.push_str(&fixed);
    }
    out.lines()
        .map(|l| {
            let l = l.trim_start();
            let (prefix, rest) = l.strip_prefix("- ").map(|r| ("- ", r)).unwrap_or(("", l));
            let mut c = rest.chars();
            match c.next() {
                Some(f) => format!("{prefix}{}{}", f.to_uppercase(), c.as_str()),
                None => prefix.trim().to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Full offline cleanup: the rules path and the LLM fallback.
pub fn rules(raw: &str, category: &str, dictionary: &[String], snippets: &HashMap<String, String>) -> String {
    let words: Vec<String> = raw.split_whitespace().map(String::from).collect();
    let words = course_correct(remove_fillers(words));
    let mut text = strip_dangling_commas(words).join(" ");
    text = spoken_formatting(&text);
    let code_like = category == "terminal" || category == "ide_editor" || category.starts_with("ai_agent");
    if code_like {
        text = apply_casing(&text);
    }
    for (trigger, expansion) in snippets {
        text = replace_ci(&text, trigger, expansion);
    }
    for term in dictionary {
        text = replace_ci(&text, term, term);
    }
    if category == "terminal" {
        return text.trim_end_matches(['.', ' ']).to_string();
    }
    if category == "ide_editor" {
        return text;
    }
    let mut text = capitalize(&text);
    let last_line = text.lines().last().unwrap_or("");
    if !last_line.starts_with("- ") && !text.ends_with(['.', '?', '!', ':', '`']) && text.split_whitespace().count() > 2 {
        text.push('.');
    }
    text
}

/// Case-insensitive whole-word replace.
fn replace_ci(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() { return text.to_string(); }
    let lower = text.to_lowercase();
    let pat = from.to_lowercase();
    let mut out = String::new();
    let mut last = 0;
    for (i, _) in lower.match_indices(&pat) {
        if i < last { continue; }
        let before_ok = lower[..i].chars().last().is_none_or(|c| !c.is_alphanumeric());
        let after_ok = lower[i + pat.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok && text.is_char_boundary(i) && text.is_char_boundary(i + pat.len()) {
            out.push_str(&text[last..i]);
            out.push_str(to);
            last = i + pat.len();
        }
    }
    out.push_str(&text[last..]);
    out
}

// ---------- voice commands ----------

#[derive(Debug, PartialEq)]
pub enum Command {
    Undo,
    Enter,
    SelectAll,
    Snippet(String),
}

// ponytail: exact-phrase matcher stands in for Needle3 tool calls; swap in Needle3 once libneedle is linked.
pub fn match_command(raw: &str, snippets: &HashMap<String, String>) -> Option<Command> {
    let n = norm(raw);
    let n = n.strip_prefix("croaker ").unwrap_or(&n);
    match n {
        "delete that" | "undo" | "undo that" | "scratch that" => return Some(Command::Undo),
        "press enter" | "send" | "send it" | "hit enter" => return Some(Command::Enter),
        "select all" => return Some(Command::SelectAll),
        _ => {}
    }
    let key = n.strip_prefix("insert snippet ").or_else(|| n.strip_prefix("insert "))?;
    snippets
        .iter()
        .find(|(k, _)| norm(k) == key)
        .map(|(_, v)| Command::Snippet(v.clone()))
}

pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"), ("ta", "Tamil"), ("hi", "Hindi"), ("kn", "Kannada"), ("te", "Telugu"),
    ("ml", "Malayalam"), ("mr", "Marathi"), ("bn", "Bengali"), ("de", "German"), ("fr", "French"),
    ("es", "Spanish"), ("it", "Italian"), ("nl", "Dutch"), ("pt", "Portuguese"), ("pl", "Polish"),
    ("ja", "Japanese"), ("ko", "Korean"), ("zh", "Chinese (Simplified)"), ("ar", "Arabic"),
];

pub fn language_name(code: &str) -> &'static str {
    LANGUAGES.iter().find(|(c, _)| *c == code).map(|(_, n)| *n).unwrap_or("English")
}

/// "In German: can we move the review" → (Some("de"), "can we move the review").
pub fn language_prefix(raw: &str) -> (Option<&'static str>, String) {
    let t = raw.trim();
    let lower = t.to_lowercase();
    for (code, name) in LANGUAGES {
        for p in [format!("in {}", name.to_lowercase()), format!("translate to {}", name.to_lowercase())] {
            if let Some(rest) = lower.strip_prefix(&p) {
                if rest.starts_with([':', ',', ' ']) {
                    let body = t[p.len()..].trim_start_matches([':', ',', ' ']).to_string();
                    return (Some(code), body);
                }
            }
        }
    }
    (None, t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        rules(s, "general", &[], &HashMap::new())
    }

    #[test]
    fn fillers_and_capitalization() {
        assert_eq!(r("um so i think uh we should ship it"), "So I think we should ship it.");
    }

    #[test]
    fn numeric_course_correction() {
        assert_eq!(r("meet at 4, no actually 5"), "Meet at 5.");
        assert_eq!(r("should be fixed by like 3, no make that 4"), "Should be fixed by 4.");
    }

    #[test]
    fn scratch_that_drops_previous_sentence() {
        assert_eq!(r("I hate this. scratch that, looks good to me"), "Looks good to me.");
    }

    #[test]
    fn spoken_formatting_lines_and_bullets() {
        assert_eq!(r("groceries bullet point milk bullet point eggs"), "Groceries\n- Milk\n- Eggs");
        assert_eq!(r("hello new paragraph world is big"), "Hello\n\nWorld is big.");
    }

    #[test]
    fn casing_grammar() {
        assert_eq!(apply_casing("rename camel case user id to snake case user id"), "rename userId to user_id");
        assert_eq!(apply_casing("set all caps max retries, then go"), "set MAX_RETRIES, then go");
    }

    #[test]
    fn terminal_is_literal() {
        assert_eq!(rules("git status.", "terminal", &[], &HashMap::new()), "git status");
    }

    #[test]
    fn dictionary_and_snippets() {
        let mut s = HashMap::new();
        s.insert("my meeting link".to_string(), "https://meet.example/abc".to_string());
        assert_eq!(rules("join via my meeting link today with sherpa-onnx", "general", &["sherpa-onnx".into()], &s),
            "Join via https://meet.example/abc today with sherpa-onnx.");
    }

    #[test]
    fn gate() {
        assert!(!needs_llm("ship it today", "general", "clean"));
        assert!(needs_llm("um ship it", "general", "clean"));
        assert!(needs_llm("meet at 4 no actually 5", "work_chat", "clean"));
        assert!(!needs_llm("um git status", "terminal", "clean"));
        assert!(needs_llm("fix it", "ai_agent.ide", "enhance"));
        assert!(needs_llm("ship it today", "general", "enhance"));
        assert!(!needs_llm("git status", "terminal", "enhance"));
    }

    #[test]
    fn commands() {
        let mut s = HashMap::new();
        s.insert("sig".to_string(), "— Akash".to_string());
        assert_eq!(match_command("Delete that.", &s), Some(Command::Undo));
        assert_eq!(match_command("Press enter", &s), Some(Command::Enter));
        assert_eq!(match_command("insert snippet sig", &s), Some(Command::Snippet("— Akash".into())));
        assert_eq!(match_command("delete that file please", &s), None);
    }

    #[test]
    fn language_prefix_detection() {
        assert_eq!(language_prefix("In German: can we move it"), (Some("de"), "can we move it".into()));
        assert_eq!(language_prefix("in general it works"), (None, "in general it works".into()));
    }
}
