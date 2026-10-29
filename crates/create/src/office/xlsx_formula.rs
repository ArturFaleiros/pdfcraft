//! The formulas of conditional formatting rules (`expression` type), evaluated on the saved
//! cell values: numbers, text and booleans; `+ - * / ^ &`, comparisons; cell references
//! (relative ones move with the cell the rule is tested on, as in Excel) and ranges; and the
//! functions such rules use most: `AND OR NOT IF IFERROR`, `MOD ABS INT ROUND`, `ISBLANK
//! ISNUMBER ISTEXT ISERROR ISEVEN ISODD`, `SEARCH FIND LEN LEFT RIGHT MID UPPER LOWER TRIM EXACT`,
//! `ROW COLUMN TODAY`, and `SUM AVERAGE MIN MAX COUNT COUNTA COUNTIF` over ranges. Anything else
//! fails to parse, and the rule is left out (and reported).

use super::xlsx_cf::Cells;

/// Deepest nesting and longest formula read.
const MAX_DEPTH: usize = 64;
const MAX_CHARS: usize = 4_096;
/// Most cells one range in a formula may cover.
const MAX_RANGE: usize = 100_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Num(f64),
    Text(String),
    Bool(bool),
    Empty,
    Error,
}

impl Value {
    fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Empty => Some(0.0),
            Value::Text(t) => t.trim().parse().ok(),
            Value::Error => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Value::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{n:.0}")
                } else {
                    n.to_string()
                }
            }
            Value::Text(t) => t.clone(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Value::Empty | Value::Error => String::new(),
        }
    }

    pub(crate) fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Text(t) => t.eq_ignore_ascii_case("true"),
            Value::Empty | Value::Error => false,
        }
    }
}

