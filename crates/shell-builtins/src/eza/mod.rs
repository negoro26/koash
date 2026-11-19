mod natord;
mod style;
mod tables;
mod theme;
mod view;

pub(crate) use view::{Entry, Environment, Folder, Layout, Settings, Sort, read_children, render};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Image,
    Video,
    Music,
    Lossless,
    Crypto,
    Document,
    Compressed,
    Temp,
    Compiled,
    Build,
    Source,
}
