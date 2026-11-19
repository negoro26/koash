use std::collections::HashMap;
use std::sync::OnceLock;

use rustc_hash::FxHashMap;

use super::Class;
use super::style::{Color, Style};
use super::tables::{
    DIRECTORY_ICONS, EXTENSION_ICONS, EXTENSION_TYPES, FILENAME_ICONS, FILENAME_TYPES,
};

const SIZE_BYTE: usize = 0;
const SIZE_KILO: usize = 1;
const SIZE_MEGA: usize = 2;
const SIZE_GIGA: usize = 3;
const SIZE_HUGE: usize = 4;

pub(super) struct Theme {
    pub(super) normal: Style,
    pub(super) directory: Style,
    pub(super) symlink: Style,
    pub(super) pipe: Style,
    pub(super) block_device: Style,
    pub(super) char_device: Style,
    pub(super) socket: Style,
    pub(super) special: Style,
    pub(super) executable: Style,
    pub(super) mount_point: Style,
    pub(super) user_read: Style,
    pub(super) user_write: Style,
    pub(super) user_execute_file: Style,
    pub(super) user_execute_other: Style,
    pub(super) group_read: Style,
    pub(super) group_write: Style,
    pub(super) group_execute: Style,
    pub(super) other_read: Style,
    pub(super) other_write: Style,
    pub(super) other_execute: Style,
    pub(super) special_user_file: Style,
    pub(super) special_other: Style,
    pub(super) attribute: Style,
    number: [Style; 5],
    pub(super) major: Style,
    pub(super) minor: Style,
    pub(super) user_you: Style,
    pub(super) user_other: Style,
    pub(super) user_root: Style,
    pub(super) punctuation: Style,
    pub(super) date: Style,
    pub(super) header: Style,
    pub(super) symlink_path: Style,
    pub(super) control_char: Style,
    pub(super) broken_symlink: Style,
    pub(super) broken_path_overlay: Style,
    types: [Style; 11],
    exts: Mappings,
    use_types: bool,
}

impl Theme {
    fn plain() -> Theme {
        Theme {
            normal: Style::NONE,
            directory: Style::NONE,
            symlink: Style::NONE,
            pipe: Style::NONE,
            block_device: Style::NONE,
            char_device: Style::NONE,
            socket: Style::NONE,
            special: Style::NONE,
            executable: Style::NONE,
            mount_point: Style::NONE,
            user_read: Style::NONE,
            user_write: Style::NONE,
            user_execute_file: Style::NONE,
            user_execute_other: Style::NONE,
            group_read: Style::NONE,
            group_write: Style::NONE,
            group_execute: Style::NONE,
            other_read: Style::NONE,
            other_write: Style::NONE,
            other_execute: Style::NONE,
            special_user_file: Style::NONE,
            special_other: Style::NONE,
            attribute: Style::NONE,
            number: [Style::NONE; 5],
            major: Style::NONE,
            minor: Style::NONE,
            user_you: Style::NONE,
            user_other: Style::NONE,
            user_root: Style::NONE,
            punctuation: Style::NONE,
            date: Style::NONE,
            header: Style::NONE,
            symlink_path: Style::NONE,
            control_char: Style::NONE,
            broken_symlink: Style::NONE,
            broken_path_overlay: Style::NONE,
            types: [Style::NONE; 11],
            exts: Mappings::default(),
            use_types: false,
        }
    }

    fn colourful() -> Theme {
        let green = Style::fg(Color::Green);
        let mut types = [Style::NONE; 11];
        types[Class::Image as usize] = Style::fg(Color::Purple);
        types[Class::Video as usize] = Style::fg(Color::Purple).bold();
        types[Class::Music as usize] = Style::fg(Color::Cyan);
        types[Class::Lossless as usize] = Style::fg(Color::Cyan).bold();
        types[Class::Crypto as usize] = green.bold();
        types[Class::Document as usize] = green;
        types[Class::Compressed as usize] = Style::fg(Color::Red);
        types[Class::Temp as usize] = Style::NONE.dimmed();
        types[Class::Compiled as usize] = Style::fg(Color::Yellow);
        types[Class::Build as usize] = Style::fg(Color::Yellow).bold().underline();
        types[Class::Source as usize] = Style::fg(Color::Yellow).bold();
        let yellow = Style::fg(Color::Yellow);
        let red = Style::fg(Color::Red);
        Theme {
            normal: Style::NONE,
            directory: Style::fg(Color::Blue).bold(),
            symlink: Style::fg(Color::Cyan),
            pipe: yellow,
            block_device: yellow.bold(),
            char_device: yellow.bold(),
            socket: red.bold(),
            special: yellow,
            executable: green.bold(),
            mount_point: Style::fg(Color::Blue).bold().underline(),
            user_read: yellow.bold(),
            user_write: red.bold(),
            user_execute_file: green.bold().underline(),
            user_execute_other: green.bold(),
            group_read: yellow,
            group_write: red,
            group_execute: green,
            other_read: yellow,
            other_write: red,
            other_execute: green,
            special_user_file: Style::fg(Color::Purple),
            special_other: Style::fg(Color::Purple),
            attribute: Style::NONE,
            number: [green, green.bold(), yellow, red, Style::fg(Color::Purple)],
            major: green.bold(),
            minor: green,
            user_you: yellow.bold(),
            user_other: Style::NONE,
            user_root: Style::NONE,
            punctuation: Style::fg(Color::DarkGray).bold(),
            date: Style::fg(Color::Blue),
            header: Style::NONE.underline(),
            symlink_path: Style::fg(Color::Cyan),
            control_char: red,
            broken_symlink: red,
            broken_path_overlay: Style::NONE.underline(),
            types,
            exts: Mappings::default(),
            use_types: true,
        }
    }

