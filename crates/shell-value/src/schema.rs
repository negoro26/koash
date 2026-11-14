use std::fmt;
use std::sync::Arc;

#[derive(Debug, Clone)]
enum Name {
    Static(&'static str),
    Shared(Arc<str>),
}

/// One column of a [`Schema`]. Cloning never copies the name.
#[derive(Debug, Clone)]
pub struct Column(Name);

impl Column {
    pub const fn new(name: &'static str) -> Self {
        Column(Name::Static(name))
    }

    pub fn name(&self) -> &str {
        match &self.0 {
            Name::Static(name) => name,
            Name::Shared(name) => name,
        }
    }
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

/// The ordered columns of a batch. A builtin with a fixed row declares them once as a `static`
/// and every batch it makes shares that slice; names only known at run time share one `Arc`.
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
            .map(|name| Column(Name::Shared(Arc::from(name.as_ref()))))
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

    /// The listed columns in the listed order. A column listed twice appears twice.
    pub fn project(&self, cols: &[usize]) -> Self {
        let columns = self.columns();
        Schema(Columns::Shared(
            cols.iter().map(|&col| columns[col].clone()).collect(),
        ))
    }
}

impl From<Vec<Column>> for Schema {
    fn from(columns: Vec<Column>) -> Self {
        Schema(Columns::Shared(columns.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static FIXED: [Column; 2] = [Column::new("a"), Column::new("b")];

    #[test]
    fn a_static_and_a_named_schema_with_the_same_names_agree() {
        let fixed = Schema::of(&FIXED);
        let named = Schema::named(["a", "b"]);
        assert_eq!(fixed.columns(), named.columns());
        assert_eq!(named.index_of("b"), Some(1));
        assert_eq!(named.index_of("c"), None);
    }

    #[test]
    fn project_reorders_and_repeats() {
        let names: Vec<String> = Schema::of(&FIXED)
            .project(&[1, 0, 1])
            .names()
            .map(str::to_string)
            .collect();
        assert_eq!(names, ["b", "a", "b"]);
    }
}
