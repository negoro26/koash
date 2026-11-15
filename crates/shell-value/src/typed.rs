use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnixTime(pub u64);

impl UnixTime {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrBlank<T>(pub Option<T>);

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

    #[test]
    fn a_time_prints_as_iso_8601_utc() {
        let cases = [
            (0, "1970-01-01T00:00:00Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (86_400, "1970-01-02T00:00:00Z"),
            (951_782_400, "2000-02-29T00:00:00Z"),
            (1_610_714_096, "2021-01-15T12:34:56Z"),
            (1_613_392_496, "2021-02-15T12:34:56Z"),
            (1_615_811_696, "2021-03-15T12:34:56Z"),
            (1_618_490_096, "2021-04-15T12:34:56Z"),
            (1_621_082_096, "2021-05-15T12:34:56Z"),
            (1_623_760_496, "2021-06-15T12:34:56Z"),
            (1_626_352_496, "2021-07-15T12:34:56Z"),
            (1_629_030_896, "2021-08-15T12:34:56Z"),
            (1_631_709_296, "2021-09-15T12:34:56Z"),
            (1_634_301_296, "2021-10-15T12:34:56Z"),
            (1_636_979_696, "2021-11-15T12:34:56Z"),
            (1_639_571_696, "2021-12-15T12:34:56Z"),
            (1_700_000_000, "2023-11-14T22:13:20Z"),
            (4_102_444_800, "2100-01-01T00:00:00Z"),
            (u64::from(u32::MAX), "2106-02-07T06:28:15Z"),
            (253_402_300_799, "9999-12-31T23:59:59Z"),
            (253_402_300_800, "10000-01-01T00:00:00Z"),
            (1 << 40, "36812-02-20T00:36:16Z"),
            (u64::MAX, "584554051223-11-09T07:00:15Z"),
        ];
        let mut scratch = String::new();
        for (secs, iso) in cases {
            assert_eq!(
                crate::Datum::Time(UnixTime(secs)).text(&mut scratch),
                iso,
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