    pub(super) fn new(color: bool, ls_colors: Option<&str>, eza_colors: Option<&str>) -> Theme {
        if !color {
            return Theme::plain();
        }
        let mut theme = Theme::colourful();
        let mut exts = Mappings::default();
        if let Some(ls) = ls_colors {
            for (key, value) in pairs(ls) {
                let style = parse_style(value);
                if !theme.set_ls(key, style) {
                    exts.add(key, style);
                }
            }
        }
        if let Some(eza) = eza_colors {
            if eza == "reset" || eza.starts_with("reset:") {
                theme.use_types = false;
            }
            for (key, value) in pairs(eza) {
                let style = parse_style(value);
                if !theme.set_ls(key, style) && !theme.set_eza(key, style) {
                    exts.add(key, style);
                }
            }
        }
        theme.exts = exts;
        theme
    }

    fn set_ls(&mut self, key: &str, style: Style) -> bool {
        match key {
            "di" => self.directory = style,
            "ex" => self.executable = style,
            "fi" => self.normal = style,
            "pi" => self.pipe = style,
            "so" => self.socket = style,
            "bd" => self.block_device = style,
            "cd" => self.char_device = style,
            "ln" => self.symlink = style,
            "or" => self.broken_symlink = style,
            _ => return false,
        }
        true
    }

    fn set_eza(&mut self, key: &str, style: Style) -> bool {
        match key {
            "ur" => self.user_read = style,
            "uw" => self.user_write = style,
            "ux" => self.user_execute_file = style,
            "ue" => self.user_execute_other = style,
            "gr" => self.group_read = style,
            "gw" => self.group_write = style,
            "gx" => self.group_execute = style,
            "tr" => self.other_read = style,
            "tw" => self.other_write = style,
            "tx" => self.other_execute = style,
            "su" => self.special_user_file = style,
            "sf" => self.special_other = style,
            "xa" => self.attribute = style,
            "sn" => self.number = [style; 5],
            "nb" => self.number[SIZE_BYTE] = style,
            "nk" => self.number[SIZE_KILO] = style,
            "nm" => self.number[SIZE_MEGA] = style,
            "ng" => self.number[SIZE_GIGA] = style,
            "nt" => self.number[SIZE_HUGE] = style,
            "df" => self.major = style,
            "ds" => self.minor = style,
            "uu" => self.user_you = style,
            "un" => self.user_other = style,
            "uR" => self.user_root = style,
            "xx" => self.punctuation = style,
            "da" => self.date = style,
            "hd" => self.header = style,
            "lp" => self.symlink_path = style,
            "cc" => self.control_char = style,
            "bO" => self.broken_path_overlay = style,
            "mp" => self.mount_point = style,
            "sp" => self.special = style,
            "im" => self.types[Class::Image as usize] = style,
            "vi" => self.types[Class::Video as usize] = style,
            "mu" => self.types[Class::Music as usize] = style,
            "lo" => self.types[Class::Lossless as usize] = style,
            "cr" => self.types[Class::Crypto as usize] = style,
            "do" => self.types[Class::Document as usize] = style,
            "co" => self.types[Class::Compressed as usize] = style,
            "tm" => self.types[Class::Temp as usize] = style,
            "cm" => self.types[Class::Compiled as usize] = style,
            "bu" => self.types[Class::Build as usize] = style,
            "sc" => self.types[Class::Source as usize] = style,
            "sb" | "ub" | "uk" | "um" | "ug" | "ut" | "gu" | "gn" | "gR" | "lc" | "lm" | "ga"
            | "gm" | "gd" | "gv" | "gt" | "gi" | "gc" | "Gm" | "Go" | "Gc" | "Gd" | "in" | "bl"
            | "oc" | "ff" | "Sn" | "Su" | "Sr" | "St" | "Sl" => {}
            _ => return false,
        }
        true
    }

