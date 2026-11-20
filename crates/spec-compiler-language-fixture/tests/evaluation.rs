//! The generated evaluator and checker: the kernel's expression test
//! (`expressions_use_three_valued_logic`, crates/kernel/tests/behaviour.rs) and evaluation tables
//! for every primitive, against the semantics of crates/kernel/src/expr.rs (`eval`, `holds`,
//! `check`), with the kernel's error texts.
//!
//! The environment here is a small in-memory world that reads records the way the kernel does:
//! a field name must be a field id (`is_name` in crates/kernel/src/ids.rs), an unset field is
//! null, and following a reference needs a lookup and a single reference field.

use std::collections::{BTreeMap, BTreeSet};

use strata_spec_compiler_language_fixture::rule_language::{
    check, eval, holds, parse, Env, EvalError, Record, Shape, TypeEnv,
};
use strata_spec_compiler_language_fixture::types::{Expr, Scope, Value};

/// The kernel's field id rule: a lower-case letter, then lower-case letters, digits and `_`, at
/// most 64 bytes.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && s.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn field_id_error(name: &str) -> String {
    format!("invalid field identifier: {name:?}")
}

#[derive(Clone)]
struct Rec {
    id: u64,
    version: i64,
    created_by: Option<String>,
    created_at: i64,
    updated_at: i64,
    fields: BTreeMap<String, Value>,
}

impl Record for Rec {
    fn system_attribute(&self, name: &str) -> Value {
        match name {
            "$id" => Value::Ref(self.id),
            "$version" => Value::Int(self.version),
            "$created_at" => Value::Time(self.created_at),
            "$updated_at" => Value::Time(self.updated_at),
            "$created_by" => match &self.created_by {
                Some(p) => Value::Str(p.clone()),
                None => Value::Null,
            },
            _ => Value::Null,
        }
    }

    fn field(&self, name: &str) -> Result<Value, String> {
        if !is_name(name) {
            return Err(field_id_error(name));
        }
        Ok(self.fields.get(name).cloned().unwrap_or(Value::Null))
    }
}

struct World {
    record: Option<Rec>,
    old: Option<Rec>,
    new: Option<Rec>,
    params: BTreeMap<String, Value>,
    now: i64,
    principal: Option<String>,
    roles: BTreeSet<String>,
    /// The records a reference can be followed into; `None` when there is no lookup.
    targets: Option<BTreeMap<u64, Rec>>,
    /// The single reference fields of the scope records' object type.
    references: BTreeSet<String>,
}

impl Env for World {
    fn record(&self, scope: &Scope) -> Option<&dyn Record> {
        let record = match scope {
            Scope::Record => &self.record,
            Scope::Old => &self.old,
            Scope::New => &self.new,
        };
        record.as_ref().map(|r| r as &dyn Record)
    }