/// A cell reference: row and column, and whether each is absolute (`$`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ref {
    row: i64,
    col: i64,
    abs_row: bool,
    abs_col: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Lit(Value),
    Cell(Ref),
    Range(Ref, Ref),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Cmp(&'static str, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// A parsed formula, with the cell its relative references are written from.
#[derive(Clone, Debug)]
pub(crate) struct Formula {
    expr: Expr,
    origin: (u32, u32),
}

struct Parser {
    chars: Vec<char>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, s: &str) -> bool {
        self.skip_ws();
        let n = s.chars().count();
        let next: String = self.chars.get(self.at..self.at.saturating_add(n).min(self.chars.len())).unwrap_or_default().iter().collect();
        if next == s {
            self.at += n;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn comparison(&mut self) -> Option<Expr> {
        self.deeper()?;
        let left = self.concat()?;
        for op in ["<=", ">=", "<>", "<", ">", "="] {
            if self.eat(op) {
                let right = self.concat()?;
                self.depth -= 1;
                return Some(Expr::Cmp(op, Box::new(left), Box::new(right)));
            }
        }
        self.depth -= 1;
        Some(left)
    }

    fn concat(&mut self) -> Option<Expr> {
        let mut left = self.additive()?;
        while self.eat("&") {
            left = Expr::Bin('&', Box::new(left), Box::new(self.additive()?));
        }
        Some(left)
    }

    fn additive(&mut self) -> Option<Expr> {
        let mut left = self.term()?;
        loop {
            if self.eat("+") {
                left = Expr::Bin('+', Box::new(left), Box::new(self.term()?));
            } else if self.eat("-") {
                left = Expr::Bin('-', Box::new(left), Box::new(self.term()?));
            } else {
                return Some(left);
            }
        }
    }

    fn term(&mut self) -> Option<Expr> {
        let mut left = self.power()?;
        loop {
            if self.eat("*") {
                left = Expr::Bin('*', Box::new(left), Box::new(self.power()?));
            } else if self.eat("/") {
                left = Expr::Bin('/', Box::new(left), Box::new(self.power()?));
            } else {
                return Some(left);
            }
        }
    }

    fn power(&mut self) -> Option<Expr> {
        let base = self.unary()?;
        if self.eat("^") { Some(Expr::Bin('^', Box::new(base), Box::new(self.unary()?))) } else { Some(base) }
    }

    fn unary(&mut self) -> Option<Expr> {
        self.deeper()?;
        let e = if self.eat("-") {
            Expr::Neg(Box::new(self.unary()?))
        } else if self.eat("+") {
            self.unary()?
        } else {
            self.primary()?
        };
        self.depth -= 1;
        Some(e)
    }

    fn cell_ref(&mut self) -> Option<Ref> {
        self.skip_ws();
        let start = self.at;
        let abs_col = self.eat("$");
        let mut col = 0i64;
        let mut letters = 0;
        while let Some(c) = self.peek().filter(char::is_ascii_alphabetic) {
            col = col * 26 + i64::from(c.to_ascii_uppercase() as u8 - b'A' + 1);
            letters += 1;
            self.at += 1;
            if letters > 3 {
                self.at = start;
                return None;
            }
        }
        let abs_row = self.peek() == Some('$');
        if abs_row {
            self.at += 1;
        }
        let mut row = 0i64;
        let mut digits = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            row = row * 10 + i64::from(d);
            digits += 1;
            self.at += 1;
            if digits > 7 {
                break;
            }
        }
        // A function name follows with `(`: not a reference.
        if letters == 0 || digits == 0 || self.peek() == Some('(') || self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            self.at = start;
            return None;
        }
        Some(Ref { row, col, abs_row, abs_col })
    }

    fn primary(&mut self) -> Option<Expr> {
        self.skip_ws();
        let c = self.peek()?;
        if c == '(' {
            self.at += 1;
            let e = self.comparison()?;
            return self.eat(")").then_some(e);
        }
        if c == '"' {
            self.at += 1;
            let mut s = String::new();
            loop {
                let ch = self.peek()?;
                self.at += 1;
                if ch == '"' {
                    if self.peek() == Some('"') {
                        s.push('"');
                        self.at += 1;
                        continue;
                    }
                    break;
                }
                s.push(ch);
            }
            return Some(Expr::Lit(Value::Text(s)));
        }
        if c.is_ascii_digit() || c == '.' {
            let start = self.at;
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.' || c == 'E' || c == 'e') {
                self.at += 1;
            }
            let s: String = self.chars.get(start..self.at)?.iter().collect();
            return s.parse().ok().map(|v| Expr::Lit(Value::Num(v)));
        }
        if let Some(a) = self.cell_ref() {
            if self.eat(":") {
                let b = self.cell_ref()?;
                return Some(Expr::Range(a, b));
            }
            return Some(Expr::Cell(a));
        }
        // A name: TRUE, FALSE or a function.
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
            self.at += 1;
        }
        let name: String = self.chars.get(start..self.at)?.iter().collect::<String>().to_ascii_uppercase();
        if name.is_empty() {
            return None;
        }
        if !self.eat("(") {
            return match name.as_str() {
                "TRUE" => Some(Expr::Lit(Value::Bool(true))),
                "FALSE" => Some(Expr::Lit(Value::Bool(false))),
                _ => None,
            };
        }
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                args.push(self.comparison()?);
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") && !self.eat(";") {
                    return None;
                }
                if args.len() > 64 {
                    return None;
                }
            }
        }
        const KNOWN: [&str; 33] = [
            "AND", "OR", "NOT", "IF", "IFERROR", "MOD", "ABS", "INT", "ROUND", "ISBLANK", "ISNUMBER", "ISTEXT", "ISERROR", "ISEVEN", "ISODD",
            "SEARCH", "FIND", "LEN", "LEFT", "RIGHT", "MID", "UPPER", "LOWER", "TRIM", "EXACT", "ROW", "COLUMN", "TODAY", "SUM", "AVERAGE", "MIN",
            "MAX", "COUNT",
        ];
        let name = name.trim_start_matches("_XLFN.").to_owned();
        (KNOWN.contains(&name.as_str()) || matches!(name.as_str(), "COUNTA" | "COUNTIF")).then_some(Expr::Call(name, args))
    }
}

/// Parse a rule's formula written for the cell `origin` (its range's top-left cell). `None` when
/// it uses something not supported.
pub(crate) fn parse(text: &str, origin: (u32, u32)) -> Option<Formula> {
    let chars: Vec<char> = text.trim().trim_start_matches('=').chars().take(MAX_CHARS + 1).collect();
    if chars.len() > MAX_CHARS {
        return None;
    }
    let mut p = Parser { chars, at: 0, depth: 0 };
    let expr = p.comparison()?;
    p.skip_ws();
    (p.at == p.chars.len()).then_some(Formula { expr, origin })
}