    pub(super) fn size_number(&self, prefix: Option<usize>) -> Style {
        self.number[prefix.map_or(SIZE_BYTE, size_slot)]
    }

    pub(super) fn file_style(
        &self,
        name: &str,
        ext: Option<&str>,
        compiled: impl FnOnce() -> bool,
    ) -> Style {
        if let Some(style) = self.exts.get(name) {
            return style;
        }
        if self.use_types
            && let Some(class) = class_of(name, ext, compiled)
        {
            return self.types[class as usize];
        }
        self.normal
    }
}

fn size_slot(prefix: usize) -> usize {
    match prefix {
        1 => SIZE_KILO,
        2 => SIZE_MEGA,
        3 => SIZE_GIGA,
        _ => SIZE_HUGE,
    }
}

fn class_of(name: &str, ext: Option<&str>, compiled: impl FnOnce() -> bool) -> Option<Class> {
    if name
        .as_bytes()
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"readme"))
    {
        return Some(Class::Build);
    }
    if let Some(class) = filename_class(name) {
        return Some(class);
    }
    if let Some(class) = ext.and_then(extension_class) {
        return Some(class);
    }
    if name.ends_with('~') || (name.starts_with('#') && name.ends_with('#')) {
        return Some(Class::Temp);
    }
    compiled().then_some(Class::Compiled)
}

macro_rules! indexed {
    ($name:ident, $table:ident, $ty:ty) => {
        pub(super) fn $name(key: &str) -> Option<$ty> {
            static MAP: OnceLock<FxHashMap<&'static str, $ty>> = OnceLock::new();
            MAP.get_or_init(|| $table.iter().copied().collect())
                .get(key)
                .copied()
        }
    };
}

indexed!(filename_class, FILENAME_TYPES, Class);
indexed!(extension_class, EXTENSION_TYPES, Class);
indexed!(directory_icon, DIRECTORY_ICONS, char);
indexed!(filename_icon, FILENAME_ICONS, char);
indexed!(extension_icon, EXTENSION_ICONS, char);

fn pairs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.split(':').filter_map(|part| {
        let (key, value) = part.split_once('=')?;
        (!key.is_empty() && !value.is_empty() && !value.contains('=')).then_some((key, value))
    })
}

