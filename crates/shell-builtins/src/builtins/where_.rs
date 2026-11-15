use shell_kernel::Session;
use shell_value::{Datum, StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Where;

static SIGNATURE: Signature = Signature {
    name: "where",
    aliases: &[],
    description: "Filter structured records by predicate",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

#[derive(Clone, Copy)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
}

impl Cmp {
    fn parse(op: &str) -> Option<Self> {
        Some(match op {
            "==" | "=" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            "=~" | "contains" => Self::Contains,
            _ => return None,
        })
    }

    fn holds<T: PartialOrd>(self, lhs: T, rhs: T) -> bool {
        match self {
            Self::Eq => lhs == rhs,
            Self::Ne => lhs != rhs,
            Self::Lt => lhs < rhs,
            Self::Le => lhs <= rhs,
            Self::Gt => lhs > rhs,
            Self::Ge => lhs >= rhs,
            Self::Contains => false,
        }
    }
}

impl Builtin for Where {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let [field, op, rhs_raw] = args else {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "where requires `<field> <op> <value>`",
                None,
                Some("example: ls | where size > 10kb".to_string()),
            ));
        };
        let Some(cmp) = Cmp::parse(op) else {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                format!("where: unknown operator `{op}`"),
                None,
                Some("operators: == != < <= > >= =~".to_string()),
            ));
        };
        let mut batch = input.into_batch();
        let Some(col_idx) = batch.col_index(field) else {
            batch.truncate_rows(0);
            return Ok(Value::from_batch(batch));
        };

        let predicate = Predicate::new(cmp, rhs_raw);
        let mut scratch = String::new();
        batch.retain_rows(|row| predicate.matches(row.get(col_idx), &mut scratch));
        Ok(Value::from_batch(batch))
    }
}

struct Predicate<'a> {
    cmp: Cmp,
    rhs_raw: &'a str,
    rhs_num: Option<f64>,
}

impl<'a> Predicate<'a> {
    fn new(cmp: Cmp, rhs_raw: &'a str) -> Self {
        Self {
            cmp,
            rhs_raw,
            rhs_num: parse_size_or_number(rhs_raw),
        }
    }

    fn matches(&self, lhs: Datum<'_>, scratch: &mut String) -> bool {
        if !matches!(self.cmp, Cmp::Contains)
            && let Some(lhs_num) = lhs.as_f64()
            && let Some(rhs_num) = self.rhs_num
        {
            return self.cmp.holds(lhs_num, rhs_num);
        }

        let lhs_str = lhs.text(scratch);
        match self.cmp {
            Cmp::Contains => lhs_str.contains(self.rhs_raw),
            _ => self.cmp.holds(lhs_str, self.rhs_raw),
        }
    }
}

fn parse_size_or_number(s: &str) -> Option<f64> {
    let lower = s.trim().to_ascii_lowercase();
    if let Ok(n) = lower.parse::<f64>() {
        return Some(n);
    }
    let suffixes = [
        ("gb", 1024.0 * 1024.0 * 1024.0),
        ("mb", 1024.0 * 1024.0),
        ("kb", 1024.0),
        ("b", 1.0),
    ];
    for (suf, mult) in suffixes {
        if let Some(num_part) = lower.strip_suffix(suf)
            && let Ok(n) = num_part.trim().parse::<f64>()
        {
            return Some(n * mult);
        }
    }
    None
}
