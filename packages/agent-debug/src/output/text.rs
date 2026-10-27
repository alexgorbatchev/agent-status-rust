use regex::Regex;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

static ESCAPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\x1b\][^\x07\x1b\x{9c}\r\n]*(?:\x07|\x1b\\|\x{9c}|$)",
        r"|\x{9d}[^\x07\x1b\x{9c}\r\n]*(?:\x07|\x1b\\|\x{9c}|$)",
        r"|(?:\x1b[P_X^]|[\x{90}\x{98}\x{9e}\x{9f}])[^\x07\x1b\x{9c}\r\n]*(?:\x07|\x1b\\|\x{9c}|$)",
        r"|(?:\x1b\[|\x{9b})[0-9:;<=>?]*[ -/]*[@-~]",
        r"|\x1b[()+*\-./][0-~]|\x1b[@-Z\\-_=><78c]|\x1b"
    ))
    .expect("fixed terminal escape pattern")
});
pub(crate) fn strip_ansi(value: &str) -> String {
    ESCAPE.replace_all(value, "").into_owned()
}
pub fn sanitize(value: &str) -> String {
    strip_ansi(value)
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}
pub fn visual_width(value: &str) -> usize {
    UnicodeWidthStr::width(strip_ansi(value).as_str())
}
fn plain_prefix(value: &str, limit: usize) -> String {
    let mut width = 0;
    value
        .graphemes(true)
        .take_while(|g| {
            width += UnicodeWidthStr::width(*g);
            width <= limit
        })
        .collect()
}
pub fn truncate(value: &str, limit: usize, tail: &str) -> String {
    if limit == 0 {
        return String::new();
    }
    if visual_width(value) <= limit {
        return value.into();
    }
    let tail_width = visual_width(tail);
    if tail_width > limit {
        return plain_prefix(&strip_ansi(tail), limit);
    }
    let available = limit - tail_width;
    let mut result = String::new();
    let mut width = 0;
    let mut offset = 0;
    let mut styled = false;
    for (start, end, sequence) in ESCAPE
        .find_iter(value)
        .map(|m| (m.start(), m.end(), m.as_str()))
        .chain(std::iter::once((value.len(), value.len(), "")))
    {
        for grapheme in value[offset..start].graphemes(true) {
            let next = width + UnicodeWidthStr::width(grapheme);
            if next > available {
                result.push_str(tail);
                if styled {
                    result.push_str("\x1b[0m");
                }
                return result;
            }
            result.push_str(grapheme);
            width = next;
        }
        if start != end {
            result.push_str(sequence);
            styled = true;
        }
        offset = end;
    }
    result.push_str(tail);
    if styled {
        result.push_str("\x1b[0m");
    }
    result
}
pub fn truncate_left(value: &str, limit: usize, prefix: &str) -> String {
    if visual_width(value) <= limit {
        return value.into();
    }
    let available = limit.saturating_sub(visual_width(prefix));
    if available == 0 {
        return truncate(prefix, limit, "");
    }
    let plain = strip_ansi(value);
    let mut width = 0;
    let retained = plain
        .graphemes(true)
        .rev()
        .take_while(|g| {
            width += UnicodeWidthStr::width(*g);
            width <= available
        })
        .collect::<Vec<_>>();
    format!("{prefix}{}", retained.into_iter().rev().collect::<String>())
}
static BRACKETED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[\s\p{Zs}]*[:·|][\s\p{Zs}]+|[\s\p{Zs}]+[-–—/]+[\s\p{Zs}]+|[\s\p{Zs}]+)")
        .expect("fixed separator")
});
static PLAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[\s\p{Zs}]*[:·|][\s\p{Zs}]+|[\s\p{Zs}]+[-–—/]+[\s\p{Zs}]+)")
        .expect("fixed separator")
});
pub fn trim_project_prefix(title: &str, project: &str) -> String {
    let name = project.trim();
    let mut candidates = vec![name];
    let base = name
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name);
    let base = if base.to_ascii_lowercase().ends_with(".git") {
        &base[..base.len() - 4]
    } else {
        base
    };
    if base != name {
        candidates.push(base);
    }
    candidates.sort_by_key(|name| std::cmp::Reverse(name.len()));
    let start = title.trim_start();
    for candidate in candidates.into_iter().filter(|name| !name.is_empty()) {
        let after_bracket = start
            .strip_prefix('[')
            .map(str::trim_start)
            .and_then(|s| s.strip_prefix(candidate))
            .map(str::trim_start)
            .and_then(|s| s.strip_prefix(']'));
        let after_plain = start.strip_prefix(candidate);
        for (rest, pattern) in [(after_bracket, &*BRACKETED), (after_plain, &*PLAIN)] {
            if let Some(rest) = rest
                && let Some(separator) = pattern.find(rest)
            {
                let remainder = rest[separator.end()..].trim();
                if !remainder
                    .trim_matches(|c: char| c.is_whitespace() || ":·|/-–—".contains(c))
                    .is_empty()
                    && !remainder.starts_with(']')
                {
                    return remainder.into();
                }
            }
        }
    }
    title.into()
}