fn high_colour<'a>(
    tokens: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Option<Color> {
    match tokens.peek() {
        Some(&"5") => {
            tokens.next();
            tokens.next()?.parse().ok().map(Color::Fixed)
        }
        Some(&"2") => {
            tokens.next();
            let r = tokens.next()?.parse().ok()?;
            let g = tokens.next()?.parse().ok()?;
            let b = tokens.next()?.parse().ok()?;
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

fn parse_style(value: &str) -> Style {
    let mut style = Style::NONE;
    let mut tokens = value.split(';').peekable();
    while let Some(token) = tokens.next() {
        match token.trim_start_matches('0') {
            "1" => style = style.bold(),
            "2" => style = style.dimmed(),
            "3" => style = style.italic(),
            "4" => style = style.underline(),
            "5" => style = style.blink(),
            "7" => style = style.reverse(),
            "8" => style = style.hidden(),
            "9" => style = style.strikethrough(),
            "30" => style = style.with_fg(Color::Black),
            "31" => style = style.with_fg(Color::Red),
            "32" => style = style.with_fg(Color::Green),
            "33" => style = style.with_fg(Color::Yellow),
            "34" => style = style.with_fg(Color::Blue),
            "35" => style = style.with_fg(Color::Purple),
            "36" => style = style.with_fg(Color::Cyan),
            "37" => style = style.with_fg(Color::White),
            "90" => style = style.with_fg(Color::DarkGray),
            "91" => style = style.with_fg(Color::LightRed),
            "92" => style = style.with_fg(Color::LightGreen),
            "93" => style = style.with_fg(Color::LightYellow),
            "94" => style = style.with_fg(Color::LightBlue),
            "95" => style = style.with_fg(Color::LightPurple),
            "96" => style = style.with_fg(Color::LightCyan),
            "97" => style = style.with_fg(Color::LightGray),
            "38" => {
                if let Some(color) = high_colour(&mut tokens) {
                    style = style.with_fg(color);
                }
            }
            "40" => style = style.on(Color::Black),
            "41" => style = style.on(Color::Red),
            "42" => style = style.on(Color::Green),
            "43" => style = style.on(Color::Yellow),
            "44" => style = style.on(Color::Blue),
            "45" => style = style.on(Color::Purple),
            "46" => style = style.on(Color::Cyan),
            "47" => style = style.on(Color::White),
            "100" => style = style.on(Color::DarkGray),
            "101" => style = style.on(Color::LightRed),
            "102" => style = style.on(Color::LightGreen),
            "103" => style = style.on(Color::LightYellow),
            "104" => style = style.on(Color::LightBlue),
            "105" => style = style.on(Color::LightPurple),
            "106" => style = style.on(Color::LightCyan),
            "107" => style = style.on(Color::LightGray),
            "48" => {
                if let Some(color) = high_colour(&mut tokens) {
                    style = style.on(color);
                }
            }
            _ => {}
        }
    }
    style
}

#[derive(Default)]
struct Mappings {
    groups: Vec<Group>,
}

enum Group {
    Simple(HashMap<String, Style>),
    Complex(Glob, Style),
}

impl Mappings {
    fn add(&mut self, pattern: &str, style: Style) {
        let simple = pattern
            .strip_prefix("*.")
            .filter(|ext| !ext.contains(['?', '*', '[', ']', '.']));
        match (self.groups.last_mut(), simple) {
            (Some(Group::Simple(map)), Some(ext)) => {
                map.insert(ext.to_string(), style);
            }
            (_, Some(ext)) => self
                .groups
                .push(Group::Simple(HashMap::from([(ext.to_string(), style)]))),
            (_, None) => self.groups.push(Group::Complex(Glob::new(pattern), style)),
        }
    }

    fn get(&self, name: &str) -> Option<Style> {
        let ext = name.rsplit_once('.').map(|(_, ext)| ext);
        for group in self.groups.iter().rev() {
            match group {
                Group::Complex(glob, style) if glob.matches(name) => return Some(*style),
                Group::Simple(map) => {
                    if let Some(style) = ext.and_then(|ext| map.get(ext)) {
                        return Some(*style);
                    }
                }
                Group::Complex(..) => {}
            }
        }
        None
    }
}

enum Token {
    Literal(char),
    One,
    Many,
    Set {
        negate: bool,
        ranges: Vec<(char, char)>,
    },
}

struct Glob(Vec<Token>);

impl Glob {
    fn new(pattern: &str) -> Glob {
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut at = 0;
        while at < chars.len() {
            match chars[at] {
                '?' => tokens.push(Token::One),
                '*' => {
                    while chars.get(at + 1) == Some(&'*') {
                        at += 1;
                    }
                    tokens.push(Token::Many);
                }
                '[' => {
                    let negate = matches!(chars.get(at + 1), Some('!'));
                    let start = (at + 1 + usize::from(negate)).min(chars.len());
                    match chars[start..].iter().position(|&c| c == ']') {
                        Some(len) if len > 0 => {
                            tokens.push(Token::Set {
                                negate,
                                ranges: set_ranges(&chars[start..start + len]),
                            });
                            at = start + len;
                        }
                        _ => tokens.push(Token::Literal('[')),
                    }
                }
                c => tokens.push(Token::Literal(c)),
            }
            at += 1;
        }
        Glob(tokens)
    }

    fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().collect();
        let (mut t, mut p) = (0, 0);
        let mut backtrack: Option<(usize, usize)> = None;
        while t < text.len() {
            let advanced = match self.0.get(p) {
                Some(Token::Many) => {
                    backtrack = Some((p, t));
                    p += 1;
                    continue;
                }
                Some(Token::One) => true,
                Some(Token::Literal(c)) => *c == text[t],
                Some(Token::Set { negate, ranges }) => {
                    ranges
                        .iter()
                        .any(|&(lo, hi)| lo <= text[t] && text[t] <= hi)
                        != *negate
                }
                None => false,
            };
            if advanced {
                t += 1;
                p += 1;
            } else if let Some((star, from)) = backtrack {
                backtrack = Some((star, from + 1));
                p = star + 1;
                t = from + 1;
            } else {
                return false;
            }
        }
        self.0[p..].iter().all(|token| matches!(token, Token::Many))
    }
}

fn set_ranges(chars: &[char]) -> Vec<(char, char)> {
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars.get(at + 1) == Some(&'-') && at + 2 < chars.len() {
            ranges.push((chars[at], chars[at + 2]));
            at += 3;
        } else {
            ranges.push((chars[at], chars[at]));
            at += 1;
        }
    }
    ranges
}