struct Eval<'a, 'b> {
    cells: &'a Cells<'b>,
    /// The cell tested, minus the formula's origin: how far relative references move.
    dr: i64,
    dc: i64,
    at: (u32, u32),
    today: f64,
}

impl Eval<'_, '_> {
    fn place(&self, r: Ref) -> Option<(u32, u32)> {
        let row = if r.abs_row { r.row } else { r.row + self.dr };
        let col = if r.abs_col { r.col } else { r.col + self.dc };
        Some((u32::try_from(row).ok().filter(|v| *v >= 1)?, u32::try_from(col).ok().filter(|v| *v >= 1)?))
    }

    fn cell(&self, r: Ref) -> Value {
        let Some(k) = self.place(r) else { return Value::Error };
        match self.cells.get(&k) {
            Some(v) => match v.number {
                Some(n) => Value::Num(n),
                None if v.text.is_empty() => Value::Empty,
                None => Value::Text(v.text.to_owned()),
            },
            None => Value::Empty,
        }
    }

    /// The values of a range, or of a single value.
    fn values(&self, e: &Expr) -> Vec<Value> {
        match e {
            Expr::Range(a, b) => {
                let (Some((r1, c1)), Some((r2, c2))) = (self.place(*a), self.place(*b)) else { return vec![Value::Error] };
                let (r1, r2, c1, c2) = (r1.min(r2), r1.max(r2), c1.min(c2), c1.max(c2));
                let count = (u64::from(r2 - r1) + 1) * (u64::from(c2 - c1) + 1);
                if count > MAX_RANGE as u64 {
                    return vec![Value::Error];
                }
                let mut out = Vec::new();
                for r in r1..=r2 {
                    for c in c1..=c2 {
                        out.push(self.cells.get(&(r, c)).map_or(Value::Empty, |v| match v.number {
                            Some(n) => Value::Num(n),
                            None if v.text.is_empty() => Value::Empty,
                            None => Value::Text(v.text.to_owned()),
                        }));
                    }
                }
                out
            }
            e => vec![self.eval(e)],
        }
    }

