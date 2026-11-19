use std::cmp::Ordering;

pub(super) fn compare_ignore_case(left: &str, right: &str) -> Ordering {
    if left.is_ascii() && right.is_ascii() {
        compare_ascii(left.as_bytes(), right.as_bytes())
    } else {
        compare(
            left.chars().flat_map(char::to_lowercase),
            right.chars().flat_map(char::to_lowercase),
        )
    }
}

pub(super) fn sort_key(name: &str) -> Option<Box<[u8]>> {
    let bytes = name.as_bytes();
    if !name.is_ascii() || bytes.iter().any(|&b| b <= b' ' || b == 0x7f) {
        return None;
    }
    let mut key = Vec::with_capacity(bytes.len() + 4);
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_digit() {
            let start = at;
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
            let run = &bytes[start..at];
            if run[0] == b'0' {
                key.push(b'0');
                key.extend_from_slice(run);
                key.push(1);
            } else {
                key.push(b'1');
                key.push(u8::try_from(run.len()).ok()?);
                key.extend_from_slice(run);
            }
        } else {
            key.push(bytes[at].to_ascii_lowercase());
            at += 1;
        }
    }
    Some(key.into())
}

fn is_space(b: u8) -> bool {
    b == b' ' || (b'\t'..=b'\r').contains(&b)
}

fn digit_at(bytes: &[u8], at: usize) -> Option<u8> {
    bytes
        .get(at)
        .filter(|b| b.is_ascii_digit())
        .map(|b| b - b'0')
}

fn compare_ascii(left: &[u8], right: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    loop {
        while left.get(i).copied().is_some_and(is_space) {
            i += 1;
        }
        while right.get(j).copied().is_some_and(is_space) {
            j += 1;
        }
        let (lc, rc) = match (left.get(i), right.get(j)) {
            (Some(&lc), Some(&rc)) => (lc.to_ascii_lowercase(), rc.to_ascii_lowercase()),
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        };
        if lc.is_ascii_digit() && rc.is_ascii_digit() {
            let (ld, rd) = (lc - b'0', rc - b'0');
            if ld == 0 || rd == 0 {
                if ld != rd {
                    return ld.cmp(&rd);
                }
                loop {
                    i += 1;
                    j += 1;
                    match (digit_at(left, i), digit_at(right, j)) {
                        (Some(ld), Some(rd)) if ld != rd => return ld.cmp(&rd),
                        (Some(_), Some(_)) => {}
                        (Some(_), None) => return Ordering::Greater,
                        (None, Some(_)) => return Ordering::Less,
                        (None, None) => break,
                    }
                }
            } else {
                let mut first = ld.cmp(&rd);
                loop {
                    i += 1;
                    j += 1;
                    match (digit_at(left, i), digit_at(right, j)) {
                        (Some(ld), Some(rd)) => {
                            if first == Ordering::Equal {
                                first = ld.cmp(&rd);
                            }
                        }
                        (Some(_), None) => return Ordering::Greater,
                        (None, Some(_)) => return Ordering::Less,
                        (None, None) => break,
                    }
                }
                if first != Ordering::Equal {
                    return first;
                }
            }
        } else {
            if lc != rc {
                return lc.cmp(&rc);
            }
            i += 1;
            j += 1;
        }
    }
}

fn digit(c: Option<char>) -> Option<u32> {
    c.and_then(|c| c.to_digit(10))
}

fn compare(
    mut left: impl Iterator<Item = char>,
    mut right: impl Iterator<Item = char>,
) -> Ordering {
    let mut l = left.next();
    let mut r = right.next();
    loop {
        while l.is_some_and(char::is_whitespace) {
            l = left.next();
        }
        while r.is_some_and(char::is_whitespace) {
            r = right.next();
        }
        let (lc, rc) = match (l, r) {
            (Some(lc), Some(rc)) => (lc, rc),
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        };
        match (lc.to_digit(10), rc.to_digit(10)) {
            (Some(ld), Some(rd)) => {
                if ld == 0 || rd == 0 {
                    match ld.cmp(&rd) {
                        Ordering::Equal => {}
                        other => return other,
                    }
                    loop {
                        l = left.next();
                        r = right.next();
                        match (digit(l), digit(r)) {
                            (Some(ld), Some(rd)) => match ld.cmp(&rd) {
                                Ordering::Equal => {}
                                other => return other,
                            },
                            (Some(_), None) => return Ordering::Greater,
                            (None, Some(_)) => return Ordering::Less,
                            (None, None) => break,
                        }
                    }
                } else {
                    let mut first = ld.cmp(&rd);
                    loop {
                        l = left.next();
                        r = right.next();
                        match (digit(l), digit(r)) {
                            (Some(ld), Some(rd)) => {
                                if first == Ordering::Equal {
                                    first = ld.cmp(&rd);
                                }
                            }
                            (Some(_), None) => return Ordering::Greater,
                            (None, Some(_)) => return Ordering::Less,
                            (None, None) => break,
                        }
                    }
                    if first != Ordering::Equal {
                        return first;
                    }
                }
            }
            _ => {
                match lc.cmp(&rc) {
                    Ordering::Equal => {}
                    other => return other,
                }
                l = left.next();
                r = right.next();
            }
        }
    }
}
