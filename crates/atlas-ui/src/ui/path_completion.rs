//! Bounded, nonrecursive filesystem completion. Call enumerate only on a worker.
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug)]
pub(crate) struct Query {
    pub parent: PathBuf,
    prefix: String,
    head: String,
    quoted: bool,
    closed_quote: bool,
    separator: char,
}

pub(crate) fn query(text: &str, base: &Path) -> Option<Query> {
    if text.is_empty() || text.contains(['\0', '\r', '\n']) || text.contains("://") {
        return None;
    }
    let quoted = text.starts_with('"');
    let closed_quote = quoted && text.len() > 1 && text.ends_with('"');
    let raw = if quoted {
        &text[1..text.len() - usize::from(closed_quote)]
    } else {
        text
    };
    if raw.is_empty() || raw.contains('"') {
        return None;
    }
    if raw.as_bytes().get(1) == Some(&b':')
        && !matches!(raw.as_bytes().get(2), Some(b'\\' | b'/'))
    {
        return None;
    }
    let separator = if raw.contains('\\') {
        '\\'
    } else if raw.contains('/') {
        '/'
    } else {
        '\\'
    };
    let (head, prefix) = raw
        .rfind(['\\', '/'])
        .map_or(("", raw), |i| (&raw[..i + 1], &raw[i + 1..]));
    // Do not discover servers/shares or enumerate a drive for drive-relative text.
    let network_path = raw.replace('/', "\\");
    let network_head = head.replace('/', "\\");
    let required = if network_path
        .to_ascii_lowercase()
        .starts_with("\\\\?\\unc\\")
    {
        4
    } else {
        2
    };
    if network_path.starts_with("\\\\")
        && network_path[2..]
            .split('\\')
            .filter(|s| !s.is_empty())
            .count()
            < required
    {
        return None;
    }
    if head.is_empty() && raw.contains(':') {
        return None;
    }
    if network_head.starts_with("\\\\")
        && network_head[2..]
            .split('\\')
            .filter(|s| !s.is_empty())
            .count()
            < required
    {
        return None;
    }
    let parent = if head.is_empty() {
        base.to_path_buf()
    } else {
        let path = Path::new(head);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        }
    };
    Some(Query {
        parent,
        prefix: prefix.to_lowercase(),
        head: head.into(),
        quoted,
        closed_quote,
        separator,
    })
}

pub(crate) fn enumerate(query: Query, generation: &AtomicU64, expected: u64) -> Vec<String> {
    if generation.load(Ordering::Relaxed) != expected {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&query.parent) else {
        return Vec::new();
    };
    let mut matches = std::collections::BTreeSet::new();
    let started = std::time::Instant::now();
    for (index, entry) in entries.enumerate() {
        // Returning no partial list preserves deterministic ordering. This is a
        // suggestion aid, so huge/slow parents yield to explicit navigation.
        if index >= 16_384 || started.elapsed() > std::time::Duration::from_millis(200) {
            return Vec::new();
        }
        if generation.load(Ordering::Relaxed) != expected {
            return Vec::new();
        }
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.to_lowercase().starts_with(&query.prefix) {
            continue;
        }
        // file_type avoids following links to potentially disconnected targets.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        matches.insert((name.to_lowercase(), name.to_owned()));
        // Retain the deterministic first eight, independent of directory order.
        if matches.len() > 8 {
            matches.pop_last();
        }
    }
    matches
        .into_iter()
        .map(|(_, name)| {
            format!(
                "{}{}{}{}{}",
                if query.quoted { "\"" } else { "" },
                query.head,
                name,
                query.separator,
                if query.closed_quote { "\"" } else { "" }
            )
        })
        .collect()
}

/// A ghost can only append at an empty selection at the end of the actual input.
pub(crate) fn suffix<'a>(text: &str, candidate: &'a str) -> Option<&'a str> {
    let prefix = candidate.get(..text.len())?;
    (prefix.to_lowercase() == text.to_lowercase())
        .then(|| &candidate[text.len()..])
        .filter(|s| !s.is_empty())
}
