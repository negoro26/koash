use crate::word_expand::{Kind, Sym};

const MAX_SEQUENCE: i128 = 1_000_000;

struct Group {
    open: usize,
    close: usize,
}

pub(crate) fn expand(syms: &[Sym]) -> Vec<Vec<Sym>> {
    let Some((group, alternatives)) = find_group(syms) else {
        return vec![syms.to_vec()];
    };
    let suffixes = expand(&syms[group.close + 1..]);
    let mut out = Vec::with_capacity(alternatives.len() * suffixes.len());
    for alternative in &alternatives {
        for suffix in &suffixes {
            let mut word = Vec::with_capacity(group.open + alternative.len() + suffix.len());
            word.extend_from_slice(&syms[..group.open]);
            word.extend_from_slice(alternative);
            word.extend_from_slice(suffix);
            out.push(word);
        }
    }
    out
}

fn is_brace(sym: Sym, ch: char) -> bool {
    sym.kind == Kind::Raw && sym.ch == ch
}

fn find_group(syms: &[Sym]) -> Option<(Group, Vec<Vec<Sym>>)> {
    for (open, _) in syms.iter().enumerate().filter(|(_, s)| is_brace(**s, '{')) {
        let Some(close) = matching_close(syms, open) else {
            continue;
        };
        let inner = &syms[open + 1..close];
        let alternatives = comma_alternatives(inner).or_else(|| sequence(inner));
        if let Some(alternatives) = alternatives {
            return Some((Group { open, close }, alternatives));
        }
    }
    None
}

fn matching_close(syms: &[Sym], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &sym) in syms.iter().enumerate().skip(open) {
        if is_brace(sym, '{') {
            depth += 1;
        } else if is_brace(sym, '}') {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn comma_alternatives(inner: &[Sym]) -> Option<Vec<Vec<Sym>>> {
    let mut depth = 0usize;
    let mut start = 0;
    let mut pieces = Vec::new();
    for (i, &sym) in inner.iter().enumerate() {
        if is_brace(sym, '{') {
            depth += 1;
        } else if is_brace(sym, '}') {
            depth -= 1;
        } else if depth == 0 && is_brace(sym, ',') {
            pieces.push(&inner[start..i]);
            start = i + 1;
        }
    }
    if pieces.is_empty() {
        return None;
    }
    pieces.push(&inner[start..]);
    Some(pieces.into_iter().flat_map(expand).collect())
}

fn sequence(inner: &[Sym]) -> Option<Vec<Vec<Sym>>> {
    if inner.iter().any(|s| s.kind != Kind::Raw) {
        return None;
    }
    let text: String = inner.iter().map(|s| s.ch).collect();
    let mut parts = text.split("..");
    let (from, to) = (parts.next()?, parts.next()?);
    let step = match parts.next() {
        Some(step) if is_int(step) => Some(step.parse::<i128>().ok()?.unsigned_abs().max(1)),
        Some(_) => return None,
        None => None,
    };
    if parts.next().is_some() {
        return None;
    }
    let step = step.unwrap_or(1) as i128;
    let items: Vec<String> = if is_int(from) && is_int(to) {
        numbers(from, to, step)?
    } else {
        let (a, b) = (single_char(from)?, single_char(to)?);
        let count = (a as i128 - b as i128).abs() / step + 1;
        if count > MAX_SEQUENCE {
            return None;
        }
        walk(a as i128, b as i128, step)
            .filter_map(|c| u32::try_from(c).ok().and_then(char::from_u32))
            .map(String::from)
            .collect()
    };
    Some(
        items
            .into_iter()
            .map(|item| {
                item.chars()
                    .map(|ch| Sym {
                        ch,
                        kind: Kind::Quoted,
                    })
                    .collect()
            })
            .collect(),
    )
}

fn is_int(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && ch.is_ascii() && !ch.is_ascii_digit()).then_some(ch)
}

fn zero_padded(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    digits.len() > 1 && digits.starts_with('0')
}

fn numbers(from: &str, to: &str, step: i128) -> Option<Vec<String>> {
    let (a, b) = (from.parse::<i128>().ok()?, to.parse::<i128>().ok()?);
    if (a - b).abs() / step + 1 > MAX_SEQUENCE {
        return None;
    }
    let width = if zero_padded(from) || zero_padded(to) {
        from.len().max(to.len())
    } else {
        0
    };
    Some(walk(a, b, step).map(|n| format!("{n:0width$}")).collect())
}

fn walk(a: i128, b: i128, step: i128) -> impl Iterator<Item = i128> {
    let signed = if a <= b { step } else { -step };
    let count = (a - b).abs() / step + 1;
    (0..count).map(move |i| a + i * signed)
}
