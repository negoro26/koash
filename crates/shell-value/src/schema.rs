use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColType {
    Any,
    Timestamp,
    Mode,
}

#[derive(Debug, Clone)]
enum Name {
    Static(&'static str),
    Shared(Arc<str>),
}

#[derive(Debug, Clone)]
pub struct Column {
    name: Name,
    ty: ColType,
    text: bool,
}

impl Column {
    pub const fn new(name: &'static str) -> Self {
        Self::typed(name, ColType::Any)
    }

    pub const fn typed(name: &'static str, ty: ColType) -> Self {
        Column {
            name: Name::Static(name),
            ty,
            text: false,
        }
    }

    pub const fn text(mut self) -> Self {
        self.text = true;
        self
    }

    pub fn is_text(&self) -> bool {
        self.text
    }

    pub fn name(&self) -> &str {
        match &self.name {
            Name::Static(name) => name,
            Name::Shared(name) => name,
        }
    }

    pub fn ty(&self) -> ColType {
        self.ty
    }
}

#[allow(clippy::panic)]
pub const fn column_index(columns: &[Column], name: &str) -> usize {
    let mut at = 0;
    while at < columns.len() {
        if let Name::Static(candidate) = columns[at].name
            && bytes_equal(candidate.as_bytes(), name.as_bytes())
        {
            return at;
        }
        at += 1;
    }
    panic!("no such column")
}

const fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut at = 0;
    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }
        at += 1;
    }
    true
}

impl PartialEq for Column {
    fn eq(&self, other: &Self) -> bool {
        self.name() == other.name()
    }
}

impl Eq for Column {}

impl fmt::Display for Column {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone)]
enum Columns {
    Static(&'static [Column]),
    Shared(Arc<[Column]>),
}

#[derive(Debug, Clone)]
pub struct Schema(Columns);

impl Schema {
    pub const fn empty() -> Self {
        Schema(Columns::Static(&[]))
    }

    pub const fn of(columns: &'static [Column]) -> Self {
        Schema(Columns::Static(columns))
    }

    pub fn named<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Self {
        let columns: Arc<[Column]> = names
            .into_iter()
            .map(|name| Column {
                name: Name::Shared(Arc::from(name.as_ref())),
                ty: ColType::Any,
                text: false,
            })
            .collect();
        Schema(Columns::Shared(columns))
    }

    pub fn columns(&self) -> &[Column] {
        match &self.0 {
            Columns::Static(columns) => columns,
            Columns::Shared(columns) => columns,
        }
    }

    pub fn len(&self) -> usize {
        self.columns().len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns().is_empty()
    }

    pub fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.columns().iter().map(Column::name)
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns().iter().position(|c| c.name() == name)
    }

    pub fn text_column(&self) -> Option<usize> {
        self.columns().iter().position(Column::is_text)
    }

    pub fn project(&self, cols: &[usize]) -> Self {
        let columns = self.columns();
        Schema(Columns::Shared(
            cols.iter()
                .map(|&col| Column {
                    text: false,
                    ..columns[col].clone()
                })
                .collect(),
        ))
    }
}

impl From<Vec<Column>> for Schema {
    fn from(columns: Vec<Column>) -> Self {
        Schema(Columns::Shared(columns.into()))
    }
}