    fn eval(&self, e: &Expr) -> Value {
        match e {
            Expr::Lit(v) => v.clone(),
            Expr::Cell(r) => self.cell(*r),
            Expr::Range(a, _) => self.cell(*a),
            Expr::Neg(x) => self.eval(x).num().map_or(Value::Error, |n| Value::Num(-n)),
            Expr::Bin('&', a, b) => Value::Text(format!("{}{}", self.eval(a).text(), self.eval(b).text())),
            Expr::Bin(op, a, b) => {
                let (Some(x), Some(y)) = (self.eval(a).num(), self.eval(b).num()) else { return Value::Error };
                let v = match op {
                    '+' => x + y,
                    '-' => x - y,
                    '*' => x * y,
                    '/' if y != 0.0 => x / y,
                    '^' => x.powf(y),
                    _ => return Value::Error,
                };
                if v.is_finite() { Value::Num(v) } else { Value::Error }
            }
            Expr::Cmp(op, a, b) => {
                let (x, y) = (self.eval(a), self.eval(b));
                if x == Value::Error || y == Value::Error {
                    return Value::Error;
                }
                let ord = match (&x, &y) {
                    (Value::Text(_), _) | (_, Value::Text(_)) => x.text().to_lowercase().cmp(&y.text().to_lowercase()),
                    _ => x.num().unwrap_or(0.0).total_cmp(&y.num().unwrap_or(0.0)),
                };
                use std::cmp::Ordering::*;
                Value::Bool(match *op {
                    "=" => ord == Equal,
                    "<>" => ord != Equal,
                    "<" => ord == Less,
                    ">" => ord == Greater,
                    "<=" => ord != Greater,
                    _ => ord != Less,
                })
            }
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    fn call(&self, name: &str, args: &[Expr]) -> Value {
        let arg = |i: usize| args.get(i).map_or(Value::Empty, |a| self.eval(a));
        let num = |i: usize| arg(i).num();
        let text = |i: usize| arg(i).text();
        let all = || args.iter().flat_map(|a| self.values(a)).collect::<Vec<Value>>();
        let nums = || all().into_iter().filter_map(|v| if let Value::Num(n) = v { Some(n) } else { None }).collect::<Vec<f64>>();
        match name {
            "AND" => Value::Bool(all().iter().all(Value::truthy)),
            "OR" => Value::Bool(all().iter().any(Value::truthy)),
            "NOT" => Value::Bool(!arg(0).truthy()),
            "IF" => {
                if arg(0).truthy() {
                    arg(1)
                } else {
                    args.get(2).map_or(Value::Bool(false), |a| self.eval(a))
                }
            }
            "IFERROR" => {
                let v = arg(0);
                if v == Value::Error { arg(1) } else { v }
            }
            "MOD" => match (num(0), num(1)) {
                (Some(a), Some(b)) if b != 0.0 => Value::Num(a - b * (a / b).floor()),
                _ => Value::Error,
            },
            "ABS" => num(0).map_or(Value::Error, |v| Value::Num(v.abs())),
            "INT" => num(0).map_or(Value::Error, |v| Value::Num(v.floor())),
            "ROUND" => match (num(0), num(1)) {
                (Some(v), Some(d)) => {
                    let k = 10f64.powi(d.clamp(-15.0, 15.0) as i32);
                    Value::Num((v * k).round() / k)
                }
                _ => Value::Error,
            },
            "ISBLANK" => Value::Bool(arg(0) == Value::Empty),
            "ISNUMBER" => Value::Bool(matches!(arg(0), Value::Num(_))),
            "ISTEXT" => Value::Bool(matches!(arg(0), Value::Text(_))),
            "ISERROR" => Value::Bool(arg(0) == Value::Error),
            "ISEVEN" => num(0).map_or(Value::Error, |v| Value::Bool((v.trunc() as i64) % 2 == 0)),
            "ISODD" => num(0).map_or(Value::Error, |v| Value::Bool((v.trunc() as i64) % 2 != 0)),
            "SEARCH" | "FIND" => {
                let (needle, hay) = (text(0), text(1));
                let found = if name == "SEARCH" { hay.to_lowercase().find(&needle.to_lowercase()) } else { hay.find(&needle) };
                found.map_or(Value::Error, |i| Value::Num(hay.get(..i).map_or(0, |p| p.chars().count()) as f64 + 1.0))
            }
            "LEN" => Value::Num(text(0).chars().count() as f64),
            "LEFT" | "RIGHT" => {
                let s = text(0);
                let n = num(1).unwrap_or(1.0).clamp(0.0, 32_767.0) as usize;
                let len = s.chars().count();
                Value::Text(if name == "LEFT" { s.chars().take(n).collect() } else { s.chars().skip(len.saturating_sub(n)).collect() })
            }
            "MID" => {
                let s = text(0);
                let start = num(1).unwrap_or(1.0).clamp(1.0, 32_767.0) as usize;
                let n = num(2).unwrap_or(0.0).clamp(0.0, 32_767.0) as usize;
                Value::Text(s.chars().skip(start - 1).take(n).collect())
            }
            "UPPER" => Value::Text(text(0).to_uppercase()),
            "LOWER" => Value::Text(text(0).to_lowercase()),
            "TRIM" => Value::Text(text(0).split_whitespace().collect::<Vec<_>>().join(" ")),
            "EXACT" => Value::Bool(text(0) == text(1)),
            "ROW" => match args.first() {
                Some(Expr::Cell(r) | Expr::Range(r, _)) => self.place(*r).map_or(Value::Error, |(r, _)| Value::Num(f64::from(r))),
                _ => Value::Num(f64::from(self.at.0)),
            },
            "COLUMN" => match args.first() {
                Some(Expr::Cell(r) | Expr::Range(r, _)) => self.place(*r).map_or(Value::Error, |(_, c)| Value::Num(f64::from(c))),
                _ => Value::Num(f64::from(self.at.1)),
            },
            "TODAY" => Value::Num(self.today.floor()),
            "SUM" => Value::Num(nums().iter().sum()),
            "AVERAGE" => {
                let v = nums();
                if v.is_empty() { Value::Error } else { Value::Num(v.iter().sum::<f64>() / v.len() as f64) }
            }
            "MIN" => Value::Num(nums().into_iter().reduce(f64::min).unwrap_or(0.0)),
            "MAX" => Value::Num(nums().into_iter().reduce(f64::max).unwrap_or(0.0)),
            "COUNT" => Value::Num(nums().len() as f64),
            "COUNTA" => Value::Num(all().iter().filter(|v| **v != Value::Empty).count() as f64),
            "COUNTIF" => {
                let Some(range) = args.first() else { return Value::Error };
                let crit = arg(1);
                let (op, target) = match &crit {
                    Value::Text(t) => {
                        let ops = ["<=", ">=", "<>", "<", ">", "="];
                        match ops.iter().find(|o| t.starts_with(**o)) {
                            Some(o) => (*o, Value::Text(t.get(o.len()..).unwrap_or_default().to_owned())),
                            None => ("=", crit.clone()),
                        }
                    }
                    v => ("=", v.clone()),
                };
                let count = self
                    .values(range)
                    .into_iter()
                    .filter(|v| {
                        let e = Expr::Cmp(
                            op,
                            Box::new(Expr::Lit(v.clone())),
                            Box::new(Expr::Lit(match (&target, target.num()) {
                                (Value::Text(_), Some(n)) => Value::Num(n),
                                _ => target.clone(),
                            })),
                        );
                        self.eval(&e).truthy()
                    })
                    .count();
                Value::Num(count as f64)
            }
            _ => Value::Error,
        }
    }
}

impl Formula {
    /// The formula's value for the cell `at`.
    pub(crate) fn eval(&self, cells: &Cells<'_>, at: (u32, u32), today: f64) -> Value {
        let e = Eval { cells, dr: i64::from(at.0) - i64::from(self.origin.0), dc: i64::from(at.1) - i64::from(self.origin.1), at, today };
        e.eval(&self.expr)
    }
}

#[cfg(test)]
mod tests {
    use super::super::xlsx_cf::Value as Cv;
    use super::*;

    fn cells<'a>(v: &[((u32, u32), Option<f64>, &'a str)]) -> Cells<'a> {
        v.iter().map(|(k, n, t)| (*k, Cv { number: *n, text: t })).collect()
    }

    #[test]
    fn relative_references_follow_the_cell() {
        let c = cells(&[((1, 3), Some(4.0), "4"), ((2, 3), Some(5.0), "5"), ((1, 1), None, "lorem")]);
        let f = parse("MOD(C1,2)=0", (1, 3)).unwrap();
        assert_eq!(f.eval(&c, (1, 3), 0.0), Value::Bool(true));
        assert_eq!(f.eval(&c, (2, 3), 0.0), Value::Bool(false));
        let g = parse("AND($C$1>3, NOT(ISERROR(SEARCH(\"or\", A1))))", (1, 1)).unwrap();
        assert!(g.eval(&c, (1, 1), 0.0).truthy());
        let h = parse("SUM(C1:C2)/COUNT(C1:C2)>=4.5", (1, 1)).unwrap();
        assert!(h.eval(&c, (1, 1), 0.0).truthy());
        assert!(parse("COUNTIF($C$1:$C$2,\">4\")=1", (1, 1)).unwrap().eval(&c, (1, 1), 0.0).truthy());
    }

    #[test]
    fn unsupported_and_hostile_formulas_are_refused() {
        assert!(parse("VLOOKUP(A1,B:C,2)", (1, 1)).is_none());
        assert!(parse("(((((", (1, 1)).is_none());
        assert!(parse(&"(".repeat(10_000), (1, 1)).is_none());
        assert!(parse(&"-".repeat(1_000), (1, 1)).is_none());
        let f = parse("1/0", (1, 1)).unwrap();
        assert_eq!(f.eval(&Cells::new(), (1, 1), 0.0), Value::Error);
        let r = parse("SUM(A1:XFD1048576)", (1, 1)).unwrap();
        assert_eq!(r.eval(&Cells::new(), (1, 1), 0.0), Value::Num(0.0));
    }
}