    fn follow(
        &self,
        _scope: &Scope,
        field: &str,
        target: &u64,
    ) -> Result<Option<Box<dyn Record + '_>>, EvalError> {
        let Some(targets) = &self.targets else {
            return Err(EvalError(format!("{field} cannot be followed here")));
        };
        if !self.references.contains(field) {
            return Err(EvalError(format!("{field} is not a reference")));
        }
        Ok(targets
            .get(target)
            .map(|r| Box::new(r.clone()) as Box<dyn Record + '_>))
    }

    fn param(&self, name: &str) -> Option<Value> {
        self.params.get(name).cloned()
    }

    fn now(&self) -> i64 {
        self.now
    }

    fn principal(&self) -> Option<String> {
        self.principal.clone()
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.contains(role)
    }
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn post() -> Rec {
    let fields = [
        ("title", text("Hello")),
        ("slug", text("hello-world")),
        ("rank", Value::Int(3)),
        ("tags", Value::List(vec![text("a"), text("b")])),
        ("author", Value::Ref(7)),
        ("dangling", Value::Ref(99)),
        ("other", Value::Ref(7)),
        ("published_at", Value::Time(100)),
    ];
    Rec {
        id: 1,
        version: 2,
        created_by: Some("u1".to_string()),
        created_at: 10,
        updated_at: 20,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

fn world() -> World {
    let author = Rec {
        id: 7,
        version: 1,
        created_by: None,
        created_at: 1,
        updated_at: 1,
        fields: [("principal", text("u1")), ("name", text("Ann"))]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    };
    World {
        record: Some(post()),
        old: None,
        new: Some(post()),
        params: [("q".to_string(), text("ell"))].into_iter().collect(),
        now: 500,
        principal: Some("u1".to_string()),
        roles: ["editor".to_string()].into_iter().collect(),
        targets: Some([(7, author)].into_iter().collect()),
        references: ["author", "dangling", "category"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

fn run_in(world: &World, source: &str) -> Result<Value, String> {
    let expr = parse(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    eval(&expr, world).map_err(|e| e.0)
}

fn run(source: &str) -> Result<Value, String> {
    run_in(&world(), source)
}

/// `crates/kernel/tests/behaviour.rs`, `expressions_use_three_valued_logic`.
#[test]
fn expressions_use_three_valued_logic() {
    let world = World {
        record: None,
        old: None,
        new: None,
        params: BTreeMap::new(),
        now: 0,
        principal: None,
        roles: BTreeSet::new(),
        targets: None,
        references: BTreeSet::new(),
    };
    let null = Expr::Lit(Value::Null);
    let t = Expr::Lit(Value::Bool(true));
    let f = Expr::Lit(Value::Bool(false));
    let lit = |s: &str| Expr::Lit(text(s));
    let eq = |a: Expr, b: Expr| Expr::Cmp {
        op: strata_spec_compiler_language_fixture::types::CmpOp::Eq,
        left: Box::new(a),
        right: Box::new(b),
    };
    assert_eq!(eval(&eq(null.clone(), lit("x")), &world), Ok(Value::Null));
    assert_eq!(
        eval(&Expr::And(vec![null.clone(), f.clone()]), &world),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        eval(&Expr::And(vec![null.clone(), t.clone()]), &world),
        Ok(Value::Null)
    );
    assert_eq!(
        eval(&Expr::Or(vec![null.clone(), t]), &world),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        eval(&Expr::Or(vec![null.clone(), f]), &world),
        Ok(Value::Null)
    );
    assert!(
        eval(&eq(lit("x"), Expr::Lit(Value::Int(1))), &world).is_err(),
        "a type error is an error, not false"
    );
    assert_eq!(
        eval(&Expr::IsSlug(Box::new(lit("a-good-slug"))), &world),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        eval(&Expr::IsSlug(Box::new(lit("Bad Slug"))), &world),
        Ok(Value::Bool(false))
    );
}

#[test]
fn every_primitive_evaluates_as_the_kernel_does() {
    let t = Ok(Value::Bool(true));
    let f = Ok(Value::Bool(false));
    let null = Ok(Value::Null);
    let err = |m: &str| Err(m.to_string());
    let table: Vec<(&str, Result<Value, String>)> = vec![
        // Literals.
        ("null", null.clone()),
        (
            "[1, 'a', time(2), ref(3)]",
            Ok(Value::List(vec![
                Value::Int(1),
                text("a"),
                Value::Time(2),
                Value::Ref(3),
            ])),
        ),
        // len, lower.
        ("len(new.title)", Ok(Value::Int(5))),
        ("len('h\u{e9}llo')", Ok(Value::Int(5))),
        ("len(new.tags)", Ok(Value::Int(2))),
        ("len(null)", null.clone()),
        ("len(new.rank)", err("len does not apply to a int")),
        ("lower('\u{c0}B')", Ok(text("\u{e0}b"))),
        ("lower(null)", null.clone()),
        ("lower(3)", err("lower does not apply to a int")),
        // contains, starts_with, is_slug.
        ("contains(new.title, 'ell')", t.clone()),
        ("contains(new.tags, 'b')", t.clone()),
        ("contains(new.tags, 1)", f.clone()),
        ("contains(null, 'x')", null.clone()),
        ("contains(new.title, null)", null.clone()),
        (
            "contains(new.title, 3)",
            err("contains does not apply to a string"),
        ),
        ("contains(3, 'x')", err("contains does not apply to a int")),
        ("starts_with(new.title, 'He')", t.clone()),
        ("starts_with(new.title, 'el')", f.clone()),
        ("starts_with(null, 'x')", null.clone()),
        (
            "starts_with(new.rank, 'x')",
            err("starts_with does not apply to a int"),
        ),
        ("is_slug(new.slug)", t.clone()),
        ("is_slug('-a')", f.clone()),
        ("is_slug('a--b')", f.clone()),
        ("is_slug('')", f.clone()),
        ("is_slug(null)", null.clone()),
        ("is_slug(1)", err("is_slug does not apply to a int")),
        // in.
        ("new.rank in [1, 2, 3]", t.clone()),
        ("'z' in new.tags", f.clone()),
        ("1 in ['1']", f.clone()),
        ("null in [1]", null.clone()),
        ("1 in null", null.clone()),
        ("1 in 'abc'", err("in does not apply to a string")),
        // Comparisons.
        ("new.rank < 4", t.clone()),
        ("new.rank <= 2", f.clone()),
        ("new.rank > 3", f.clone()),
        ("new.rank >= 3", t.clone()),
        ("new.rank != 3", f.clone()),
        ("'a' < 'b'", t.clone()),
        ("'b' <= 'a'", f.clone()),
        ("time(1) < now", t.clone()),
        ("ref(7) == new.author", t.clone()),
        ("[1, 2] == [1, 2]", t.clone()),
        ("null == null", null.clone()),
        ("null != null", null.clone()),
        ("null < 1", null.clone()),
        ("new.missing == null", null.clone()),
        ("1 < 'a'", err("cannot compare a int with a string")),
        ("1 == 'a'", err("cannot compare a int with a string")),
        (
            "ref(1) < ref(2)",
            err("ordering does not apply to a reference"),
        ),
        ("true < false", err("ordering does not apply to a bool")),
        ("[1] < [2]", err("ordering does not apply to a list")),
        // not, and, or, is_null, if.
        ("not true", f.clone()),
        ("not null", null.clone()),
        ("not 1", err("not does not apply to a int")),
        ("false and 1", f.clone()),
        ("true and 1", err("and does not apply to a int")),
        ("null and false", f.clone()),
        ("null and true", null.clone()),
        ("true or 1", t.clone()),
        ("false or 1", err("or does not apply to a int")),
        ("null or false", null.clone()),
        ("is_null(new.missing)", t.clone()),
        ("is_null(1)", f.clone()),
        ("is_null(len(null))", t.clone()),
        ("if(null, 1, 2)", Ok(Value::Int(2))),
        ("if(false, 1, 2)", Ok(Value::Int(2))),
        ("if(true, 1, 'x')", Ok(Value::Int(1))),
        ("if(true, 1, len(5))", Ok(Value::Int(1))),
        ("if(1, 2, 3)", err("if does not apply to a int")),
        // Scopes.
        ("param.q", Ok(text("ell"))),
        ("param.absent", null.clone()),
        ("now", Ok(Value::Time(500))),
        ("principal", Ok(text("u1"))),
        ("has_role(editor)", t.clone()),
        ("has_role(author)", f.clone()),
        ("new.$id", Ok(Value::Ref(1))),
        ("new.$version", Ok(Value::Int(2))),
        ("new.$created_by", Ok(text("u1"))),
        ("new.$created_at", Ok(Value::Time(10))),
        ("record.$updated_at", Ok(Value::Time(20))),
        ("old.title", null.clone()),
        ("old.$id", null.clone()),
        ("new.Bad", err("unknown attribute Bad")),
        ("new.$nope", err("unknown attribute $nope")),
        // One reference hop.
        ("new.author.principal", Ok(text("u1"))),
        ("new.author.$id", Ok(Value::Ref(7))),
        ("new.author.$created_by", null.clone()),
        ("new.author.Bad", err("unknown attribute Bad")),
        ("new.category.name", null.clone()),
        ("new.dangling.name", null.clone()),
        ("old.author.name", null.clone()),
        (
            "new.title.name",
            err("following a reference does not apply to a string"),
        ),
        (
            "new.tags.name",
            err("following a reference does not apply to a list"),
        ),
        ("new.other.name", err("other is not a reference")),
        ("new.Bad.name", err(&field_id_error("Bad"))),
    ];
    let mut failures = Vec::new();
    for (source, expected) in table {
        let got = run(source);
        if got != expected {
            failures.push(format!("{source}: expected {expected:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_reference_is_followed_only_where_there_is_a_lookup() {
    let mut world = world();
    world.targets = None;
    assert_eq!(
        run_in(&world, "new.author.name"),
        Err("author cannot be followed here".to_string())
    );
    // A null reference is null before any lookup is needed.
    assert_eq!(run_in(&world, "new.category.name"), Ok(Value::Null));
}

#[test]
fn only_true_holds() {
    let world = world();
    let holds_src = |s: &str| holds(&parse(s).unwrap(), &world);
    assert_eq!(holds_src("true"), Ok(true));
    assert_eq!(holds_src("null == 1"), Ok(false));
    assert_eq!(holds_src("false"), Ok(false));
    assert_eq!(holds_src("1"), Ok(false));
    assert!(holds_src("not 1").is_err());
    assert_eq!(holds(&Expr::And(vec![]), &world), Ok(true));
    assert_eq!(holds(&Expr::Or(vec![]), &world), Ok(false));
}

/// A definition site: the post's fields, the author object a reference reaches, the declared
/// parameters and roles.
struct Site {
    fields: BTreeMap<String, Shape>,
    references: BTreeMap<String, BTreeMap<String, Shape>>,
    params: BTreeSet<String>,
    roles: BTreeSet<String>,
}

impl TypeEnv for Site {
    fn field_shape(&self, through: Option<&str>, name: &str) -> Result<Shape, String> {
        if !is_name(name) {
            return Err(field_id_error(name));
        }
        let fields = match through {
            None => &self.fields,
            Some(field) => self
                .references
                .get(field)
                .ok_or_else(|| format!("{field} is not a single reference"))?,
        };
        fields
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown field {name}"))
    }

    fn follow(&self, field: &str) -> Result<(), String> {
        if !is_name(field) {
            return Err(field_id_error(field));
        }
        if !self.fields.contains_key(field) {
            return Err(format!("unknown field {field}"));
        }
        if !self.references.contains_key(field) {
            return Err(format!("{field} is not a single reference"));
        }
        Ok(())
    }

    fn has_param(&self, name: &str) -> bool {
        self.params.contains(name)
    }

    fn has_role(&self, role: &str) -> bool {
        self.roles.contains(role)
    }
}

fn site() -> Site {
    let shapes = |pairs: &[(&str, Shape)]| -> BTreeMap<String, Shape> {
        pairs.iter().map(|(k, s)| (k.to_string(), *s)).collect()
    };
    Site {
        fields: shapes(&[
            ("title", Shape::Str),
            ("rank", Shape::Int),
            ("tags", Shape::List),
            ("author", Shape::Ref),
            ("published_at", Shape::Time),
            ("featured", Shape::Bool),
        ]),
        references: [(
            "author".to_string(),
            shapes(&[("name", Shape::Str), ("principal", Shape::Str)]),
        )]
        .into_iter()
        .collect(),
        params: ["q".to_string()].into_iter().collect(),
        roles: ["editor".to_string()].into_iter().collect(),
    }
}

#[test]
fn the_check_gives_shapes_and_refuses_as_the_kernel_does() {
    let site = site();
    let err = |m: &str| Err(m.to_string());
    let table: Vec<(&str, Result<Shape, String>)> = vec![
        ("len(new.title) <= 120", Ok(Shape::Bool)),
        ("new.title", Ok(Shape::Str)),
        ("new.$id", Ok(Shape::Ref)),
        ("old.$created_by", Ok(Shape::Str)),
        ("new.$version", Ok(Shape::Int)),
        ("new.author.name", Ok(Shape::Str)),
        ("new.author.$updated_at", Ok(Shape::Time)),
        ("null", Ok(Shape::Any)),
        ("[1, 'a']", Ok(Shape::List)),
        ("time(3)", Ok(Shape::Time)),
        ("now", Ok(Shape::Time)),
        ("principal", Ok(Shape::Str)),
        ("param.q", Ok(Shape::Any)),
        ("has_role(editor)", Ok(Shape::Bool)),
        ("null < 1", Ok(Shape::Bool)),
        ("param.q == 1", Ok(Shape::Bool)),
        ("contains(new.tags, 1)", Ok(Shape::Bool)),
        ("if(true, null, 'a')", Ok(Shape::Str)),
        ("if(true, 1, null)", Ok(Shape::Int)),
        ("if(new.featured, new.rank, 0)", Ok(Shape::Int)),
        ("new.nope", err("unknown field nope")),
        ("new.Bad", err(&field_id_error("Bad"))),
        ("new.title.name", err("title is not a single reference")),
        ("new.nope.name", err("unknown field nope")),
        ("new.author.nope", err("unknown field nope")),
        ("param.z", err("undeclared parameter z")),
        ("has_role(ghost)", err("unknown role ghost")),
        ("not 1", err("not does not accept a Int operand")),
        ("1 and true", err("and/or does not accept a Int operand")),
        ("true or 'a'", err("and/or does not accept a Str operand")),
        ("1 == 'a'", err("cannot compare Int with Str")),
        (
            "true < false",
            err("ordering does not accept a Bool operand"),
        ),
        (
            "ref(1) < null",
            err("ordering does not accept a Ref operand"),
        ),
        ("1 in 2", err("in does not accept a Int operand")),
        ("len(1)", err("len does not accept a Int operand")),
        ("lower(1)", err("lower does not accept a Int operand")),
        ("is_slug(1)", err("is_slug does not accept a Int operand")),
        (
            "contains(new.title, 1)",
            err("contains does not accept a Int operand"),
        ),
        (
            "contains(1, 'a')",
            err("contains does not accept a Int operand"),
        ),
        ("if(1, 2, 3)", err("if does not accept a Int operand")),
        ("if(true, 1, 'a')", err("if branches differ: Int and Str")),
        // The kernel checks both operands of contains before their shapes, and each operand of
        // starts_with before the next.
        ("contains(1, new.nope)", err("unknown field nope")),
        (
            "starts_with(1, new.nope)",
            err("starts_with does not accept a Int operand"),
        ),
        ("1 == new.nope", err("unknown field nope")),
        ("new.nope and 1", err("unknown field nope")),
        (
            "1 and new.nope",
            err("and/or does not accept a Int operand"),
        ),
    ];
    let mut failures = Vec::new();
    for (source, expected) in table {
        let expr = parse(source).unwrap_or_else(|e| panic!("{source}: {e}"));
        let got = check(&expr, &site);
        if got != expected {
            failures.push(format!("{source}: expected {expected:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
