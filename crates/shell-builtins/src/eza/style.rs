#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Purple,
    Cyan,
    White,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightPurple,
    LightCyan,
    LightGray,
    Fixed(u8),
    Rgb(u8, u8, u8),
}

const NAMED: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Purple,
    Color::Cyan,
    Color::White,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightPurple,
    Color::LightCyan,
    Color::LightGray,
];

const FIXED: u64 = 0x100;
const RGB: u64 = 0x100_0000;
const FG_SHIFT: u32 = 8;
const BG_SHIFT: u32 = 33;
const COLOR_MASK: u64 = 0x1ff_ffff;

impl Color {
    const fn code(self) -> u64 {
        match self {
            Color::Black => 1,
            Color::Red => 2,
            Color::Green => 3,
            Color::Yellow => 4,
            Color::Blue => 5,
            Color::Purple => 6,
            Color::Cyan => 7,
            Color::White => 8,
            Color::DarkGray => 9,
            Color::LightRed => 10,
            Color::LightGreen => 11,
            Color::LightYellow => 12,
            Color::LightBlue => 13,
            Color::LightPurple => 14,
            Color::LightCyan => 15,
            Color::LightGray => 16,
            Color::Fixed(n) => FIXED | n as u64,
            Color::Rgb(r, g, b) => RGB | (r as u64) << 16 | (g as u64) << 8 | b as u64,
        }
    }

    fn from_code(code: u64) -> Color {
        if code & RGB != 0 {
            Color::Rgb((code >> 16) as u8, (code >> 8) as u8, code as u8)
        } else if code & FIXED != 0 {
            Color::Fixed(code as u8)
        } else {
            NAMED[(code as usize).saturating_sub(1).min(15)]
        }
    }

    fn write(self, out: &mut Vec<u8>, background: bool) {
        let (base, bright) = match self {
            Color::Black => (0, false),
            Color::Red => (1, false),
            Color::Green => (2, false),
            Color::Yellow => (3, false),
            Color::Blue => (4, false),
            Color::Purple => (5, false),
            Color::Cyan => (6, false),
            Color::White => (7, false),
            Color::DarkGray => (0, true),
            Color::LightRed => (1, true),
            Color::LightGreen => (2, true),
            Color::LightYellow => (3, true),
            Color::LightBlue => (4, true),
            Color::LightPurple => (5, true),
            Color::LightCyan => (6, true),
            Color::LightGray => (7, true),
            Color::Fixed(n) => {
                out.extend_from_slice(if background { b"48;5;" } else { b"38;5;" });
                push_number(out, u32::from(n));
                return;
            }
            Color::Rgb(r, g, b) => {
                out.extend_from_slice(if background { b"48;2;" } else { b"38;2;" });
                push_number(out, u32::from(r));
                out.push(b';');
                push_number(out, u32::from(g));
                out.push(b';');
                push_number(out, u32::from(b));
                return;
            }
        };
        let first = match (background, bright) {
            (false, false) => 30,
            (false, true) => 90,
            (true, false) => 40,
            (true, true) => 100,
        };
        push_number(out, first + base);
    }
}

