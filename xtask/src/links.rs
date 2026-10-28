//! Offline local Markdown link verification over maintained documentation trees.
use crate::{read, Result, ToolError};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
const EXCLUDED: &[&str] = &[
    ".scratch",
    ".git",
    ".cache",
    "__pycache__",
    "target",
    "dist",
    "node_modules",
];
fn documents(dir: &Path, recursive: bool, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|source| ToolError::Io {
        context: format!("read directory {}", dir.display()),
        source,
    })? {
        let entry = entry.map_err(|source| ToolError::Io {
            context: "read documentation entry".into(),
            source,
        })?;
        let path = entry.path();
        if EXCLUDED.contains(&entry.file_name().to_string_lossy().as_ref()) {
            continue;
        }
        if path.is_file() && path.extension().is_some_and(|x| x == "md") {
            files.push(path);
        } else if recursive && entry.file_type().is_ok_and(|t| t.is_dir()) {
            documents(&path, true, files)?;
        }
    }
    Ok(())
}
pub(crate) fn check(root: &Path) -> Result<(usize, Vec<String>)> {
    if !root.is_dir() {
        return Err(ToolError::Invalid(
            "root must be an existing directory".into(),
        ));
    }
    let mut files = Vec::new();
    documents(root, false, &mut files)?;
    documents(&root.join("docs"), true, &mut files)?;
    files.sort();
    let scheme = regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")?;
    let escapes = regex::Regex::new(r##"\\([!"#$%&'()*+,\-./:;<=>?@\[\]\\^_`{|}~])"##)?;
    let mut errors = Vec::new();
    for file in &files {
        let text = read(file)?;
        for link in links(&text)? {
            let value = &link.target;
            let mut reason = link.reason;
            if reason.is_none() {
                let target = escapes.replace_all(value, "$1");
                let decoded = html_escape::decode_html_entities(&target);
                if scheme.is_match(&decoded) || decoded.starts_with("//") {
                    continue;
                }
                let (url_path, fragment) = decoded.split_once('#').unwrap_or((&decoded, ""));
                let path = percent_encoding::percent_decode_str(
                    url_path.split('?').next().unwrap_or_default(),
                )
                .decode_utf8_lossy();
                let destination = if path.starts_with('/') {
                    root.join(path.trim_start_matches('/'))
                } else if path.is_empty() {
                    file.clone()
                } else {
                    file.parent().unwrap_or(root).join(path.as_ref())
                };
                if !destination.exists() {
                    reason = Some("missing target");
                } else if !fragment.is_empty()
                    && destination
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("md"))
                    && !headings(&read(&destination)?)?.contains(
                        percent_encoding::percent_decode_str(fragment)
                            .decode_utf8_lossy()
                            .as_ref(),
                    )
                {
                    reason = Some("missing heading fragment");
                }
            }
            if let Some(reason) = reason {
                let line = text[..link.offset].bytes().filter(|b| *b == b'\n').count() + 1;
                errors.push(format!(
                    "{}:{line}: {value}: {reason}",
                    file.strip_prefix(root).unwrap_or(file).display()
                ));
            }
        }
    }
    Ok((files.len(), errors))
}

