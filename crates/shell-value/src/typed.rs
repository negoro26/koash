use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch. A column of these prints as an ISO 8601 UTC string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnixTime(pub u64);

impl UnixTime {
    /// `None` for a time before 1970 and for one the filesystem cannot report.
    pub fn from_system(time: io::Result<SystemTime>) -> Option<Self> {
        let since = time.ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(UnixTime(since.as_secs()))
    }

    pub(crate) fn text(self) -> ShortText {
        let (days, rem) = (self.0 / 86_400, self.0 % 86_400);
        let (year, month, day) = civil_from_days(days as i64);
        let mut text = ShortText::default();
        text.push_decimal(year as u64, 4);
        text.push(b'-');
        text.push_decimal(month as u64, 2);
        text.push(b'-');
        text.push_decimal(day as u64, 2);
        text.push(b'T');
        text.push_decimal(rem / 3600, 2);
        text.push(b':');
        text.push_decimal(rem % 3600 / 60, 2);
        text.push(b':');
        text.push_decimal(rem % 60, 2);
        text.push(b'Z');
        text
    }
}

/// Unix permission bits. A column of these prints as an octal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMode(pub u32);

impl FileMode {
    pub(crate) fn text(self) -> ShortText {
        let mut digits = [0u8; 11];
        let mut at = digits.len();
        let mut bits = self.0;
        loop {
            at -= 1;
            digits[at] = b'0' + (bits & 7) as u8;
            bits >>= 3;
            if bits == 0 {
                break;
            }
        }
        let mut text = ShortText::default();
        digits[at..].iter().for_each(|&digit| text.push(digit));
        text
    }
}

/// A value that is the empty string when it is missing, instead of null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrBlank<T>(pub Option<T>);

/// Up to 31 bytes of ASCII text held inline, so formatting a time or a mode never allocates.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShortText {
    len: u8,
    buf: [u8; 31],
}

impl ShortText {
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len()]).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, byte: u8) {
        let at = self.len();
        if let Some(slot) = self.buf.get_mut(at) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn push_decimal(&mut self, mut n: u64, width: usize) {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for _ in digits.len() - at..width {
            self.push(b'0');
        }
        digits[at..].iter().for_each(|&digit| self.push(digit));
    }
}

// Howard Hinnant's civil_from_days: https://howardhinnant.github.io/date_algorithms.html#civil_from_days
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The deleted `format_unix_iso`, kept as the oracle the new formatter must agree with.
    fn reference_iso(secs: u64) -> String {
        let days = secs / 86400;
        let rem = secs % 86400;
        let hour = rem / 3600;
        let min = (rem % 3600) / 60;
        let sec = rem % 60;
        let (year, month, day) = civil_from_days(days as i64);
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
    }

    #[test]
    fn a_time_prints_as_the_old_iso_string() {
        let edges = [
            0,
            1,
            86_399,
            86_400,
            951_782_400,
            1_700_000_000,
            u64::from(u32::MAX),
            4_102_444_800,
            253_402_300_799,
            253_402_300_800,
            1 << 40,
            u64::MAX,
        ];
        let stepped = (0..4_000_000_000u64).step_by(7_654_321);
        for secs in edges.into_iter().chain(stepped) {
            assert_eq!(
                UnixTime(secs).text().as_str(),
                reference_iso(secs),
                "{secs}"
            );
        }
    }

    #[test]
    fn a_mode_prints_as_octal() {
        for mode in [0, 7, 0o70, 0o644, 0o777, 0o4755, u32::MAX] {
            assert_eq!(
                FileMode(mode).text().as_str(),
                format!("{mode:o}"),
                "{mode:o}"
            );
        }
    }

    #[test]
    fn a_time_before_1970_or_unknown_is_none() {
        let before = UNIX_EPOCH - std::time::Duration::from_secs(1);
        assert_eq!(UnixTime::from_system(Ok(before)), None);
        assert_eq!(
            UnixTime::from_system(Err(io::ErrorKind::Unsupported.into())),
            None
        );
        let later = UNIX_EPOCH + std::time::Duration::from_secs(5);
        assert_eq!(UnixTime::from_system(Ok(later)), Some(UnixTime(5)));
    }
}