fn push_number(out: &mut Vec<u8>, mut n: u32) {
    let mut digits = [0u8; 10];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

const BOLD: u64 = 1;
const DIMMED: u64 = 2;
const ITALIC: u64 = 4;
const UNDERLINE: u64 = 8;
const BLINK: u64 = 16;
const REVERSE: u64 = 32;
const HIDDEN: u64 = 64;
const STRIKE: u64 = 128;
const ATTRS: u64 = 0xff;
const ATTR_CODES: [(u64, u8); 8] = [
    (BOLD, b'1'),
    (DIMMED, b'2'),
    (ITALIC, b'3'),
    (UNDERLINE, b'4'),
    (BLINK, b'5'),
    (REVERSE, b'7'),
    (HIDDEN, b'8'),
    (STRIKE, b'9'),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) struct Style(u64);

impl Style {
    pub(super) const NONE: Style = Style(0);

    pub(super) const fn fg(color: Color) -> Style {
        Style(color.code() << FG_SHIFT)
    }

    const fn attr(self, bit: u64) -> Style {
        Style(self.0 | bit)
    }

    pub(super) const fn bold(self) -> Style {
        self.attr(BOLD)
    }

    pub(super) const fn dimmed(self) -> Style {
        self.attr(DIMMED)
    }

    pub(super) const fn italic(self) -> Style {
        self.attr(ITALIC)
    }

    pub(super) const fn underline(self) -> Style {
        self.attr(UNDERLINE)
    }

    pub(super) const fn blink(self) -> Style {
        self.attr(BLINK)
    }

    pub(super) const fn reverse(self) -> Style {
        self.attr(REVERSE)
    }

    pub(super) const fn hidden(self) -> Style {
        self.attr(HIDDEN)
    }

    pub(super) const fn strikethrough(self) -> Style {
        self.attr(STRIKE)
    }

    pub(super) const fn with_fg(self, color: Color) -> Style {
        Style(self.0 & !(COLOR_MASK << FG_SHIFT) | color.code() << FG_SHIFT)
    }

    pub(super) const fn on(self, color: Color) -> Style {
        Style(self.0 & !(COLOR_MASK << BG_SHIFT) | color.code() << BG_SHIFT)
    }

    pub(super) fn is_plain(&self) -> bool {
        self.0 == 0
    }

    fn fg_code(self) -> u64 {
        self.0 >> FG_SHIFT & COLOR_MASK
    }

    fn bg_code(self) -> u64 {
        self.0 >> BG_SHIFT & COLOR_MASK
    }

    pub(super) fn icon_style(&self) -> Style {
        let code = match self.bg_code() {
            0 => self.fg_code(),
            bg => bg,
        };
        Style(code << FG_SHIFT)
    }

    pub(super) fn overlay(self, over: Style) -> Style {
        let mut bits = self.0 | over.0 & ATTRS;
        if over.fg_code() != 0 {
            bits = bits & !(COLOR_MASK << FG_SHIFT) | over.fg_code() << FG_SHIFT;
        }
        if over.bg_code() != 0 {
            bits = bits & !(COLOR_MASK << BG_SHIFT) | over.bg_code() << BG_SHIFT;
        }
        Style(bits)
    }

    fn write_prefix_into(&self, out: &mut Vec<u8>) {
        if self.is_plain() {
            return;
        }
        out.extend_from_slice(b"\x1b[");
        let mut wrote = false;
        for (bit, code) in ATTR_CODES {
            if self.0 & bit != 0 {
                if wrote {
                    out.push(b';');
                }
                wrote = true;
                out.push(code);
            }
        }
        if self.bg_code() != 0 {
            if wrote {
                out.push(b';');
            }
            wrote = true;
            Color::from_code(self.bg_code()).write(out, true);
        }
        if self.fg_code() != 0 {
            if wrote {
                out.push(b';');
            }
            Color::from_code(self.fg_code()).write(out, false);
        }
        out.push(b'm');
    }

    fn difference(&self, next: &Style) -> Difference {
        if self == next {
            return Difference::Same;
        }
        let dropped = self.0 & ATTRS & !next.0;
        if dropped != 0
            || (self.fg_code() != 0 && next.fg_code() == 0)
            || (self.bg_code() != 0 && next.bg_code() == 0)
        {
            return Difference::Reset;
        }
        let mut bits = (self.0 ^ next.0) & ATTRS;
        if self.fg_code() != next.fg_code() {
            bits |= next.fg_code() << FG_SHIFT;
        }
        if self.bg_code() != next.bg_code() {
            bits |= next.bg_code() << BG_SHIFT;
        }
        Difference::Extra(Style(bits))
    }
}

enum Difference {
    Same,
    Reset,
    Extra(Style),
}

const RESET: &[u8] = b"\x1b[0m";
const CACHE_SLOTS: usize = 64;
const NO_STYLE: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct Prefix {
    key: u64,
    len: u8,
    bytes: [u8; 63],
}

pub(super) struct Painter {
    pub(super) out: Vec<u8>,
    last: u64,
    prefixes: Vec<Prefix>,
}

impl Painter {
    pub(super) fn with_capacity(capacity: usize) -> Painter {
        Painter {
            out: Vec::with_capacity(capacity),
            last: NO_STYLE,
            prefixes: vec![
                Prefix {
                    key: NO_STYLE,
                    len: 0,
                    bytes: [0; 63],
                };
                CACHE_SLOTS
            ],
        }
    }

    fn write_prefix(&mut self, style: &Style) {
        if style.is_plain() {
            return;
        }
        let slot = (style.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 58) as usize;
        let cached = &mut self.prefixes[slot];
        if cached.key != style.0 {
            let mut bytes = Vec::with_capacity(63);
            style.write_prefix_into(&mut bytes);
            cached.key = style.0;
            cached.len = bytes.len() as u8;
            cached.bytes[..bytes.len()].copy_from_slice(&bytes);
        }
        self.out
            .extend_from_slice(&cached.bytes[..usize::from(cached.len)]);
    }

    #[inline]
    pub(super) fn push(&mut self, style: &Style, text: &[u8]) {
        if self.last != style.0 {
            if self.last == NO_STYLE {
                self.write_prefix(style);
            } else {
                match Style(self.last).difference(style) {
                    Difference::Same => {}
                    Difference::Reset => {
                        self.out.extend_from_slice(RESET);
                        self.write_prefix(style);
                    }
                    Difference::Extra(extra) => self.write_prefix(&extra),
                }
            }
            self.last = style.0;
        }
        self.out.extend_from_slice(text);
    }

    pub(super) fn spaces(&mut self, count: usize) {
        self.pad(&Style::NONE, count);
    }

    pub(super) fn pad(&mut self, style: &Style, count: usize) {
        const SPACES: [u8; 32] = [b' '; 32];
        let mut left = count;
        let mut first = true;
        while left > 0 || first {
            let n = left.min(SPACES.len());
            self.push(style, &SPACES[..n]);
            left -= n;
            first = false;
        }
    }

    pub(super) fn end_line(&mut self) {
        if self.last != NO_STYLE && self.last != 0 {
            self.out.extend_from_slice(RESET);
        }
        self.last = NO_STYLE;
        self.out.push(b'\n');
    }

    pub(super) fn last(&self) -> Option<Style> {
        (self.last != NO_STYLE).then_some(Style(self.last))
    }

    pub(super) fn replay(&mut self, bytes: &[u8], last: Option<Style>) {
        self.out.extend_from_slice(bytes);
        self.last = last.map_or(NO_STYLE, |style| style.0);
    }

    pub(super) fn raw(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
}