fn headings(text: &str) -> Result<HashSet<String>> {
    let atx = regex::Regex::new(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$")?;
    let setext = regex::Regex::new(r"^ {0,3}(?:=+|-+)\s*$")?;
    let filtered = without_fences(text)?;
    let lines: Vec<_> = filtered.lines().collect();
    let mut anchors = HashSet::new();
    let marks = regex::Regex::new(r"\p{M}")?;
    for (index, line) in lines.iter().enumerate() {
        let title = if let Some(capture) = atx.captures(line) {
            capture.get(1).map(|x| x.as_str())
        } else if lines
            .get(index + 1)
            .is_some_and(|next| setext.is_match(next))
            && !line.trim().is_empty()
        {
            Some(line.trim())
        } else {
            None
        };
        if let Some(title) = title {
            let base: String = heading_text(title)?
                .to_lowercase()
                .chars()
                .filter(|c| {
                    c.is_whitespace()
                        || c.is_alphanumeric()
                        || *c == '-'
                        || *c == '_'
                        || marks.is_match(&c.to_string())
                })
                .map(|c| if c.is_whitespace() { '-' } else { c })
                .collect();
            let mut anchor = base.clone();
            let mut suffix = 0;
            while anchors.contains(&anchor) {
                suffix += 1;
                anchor = format!("{base}-{suffix}");
            }
            anchors.insert(anchor);
        }
    }
    Ok(anchors)
}

// Hide literal text without shifting byte offsets or diagnostic line numbers.
fn blank(text: &str) -> String {
    text.bytes()
        .map(|b| if b == b'\n' { '\n' } else { ' ' })
        .collect()
}
fn without_fences(text: &str) -> Result<String> {
    let quote = regex::Regex::new(r"^ {0,3}>[ \t]?")?;
    let marker = regex::Regex::new(r"^ {0,3}(`{3,}|~{3,})(.*)$")?;
    let mut fence: Option<(u8, usize, usize)> = None;
    let mut result = String::new();
    for line in text.split_inclusive('\n') {
        let mut content = line;
        let mut depth = 0;
        while let Some(found) = quote.find(content) {
            depth += 1;
            content = &content[found.end()..];
        }
        if fence.is_some_and(|(_, _, fdepth)| depth < fdepth) {
            fence = None;
        }
        if let Some((kind, length, fdepth)) = fence {
            result.push_str(&blank(line));
            content = line;
            for _ in 0..fdepth {
                if let Some(found) = quote.find(content) {
                    content = &content[found.end()..];
                }
            }
            if let Some(capture) = marker.captures(content.trim_end_matches('\n')) {
                if let (Some(run), Some(rest)) = (capture.get(1), capture.get(2)) {
                    if run.as_str().as_bytes()[0] == kind
                        && run.len() >= length
                        && rest.as_str().trim().is_empty()
                    {
                        fence = None;
                    }
                }
            }
        } else if let Some(capture) = marker.captures(content.trim_end_matches('\n')) {
            if let Some(run) = capture.get(1) {
                fence = Some((run.as_str().as_bytes()[0], run.len(), depth));
            }
            result.push_str(&blank(line));
        } else {
            result.push_str(line);
        }
    }
    Ok(result)
}
fn code_spans(text: &str) -> Vec<(usize, usize, &str)> {
    let b = text.as_bytes();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'`' {
            let start = i;
            while i < b.len() && b[i] == b'`' {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }
    let mut spans = Vec::new();
    let mut index = 0;
    while index < runs.len() {
        let (start, end) = runs[index];
        let escapes = b[..start].iter().rev().take_while(|x| **x == b'\\').count();
        if escapes % 2 == 1 {
            index += 1;
            continue;
        }
        if let Some(close) =
            (index + 1..runs.len()).find(|j| runs[*j].1 - runs[*j].0 == end - start)
        {
            spans.push((start, runs[close].1, &text[end..runs[close].0]));
            index = close + 1;
        } else {
            index += 1;
        }
    }
    spans
}
fn without_inline_code(text: &str) -> String {
    let mut result = text.to_owned();
    for (start, end, _) in code_spans(text) {
        result.replace_range(start..end, &blank(&text[start..end]));
    }
    result
}

fn closing(text: &str, start: usize, opening: u8, close: u8) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0;
    let mut index = start;
    while index < b.len() {
        if b[index] == b'\\' {
            index += 2;
            continue;
        }
        if b[index] == opening {
            depth += 1;
        } else if b[index] == close {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}
fn destination(text: &str, start: usize) -> Option<(&str, usize)> {
    let rest = text.get(start..)?;
    let start = start + rest.len() - rest.trim_start().len();
    let b = text.as_bytes();
    if b.get(start) == Some(&b'<') {
        let end = text[start + 1..].find('>')? + start + 1;
        return Some((&text[start + 1..end], end + 1));
    }
    let mut index = start;
    let mut depth = 0;
    while index < b.len() {
        if b[index] == b'\\' {
            index = (index + 2).min(b.len());
            continue;
        }
        if b[index].is_ascii_whitespace() || (b[index] == b')' && depth == 0) {
            break;
        }
        if b[index] == b'(' {
            depth += 1;
        } else if b[index] == b')' {
            depth -= 1;
        }
        index += 1;
    }
    text.get(start..index).map(|target| (target, index))
}
fn inline_destination(text: &str, start: usize) -> Option<(&str, usize)> {
    let (target, mut index) = destination(text, start + 1)?;
    index += text[index..].len() - text[index..].trim_start().len();
    if let Some(delimiter) = text
        .as_bytes()
        .get(index)
        .copied()
        .filter(|x| b"\"'(".contains(x))
    {
        let end = if delimiter == b'(' {
            closing(text, index, b'(', b')')?
        } else {
            let mut end = index + 1;
            while let Some(b) = text.as_bytes().get(end) {
                if *b == b'\\' {
                    end += 2;
                } else if *b == delimiter {
                    break;
                } else {
                    end += 1;
                }
            }
            end
        };
        if end >= text.len() {
            return None;
        }
        index = end + 1;
        index += text[index..].len() - text[index..].trim_start().len();
    }
    (text.as_bytes().get(index) == Some(&b')')).then_some((target, index))
}
fn label(text: &str) -> String {
    use unicode_casefold::UnicodeCaseFold;
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .case_fold()
        .collect()
}

struct Link {
    offset: usize,
    target: String,
    reason: Option<&'static str>,
}
fn links(text: &str) -> Result<Vec<Link>> {
    use std::collections::HashMap;
    let mut text = without_inline_code(&without_fences(text)?);
    let definition = regex::Regex::new(r"(?m)^ {0,3}\[")?;
    let starts: Vec<_> = definition
        .find_iter(&text)
        .map(|x| (x.start(), x.end() - 1))
        .collect();
    let mut definitions = HashMap::new();
    for (start, opening) in starts {
        let Some(end) = closing(&text, opening, b'[', b']') else {
            continue;
        };
        if text.as_bytes().get(end + 1) != Some(&b':') {
            continue;
        }
        if let Some((target, _)) = destination(&text, end + 2) {
            definitions
                .entry(label(&text[opening + 1..end]))
                .or_insert_with(|| target.to_owned());
        }
        let line_end = text[end..]
            .find('\n')
            .map(|x| x + end)
            .unwrap_or(text.len());
        text.replace_range(start..line_end, &blank(&text[start..line_end]));
    }
    let mut result = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let b = text.as_bytes();
        if b[index] == b'\\' {
            index += 2;
            continue;
        }
        if b[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(end) = closing(&text, index, b'[', b']') else {
            index += 1;
            continue;
        };
        let after = end + 1;
        if b.get(after) == Some(&b'(') {
            if let Some((target, close)) = inline_destination(&text, after) {
                result.push(Link {
                    offset: index,
                    target: target.to_owned(),
                    reason: None,
                });
                index = close + 1;
                continue;
            }
        } else if b.get(after) == Some(&b'[') {
            if let Some(close) = closing(&text, after, b'[', b']') {
                let explicit = &text[after + 1..close];
                let key = label(if explicit.is_empty() {
                    &text[index + 1..end]
                } else {
                    explicit
                });
                result.push(Link {
                    offset: index,
                    target: definitions
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| key.clone()),
                    reason: (!definitions.contains_key(&key)).then_some("undefined reference"),
                });
                index = close + 1;
                continue;
            }
        }
        if let Some(target) = definitions.get(&label(&text[index + 1..end])) {
            result.push(Link {
                offset: index,
                target: target.clone(),
                reason: None,
            });
        }
        index = end + 1;
    }
    Ok(result)
}

fn heading_text(title: &str) -> Result<String> {
    let mut title = title.to_owned();
    let mut code = Vec::new();
    for (number, (start, end, content)) in code_spans(&title).into_iter().enumerate().rev() {
        code.push((start, end, format!("\0code{number}\0"), content.to_owned()));
    }
    for (start, end, token, _) in &code {
        title.replace_range(*start..*end, token);
    }
    title = regex::Regex::new(r"!?\[([^]]+)\](?:\([^)]*\)|\[[^]]*\])")?
        .replace_all(&title, "$1")
        .into_owned();
    title =
        html_escape::decode_html_entities(&regex::Regex::new(r"<[^>]*>")?.replace_all(&title, ""))
            .into_owned();
    // Only delimiter underscores are removed. Embedded snake_case and code
    // underscores are literal and must survive GitHub-style heading slugs.
    loop {
        let mut remove = None;
        let mut index = 0;
        while index < title.len() {
            if title.as_bytes()[index] != b'_' {
                index += 1;
                continue;
            }
            let start = index;
            while title.as_bytes().get(index) == Some(&b'_') {
                index += 1;
            }
            let count = index - start;
            let word = |c: char| c.is_alphanumeric() || c == '_';
            if title[..start].chars().next_back().is_some_and(word)
                || title[index..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
            {
                continue;
            }
            let delimiter = "_".repeat(count);
            let mut search = index;
            while let Some(found) = title[search..].find(&delimiter) {
                let close = search + found;
                let after = close + count;
                if close > index
                    && title[index..close]
                        .chars()
                        .next_back()
                        .is_some_and(|c| !c.is_whitespace())
                    && title[after..].chars().next().is_none_or(|c| !word(c))
                {
                    remove = Some((start, index, close, after));
                    break;
                }
                search = after;
            }
            if remove.is_some() {
                break;
            }
        }
        let Some((start, content, close, after)) = remove else {
            break;
        };
        title.replace_range(close..after, "");
        title.replace_range(start..content, "");
    }
    for (_, _, token, content) in code {
        title = title.replace(&token, &content);
    }
    Ok(title)
}
