//! Hand-built trees exercising the printer, the visitors, the lvalue rules
//! and the node sizes.

use std::mem::size_of;

use rphp_intern::{IdentId, Interner};
use rphp_span::{FileId, Span};

use super::lvalue::{self, LvalueCtx as C, LvalueErrorKind as K, SpecialVars};
use super::pretty::{self, Options};
use super::visit::{self, Visitor, VisitorMut};
use super::*;

const S: Span = Span::dummy();

fn sp(lo: u32, hi: u32) -> Span {
    Span::new(FileId(0), lo, hi)
}

// ----- builders --------------------------------------------------------------

struct B {
    it: Interner,
}

impl B {
    fn new() -> Self {
        B {
            it: Interner::new(),
        }
    }
    fn id(&mut self, s: &str) -> IdentId {
        self.it.intern_str(s)
    }
    fn var(&mut self, s: &str) -> Expr {
        Expr::Var(self.id(s), S)
    }
    fn str_(&mut self, s: &str) -> Expr {
        Expr::Str(self.id(s), S)
    }
    fn name(&mut self, s: &str, kind: NameKind) -> Name {
        Name::new(self.id(s), kind, S)
    }
    fn uq(&mut self, s: &str) -> Name {
        self.name(s, NameKind::Unqualified)
    }
    fn q(&mut self, s: &str) -> Name {
        self.name(s, NameKind::Qualified)
    }
    fn member(&mut self, s: &str) -> MemberName {
        MemberName::Ident(self.id(s), S)
    }
    fn prop(&mut self, obj: Expr, name: &str, nullsafe: bool) -> Expr {
        Expr::Prop {
            obj: Box::new(obj),
            name: self.member(name),
            nullsafe,
            span: S,
        }
    }
    fn sprop(&mut self, class: ClassRef, name: &str) -> Expr {
        Expr::StaticProp {
            class,
            name: self.member(name),
            span: S,
        }
    }
    fn cls(&mut self, s: &str) -> ClassRef {
        ClassRef::Named(self.uq(s))
    }
    fn call(&mut self, f: &str, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Callee::Name(self.uq(f)),
            args: args.into_iter().map(arg).collect(),
            span: S,
        }
    }
    fn mcall(&mut self, obj: Expr, m: &str, nullsafe: bool) -> Expr {
        Expr::MethodCall {
            obj: Box::new(obj),
            name: self.member(m),
            args: vec![],
            nullsafe,
            span: S,
        }
    }
    fn scall(&mut self, class: &str, m: &str) -> Expr {
        Expr::StaticCall {
            class: self.cls(class),
            name: self.member(m),
            args: vec![],
            span: S,
        }
    }
    fn new_(&mut self, class: &str) -> Expr {
        Expr::New {
            class: NewTarget::Ref(self.cls(class)),
            args: vec![],
            span: S,
        }
    }
    fn cconst(&mut self, class: &str, c: &str) -> Expr {
        Expr::ClassConst {
            class: self.cls(class),
            name: ConstSel::Ident(self.id(c), S),
            span: S,
        }
    }
    fn konst(&mut self, c: &str) -> Expr {
        Expr::Const(self.uq(c))
    }
    fn fcc(&mut self, f: &str) -> Expr {
        Expr::Callable {
            target: CallableTarget::Func(Callee::Name(self.uq(f))),
            span: S,
        }
    }
    fn varvar_str(&mut self, s: &str) -> Expr {
        Expr::VarVar {
            name: Box::new(self.str_(s)),
            span: S,
        }
    }
    fn param(
        &mut self,
        name: &str,
        ty: Option<Type>,
        default: Option<Expr>,
        promote: Option<Modifiers>,
    ) -> Param {
        Param {
            attrs: vec![],
            name: self.id(name),
            ty,
            default,
            by_ref: false,
            variadic: false,
            promote,
            hooks: vec![],
            span: S,
        }
    }
}

fn int(i: i64) -> Expr {
    Expr::Int(i, S)
}
fn index(base: Expr, idx: Expr) -> Expr {
    Expr::Index {
        base: Box::new(base),
        index: Some(Box::new(idx)),
        span: S,
    }
}
fn append(base: Expr) -> Expr {
    Expr::Index {
        base: Box::new(base),
        index: None,
        span: S,
    }
}
fn bin(op: BinOp, l: Expr, r: Expr) -> Expr {
    Expr::Binary {
        op,
        lhs: Box::new(l),
        rhs: Box::new(r),
        span: S,
    }
}
fn un(op: UnOp, e: Expr) -> Expr {
    Expr::Unary {
        op,
        expr: Box::new(e),
        span: S,
    }
}
fn ty(b: Builtin) -> Type {
    Type::builtin(b, S)
}
fn arg(value: Expr) -> Arg {
    Arg {
        name: None,
        value,
        spread: false,
        span: S,
    }
}
fn item(value: Expr) -> ArrayItem {
    ArrayItem {
        key: None,
        value: Some(value),
        by_ref: false,
        spread: false,
        span: S,
    }
}
fn skip() -> ArrayItem {
    ArrayItem {
        key: None,
        value: None,
        by_ref: false,
        spread: false,
        span: S,
    }
}
fn kitem(key: Expr, value: Expr) -> ArrayItem {
    ArrayItem {
        key: Some(key),
        value: Some(value),
        by_ref: false,
        spread: false,
        span: S,
    }
}
fn ref_item(value: Expr) -> ArrayItem {
    ArrayItem {
        by_ref: true,
        ..item(value)
    }
}
fn spread_item(value: Expr) -> ArrayItem {
    ArrayItem {
        spread: true,
        ..item(value)
    }
}
fn arr(items: Vec<ArrayItem>) -> Expr {
    arr_with(items, ArraySyntax::Short)
}
fn arr_with(items: Vec<ArrayItem>, syntax: ArraySyntax) -> Expr {
    Expr::Array {
        items,
        syntax,
        span: S,
    }
}
fn assign(target: Expr, value: Expr) -> Expr {
    Expr::Assign {
        target: Box::new(target),
        value: Box::new(value),
        op: None,
        by_ref: false,
        span: S,
    }
}
fn expr_stmt(e: Expr) -> Stmt {
    Stmt::Expr { expr: e, span: S }
}
fn vis(v: Visibility) -> Modifiers {
    Modifiers {
        vis: Some(v),
        ..Modifiers::default()
    }
}
fn program(items: Vec<Stmt>) -> Program {
    Program {
        file: FileId(0),
        strict_types: false,
        items,
        halt_offset: None,
    }
}

/// ```php
/// declare(strict_types=1);
/// namespace App\Models;
/// use App\Contracts\HasName as Named;
/// #[Entity(table: 'users')]
/// final class User extends Base implements Named {
///     public function __construct(public readonly string $name, private ?int $age = null) {}
///     public string $label {
///         get => strtoupper($this->name);
///         set(string $value) { $this->label = $value; }
///     }
/// }
/// ```
fn user_class_program(b: &mut B) -> Program {
    let declare = Stmt::Declare {
        directives: vec![Directive {
            name: b.id("strict_types"),
            value: int(1),
            span: S,
        }],
        body: None,
        span: S,
    };
    let use_ = Stmt::Use {
        kind: UseKind::Class,
        prefix: None,
        items: vec![UseItem {
            name: b.q("App\\Contracts\\HasName"),
            alias: Some(b.id("Named")),
            kind: None,
            span: S,
        }],
        span: S,
    };
    let attr = AttrGroup {
        attrs: vec![Attr {
            name: b.uq("Entity"),
            args: vec![Arg {
                name: Some(b.id("table")),
                value: b.str_("users"),
                spread: false,
                span: S,
            }],
            span: S,
        }],
        span: S,
    };
    let ctor = MethodDecl {
        attrs: vec![],
        modifiers: vis(Visibility::Public),
        by_ref: false,
        name: b.id("__construct"),
        params: vec![
            b.param(
                "name",
                Some(ty(Builtin::String)),
                None,
                Some(Modifiers {
                    readonly: true,
                    ..vis(Visibility::Public)
                }),
            ),
            b.param(
                "age",
                Some(Type {
                    kind: TypeKind::Nullable(Box::new(ty(Builtin::Int))),
                    span: S,
                }),
                Some(Expr::Null(S)),
                Some(vis(Visibility::Private)),
            ),
        ],
        ret: None,
        body: Some(vec![]),
        doc: None,
        span: S,
    };
    let this = b.var("this");
    let this_name = b.prop(this, "name", false);
    let get = Hook {
        kind: HookKind::Get,
        attrs: vec![],
        final_: false,
        by_ref: false,
        params: None,
        body: HookBody::Expr(b.call("strtoupper", vec![this_name])),
        span: S,
    };
    let this = b.var("this");
    let this_label = b.prop(this, "label", false);
    let value = b.var("value");
    let set = Hook {
        kind: HookKind::Set,
        attrs: vec![],
        final_: false,
        by_ref: false,
        params: Some(vec![b.param(
            "value",
            Some(ty(Builtin::String)),
            None,
            None,
        )]),
        body: HookBody::Block(vec![expr_stmt(assign(this_label, value))]),
        span: S,
    };
    let label = PropMember {
        attrs: vec![],
        modifiers: vis(Visibility::Public),
        ty: Some(ty(Builtin::String)),
        items: vec![PropItem {
            name: b.id("label"),
            default: None,
            span: S,
        }],
        hooks: vec![get, set],
        doc: None,
        span: S,
    };
    let class = ClassLike {
        kind: ClassKind::Class,
        attrs: vec![attr],
        modifiers: Modifiers {
            final_: true,
            ..Modifiers::default()
        },
        name: Some(b.id("User")),
        extends: vec![b.uq("Base")],
        implements: vec![b.uq("Named")],
        backing: None,
        members: vec![Member::Method(ctor), Member::Prop(label)],
        doc: None,
        span: S,
    };
    let ns = Stmt::Namespace {
        name: Some(b.q("App\\Models")),
        body: vec![use_, Stmt::ClassLike(class)],
        braced: false,
        span: S,
    };
    Program {
        strict_types: true,
        ..program(vec![declare, ns])
    }
}

/// `$r = match (true) { $x > 1, $x < -1 => 'big', default => 'small' };`
fn match_stmt(b: &mut B) -> Stmt {
    let x1 = b.var("x");
    let x2 = b.var("x");
    let big = b.str_("big");
    let small = b.str_("small");
    let r = b.var("r");
    let m = Expr::Match {
        subject: Box::new(Expr::Bool(true, S)),
        arms: vec![
            MatchArm {
                conds: Some(vec![
                    bin(BinOp::Gt, x1, int(1)),
                    bin(BinOp::Lt, x2, un(UnOp::Neg, int(1))),
                ]),
                body: big,
                span: S,
            },
            MatchArm {
                conds: None,
                body: small,
                span: S,
            },
        ],
        span: S,
    };
    expr_stmt(assign(r, m))
}

// ----- pretty ------------------------------------------------------------------

#[test]
fn pretty_program_snapshot() {
    let mut b = B::new();
    let prog = user_class_program(&mut b);
    let expected = r#"(program file=0 strict
  (declare
    (directive strict_types
      (int 1)))
  (namespace
    (name App\Models kind=q)
    (use kind=class
      (item alias=Named
        (name App\Contracts\HasName kind=q)))
    (class-like kind=class name=User mods=final
      (attr-group
        (attr Entity kind=u
          (arg name=table
            (str "users"))))
      (extends
        (name Base kind=u))
      (implements
        (name Named kind=u))
      (method __construct mods=public
        (params
          (param $name promote=public,readonly
            (type string))
          (param $age promote=private
            (nullable
              (type int))
            (default
              (null))))
        (body))
      (prop mods=public
        (type string)
        (item $label)
        (hook get
          (expr
            (call
              (name strtoupper kind=u)
              (arg
                (prop name=name
                  (var $this))))))
        (hook set
          (params
            (param $value
              (type string)))
          (body
            (expr-stmt
              (assign
                (prop name=label
                  (var $this))
                (var $value)))))))))"#;
    assert_eq!(pretty::print(&prog, &b.it), expected);
}

#[test]
fn pretty_match_snapshot() {
    let mut b = B::new();
    let s = match_stmt(&mut b);
    let expected = r#"(expr-stmt
  (assign
    (var $r)
    (match
      (bool true)
      (arm
        (conds
          (binary gt
            (var $x)
            (int 1))
          (binary lt
            (var $x)
            (unary neg
              (int 1))))
        (str "big"))
      (arm default
        (str "small")))))"#;
    assert_eq!(pretty::print_stmt(&s, &b.it, Options::default()), expected);
}

#[test]
fn pretty_destructuring_with_spans() {
    // `[$a, ['k' => &$b]] = f();` at byte offsets 0..25
    let mut b = B::new();
    let inner = Expr::Array {
        items: vec![ArrayItem {
            key: Some(Expr::Str(b.id("k"), sp(6, 9))),
            value: Some(Expr::Var(b.id("b"), sp(14, 16))),
            by_ref: true,
            spread: false,
            span: sp(6, 16),
        }],
        syntax: ArraySyntax::Short,
        span: sp(5, 17),
    };
    let outer = Expr::Array {
        items: vec![
            ArrayItem {
                key: None,
                value: Some(Expr::Var(b.id("a"), sp(1, 3))),
                by_ref: false,
                spread: false,
                span: sp(1, 3),
            },
            ArrayItem {
                key: None,
                value: Some(inner),
                by_ref: false,
                spread: false,
                span: sp(5, 17),
            },
        ],
        syntax: ArraySyntax::Short,
        span: sp(0, 18),
    };
    let call = Expr::Call {
        callee: Callee::Name(Name::new(b.id("f"), NameKind::Unqualified, sp(21, 22))),
        args: vec![],
        span: sp(21, 24),
    };
    let stmt = Stmt::Expr {
        expr: Expr::Assign {
            target: Box::new(outer),
            value: Box::new(call),
            op: None,
            by_ref: false,
            span: sp(0, 24),
        },
        span: sp(0, 25),
    };
    let expected = r#"(expr-stmt @0..25
  (assign @0..24
    (array syntax=short @0..18
      (item @1..3
        (var $a @1..3))
      (item @5..17
        (array syntax=short @5..17
          (item by-ref @6..16
            (key
              (str "k" @6..9))
            (var $b @14..16)))))
    (call @21..24
      (name f kind=u @21..22))))"#;
    assert_eq!(
        pretty::print_stmt(&stmt, &b.it, Options { spans: true }),
        expected
    );
}

#[test]
fn pretty_interpolated_string_and_escapes() {
    // "Hi {$user->name}, you have $n items\n"
    let mut b = B::new();
    let user = b.var("user");
    let user_name = b.prop(user, "name", false);
    let n = b.var("n");
    let e = Expr::Interp {
        parts: vec![
            InterpPart::Lit(b.id("Hi "), S),
            InterpPart::Expr(user_name),
            InterpPart::Lit(b.id(", you have "), S),
            InterpPart::Expr(n),
            InterpPart::Lit(b.id(" items\n"), S),
        ],
        span: S,
    };
    let expected = r#"(interp
  (lit "Hi ")
  (prop name=name
    (var $user))
  (lit ", you have ")
  (var $n)
  (lit " items\n"))"#;
    assert_eq!(pretty::print_expr(&e, &b.it, Options::default()), expected);

    assert_eq!(
        pretty::escape_bytes(b"a\"b\\c\x00\xff\t\r"),
        r#""a\"b\\c\0\xff\t\r""#
    );
    // Non-bare identifiers (a variable with a space, from `${'a b'}`) are quoted.
    let weird = Expr::Var(b.id("a b"), S);
    assert_eq!(
        pretty::print_expr(&weird, &b.it, Options::default()),
        r#"(var $"a b")"#
    );
    let f = Expr::Float(1.5e300, S);
    assert_eq!(
        pretty::print_expr(&f, &b.it, Options::default()),
        "(float 1.5e300)"
    );
}

#[test]
fn pretty_resolved_names_and_hir_nodes() {
    let mut b = B::new();
    let mut n = b.q("Foo\\Bar");
    n.resolved = Some(Resolved::Class {
        fqn: b.id("App\\Foo\\Bar"),
        key: b.id("app\\foo\\bar"),
    });
    let e = Expr::ClassConst {
        class: ClassRef::Named(n),
        name: ConstSel::Class(S),
        span: S,
    };
    assert_eq!(
        pretty::print_expr(&e, &b.it, Options::default()),
        "(class-const const=class\n  (name Foo\\Bar kind=q resolved=class fqn=App\\Foo\\Bar key=app\\foo\\bar))"
    );
    let mut f = b.uq("strlen");
    f.resolved = Some(Resolved::Func {
        ns_key: Some(b.id("app\\strlen")),
        global_key: b.id("strlen"),
    });
    let e = Expr::Call {
        callee: Callee::Name(f),
        args: vec![],
        span: S,
    };
    assert_eq!(
        pretty::print_expr(&e, &b.it, Options::default()),
        "(call\n  (name strlen kind=u resolved=func ns=app\\strlen global=strlen))"
    );
    let a = b.var("a");
    let let_ = Expr::Let {
        temp: TempId(0),
        init: Box::new(a),
        body: Box::new(Expr::Seq(vec![Expr::Temp(TempId(0), S), Expr::Null(S)], S)),
        span: S,
    };
    assert_eq!(
        pretty::print_expr(&let_, &b.it, Options::default()),
        "(let t0\n  (var $a)\n  (seq\n    (temp t0)\n    (null)))"
    );
}

// ----- visit -------------------------------------------------------------------

#[derive(Default, Debug, PartialEq)]
struct Counts {
    stmts: usize,
    exprs: usize,
    names: usize,
    params: usize,
    hooks: usize,
    types: usize,
    members: usize,
    attr_groups: usize,
    args: usize,
}

impl Visitor for Counts {
    fn visit_stmt(&mut self, s: &Stmt) {
        self.stmts += 1;
        visit::walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &Expr) {
        self.exprs += 1;
        visit::walk_expr(self, e);
    }
    fn visit_name(&mut self, _: &Name) {
        self.names += 1;
    }
    fn visit_param(&mut self, p: &Param) {
        self.params += 1;
        visit::walk_param(self, p);
    }
    fn visit_hook(&mut self, h: &Hook) {
        self.hooks += 1;
        visit::walk_hook(self, h);
    }
    fn visit_type(&mut self, t: &Type) {
        self.types += 1;
        visit::walk_type(self, t);
    }
    fn visit_member(&mut self, m: &Member) {
        self.members += 1;
        visit::walk_member(self, m);
    }
    fn visit_attr_group(&mut self, g: &AttrGroup) {
        self.attr_groups += 1;
        visit::walk_attr_group(self, g);
    }
    fn visit_arg(&mut self, a: &Arg) {
        self.args += 1;
        visit::walk_arg(self, a);
    }
}

#[test]
fn visitor_counts_every_node_family() {
    let mut b = B::new();
    let prog = user_class_program(&mut b);
    let mut c = Counts::default();
    c.visit_program(&prog);
    assert_eq!(
        c,
        Counts {
            stmts: 5,  // declare, namespace, use, class, `$this->label = $value;`
            exprs: 10, // 1, 'users', null, strtoupper(..), $this->name, $this, assign, prop, $this, $value
            names: 6,  // App\Models, HasName, Entity, Base, Named, strtoupper
            params: 3, // $name, $age, set($value)
            hooks: 2,
            types: 5, // string, ?int, int, string, string
            members: 2,
            attr_groups: 1,
            args: 2, // table: 'users', strtoupper($this->name)
        }
    );

    let mut c = Counts::default();
    c.visit_stmt(&match_stmt(&mut b));
    assert_eq!(c.exprs, 13);
    assert_eq!(c.stmts, 1);

    // foreach ($xs as $k => [$a, $b]) { echo $a; }
    let pattern = arr(vec![item(b.var("a")), item(b.var("b"))]);
    let fe = Stmt::Foreach {
        subject: b.var("xs"),
        key: Some(Box::new(b.var("k"))),
        value: Box::new(pattern),
        by_ref: false,
        body: vec![Stmt::Echo {
            args: vec![b.var("a")],
            span: S,
        }],
        span: S,
    };
    let mut c = Counts::default();
    c.visit_stmt(&fe);
    assert_eq!((c.stmts, c.exprs), (2, 6));
    let expected = "(foreach
  (var $xs)
  (key
    (var $k))
  (value
    (array syntax=short
      (item
        (var $a))
      (item
        (var $b))))
  (body
    (echo
      (var $a))))";
    assert_eq!(pretty::print_stmt(&fe, &b.it, Options::default()), expected);
}

/// A stand-in resolver: marks every name as a resolved class and bumps every
/// integer literal, to prove `VisitorMut` rewrites in place and that `walk_*`
/// continues below an override.
struct Rewriter;

impl VisitorMut for Rewriter {
    fn visit_name(&mut self, n: &mut Name) {
        n.resolved = Some(Resolved::Class {
            fqn: n.text,
            key: n.text,
        });
    }
    fn visit_expr(&mut self, e: &mut Expr) {
        if let Expr::Int(v, _) = e {
            *v += 1;
        }
        visit::mutable::walk_expr(self, e);
    }
}

#[test]
fn visitor_mut_rewrites_in_place() {
    let mut b = B::new();
    let mut prog = user_class_program(&mut b);
    Rewriter.visit_program(&mut prog);
    let out = pretty::print(&prog, &b.it);
    assert!(
        out.contains("(directive strict_types\n      (int 2))"),
        "{out}"
    );
    assert!(
        out.contains("(name Base kind=u resolved=class fqn=Base key=Base)"),
        "{out}"
    );
    assert!(
        out.contains("(name strtoupper kind=u resolved=class fqn=strtoupper key=strtoupper)"),
        "{out}"
    );
    // Every name got resolved (6 names, 6 `resolved=` markers).
    assert_eq!(out.matches("resolved=class").count(), 6);

    let mut s = match_stmt(&mut b);
    Rewriter.visit_stmt(&mut s);
    let out = pretty::print_stmt(&s, &b.it, Options::default());
    assert_eq!(out.matches("(int 2)").count(), 2);
    assert_eq!(out.matches("(int 1)").count(), 0);
}

// ----- lvalue --------------------------------------------------------------------

fn sv(b: &mut B) -> SpecialVars {
    SpecialVars::intern(&mut b.it)
}

#[track_caller]
fn ok(e: &Expr, ctx: C, sv: &SpecialVars) {
    assert_eq!(
        lvalue::validate(e, ctx, sv),
        Ok(()),
        "{ctx:?} should accept {e:?}"
    );
}

#[track_caller]
fn bad(e: &Expr, ctx: C, sv: &SpecialVars, kind: K) {
    match lvalue::validate(e, ctx, sv) {
        Err(err) => assert_eq!(err.kind, kind, "{ctx:?} on {e:?}"),
        Ok(()) => panic!("{ctx:?} should reject {e:?} with {kind:?}"),
    }
}

#[test]
fn lvalue_accepts_plain_targets() {
    let mut b = B::new();
    let sv = sv(&mut b);
    ok(&b.var("a"), C::Assign, &sv);
    let vv = Expr::VarVar {
        name: Box::new(b.var("a")),
        span: S,
    };
    ok(&vv, C::Assign, &sv);
    ok(&append(b.var("a")), C::Assign, &sv);
    ok(&index(append(b.var("a")), int(0)), C::Assign, &sv);
    let e = append(b.var("a"));
    let e = b.prop(e, "x", false);
    ok(&e, C::Assign, &sv);
    let f = b.call("f", vec![]);
    let e = b.prop(f, "x", false);
    ok(&e, C::Assign, &sv);
    ok(&index(b.call("f", vec![]), int(0)), C::Assign, &sv);
    let sc = b.scall("A", "m");
    let e = b.prop(sc, "x", false);
    ok(&e, C::Assign, &sv);
    let c = b.cls("A");
    let e = b.sprop(c, "b");
    ok(&e, C::Assign, &sv);
    let c = b.cls("A");
    let e = index(b.sprop(c, "b"), int(0));
    ok(&e, C::Assign, &sv);
    let dyn_cls = ClassRef::Expr(Box::new(b.var("a")));
    let e = b.sprop(dyn_cls, "b");
    ok(&e, C::Assign, &sv);
    let this = b.var("this");
    let e = b.prop(this, "x", false);
    ok(&e, C::Assign, &sv);
    ok(&index(b.var("this"), int(0)), C::Assign, &sv);
    // nullsafe inside an *index* is an rvalue, not part of the chain
    let inner = b.var("b");
    let idx = b.prop(inner, "c", true);
    ok(&index(b.var("a"), idx), C::Assign, &sv);
    // `$GLOBALS['x']`, nested too
    ok(&index(b.var("GLOBALS"), b.str_("x")), C::Assign, &sv);
    ok(
        &index(index(b.var("GLOBALS"), b.str_("x")), b.str_("y")),
        C::Assign,
        &sv,
    );
    // HIR temporaries
    ok(&Expr::Temp(TempId(0), S), C::Assign, &sv);
    let e = b.prop(Expr::Temp(TempId(1), S), "x", false);
    ok(&e, C::Assign, &sv);
    ok(&Expr::Error(S), C::Assign, &sv);
    // compound / coalesce / inc-dec
    ok(&b.var("this"), C::CompoundAssign, &sv);
    ok(&append(b.var("a")), C::CompoundAssign, &sv);
    ok(&index(b.var("a"), int(0)), C::CoalesceAssign, &sv);
    // reference targets
    let e = b.var("a");
    let e = b.prop(e, "b", false);
    ok(&e, C::AssignRef, &sv);
    ok(&append(b.var("a")), C::AssignRef, &sv);
    let f = b.call("g", vec![]);
    let e = b.prop(f, "x", false);
    ok(&e, C::AssignRef, &sv);
    // foreach
    ok(&append(b.var("a")), C::Foreach, &sv);
    ok(&append(b.var("k")), C::ForeachKey, &sv);
    let e = b.var("a");
    let e = b.prop(e, "b", false);
    ok(&e, C::Foreach, &sv);
    // unset
    let this = b.var("this");
    let e = b.prop(this, "x", false);
    ok(&e, C::Unset, &sv);
    let c = b.cls("A");
    let e = b.sprop(c, "x");
    ok(&e, C::Unset, &sv);
    let f = b.call("g", vec![]);
    let e = b.prop(f, "x", false);
    ok(&e, C::Unset, &sv);
    ok(&index(b.var("GLOBALS"), b.str_("x")), C::Unset, &sv);
    ok(&vv, C::Unset, &sv);
    // global
    ok(&b.var("a"), C::Global, &sv);
    ok(&vv, C::Global, &sv);
    ok(&b.var("GLOBALS"), C::Global, &sv);
    // by-ref args: PHP checks these at runtime; referenceable shapes pass
    ok(&b.var("this"), C::ByRefArg, &sv);
    ok(&b.var("GLOBALS"), C::ByRefArg, &sv);
    ok(&b.call("g", vec![]), C::ByRefArg, &sv);
    ok(&append(b.var("a")), C::ByRefArg, &sv);
    let e = b.var("a");
    let e = b.prop(e, "b", false);
    ok(&e, C::ByRefArg, &sv);
}

#[test]
fn lvalue_accepts_destructuring_patterns() {
    let mut b = B::new();
    let sv = sv(&mut b);

    let nested = arr(vec![item(b.var("b")), item(b.var("c"))]);
    ok(&arr(vec![item(b.var("a")), item(nested)]), C::Assign, &sv);
    let k = (b.str_("x"), b.str_("y"));
    ok(
        &arr(vec![kitem(k.0, b.var("a")), kitem(k.1, b.var("b"))]),
        C::Assign,
        &sv,
    );
    ok(&arr(vec![skip(), item(b.var("a"))]), C::Assign, &sv);
    ok(&arr(vec![ref_item(b.var("a"))]), C::Assign, &sv);
    let kx = b.str_("x");
    ok(
        &arr(vec![kitem(kx, Expr::Var(b.id("a"), S))]),
        C::Assign,
        &sv,
    );
    ok(&arr(vec![item(append(b.var("a")))]), C::Assign, &sv);
    let f = b.call("f", vec![]);
    let e = b.prop(f, "x", false);
    ok(&arr(vec![item(e)]), C::Assign, &sv);
    ok(
        &arr(vec![item(index(b.call("f", vec![]), int(0)))]),
        C::Assign,
        &sv,
    );
    let kx = b.str_("x");
    let inner = arr(vec![kitem(kx, b.var("b"))]);
    ok(&arr(vec![item(b.var("a")), item(inner)]), C::Assign, &sv);
    let this = b.var("this");
    let e = b.prop(this, "x", false);
    ok(&arr(vec![item(e)]), C::Assign, &sv);
    // list() at the top level, nested list() inside list()
    let inner = arr_with(vec![item(b.var("b"))], ArraySyntax::List);
    ok(
        &arr_with(vec![item(b.var("a")), item(inner)], ArraySyntax::List),
        C::Assign,
        &sv,
    );
    // trailing skipped slot next to a real element is fine when unkeyed
    let inner = arr(vec![item(b.var("b")), skip()]);
    ok(&arr(vec![item(b.var("a")), item(inner)]), C::Assign, &sv);
    // foreach value and a single element via the List ctx
    ok(
        &arr(vec![item(b.var("a")), item(b.var("b"))]),
        C::Foreach,
        &sv,
    );
    ok(&index(b.var("a"), int(0)), C::List, &sv);
}

#[test]
fn lvalue_rejects_this_and_globals() {
    let mut b = B::new();
    let sv = sv(&mut b);
    let this = |b: &mut B| b.var("this");
    let globals = |b: &mut B| b.var("GLOBALS");

    bad(&this(&mut b), C::Assign, &sv, K::ThisReassign);
    bad(&this(&mut b), C::CoalesceAssign, &sv, K::ThisReassign);
    bad(&this(&mut b), C::AssignRef, &sv, K::ThisReassign);
    bad(&this(&mut b), C::Foreach, &sv, K::ThisReassign);
    bad(&this(&mut b), C::ForeachKey, &sv, K::ThisReassign);
    bad(&this(&mut b), C::Unset, &sv, K::ThisUnset);
    bad(&this(&mut b), C::Global, &sv, K::ThisGlobal);
    bad(
        &arr(vec![item(this(&mut b))]),
        C::Assign,
        &sv,
        K::ThisReassign,
    );
    bad(&this(&mut b), C::List, &sv, K::ThisReassign);
    // `${'this'}` is the same node as `$this` in Zend
    bad(&b.varvar_str("this"), C::Assign, &sv, K::ThisReassign);
    bad(&b.varvar_str("this"), C::Global, &sv, K::ThisGlobal);

    bad(&globals(&mut b), C::Assign, &sv, K::GlobalsWrite);
    bad(&globals(&mut b), C::CompoundAssign, &sv, K::GlobalsWrite);
    bad(&globals(&mut b), C::AssignRef, &sv, K::GlobalsWrite);
    bad(&globals(&mut b), C::Foreach, &sv, K::GlobalsWrite);
    bad(&globals(&mut b), C::Unset, &sv, K::GlobalsWrite);
    bad(
        &arr(vec![item(globals(&mut b))]),
        C::Assign,
        &sv,
        K::GlobalsWrite,
    );
    bad(&b.varvar_str("GLOBALS"), C::Assign, &sv, K::GlobalsWrite);
    bad(&append(globals(&mut b)), C::Assign, &sv, K::GlobalsAppend);
    bad(
        &index(append(globals(&mut b)), b.str_("x")),
        C::Assign,
        &sv,
        K::GlobalsAppend,
    );
    bad(
        &arr(vec![item(append(globals(&mut b)))]),
        C::Assign,
        &sv,
        K::GlobalsAppend,
    );

    // Without the names interned nothing is special.
    let none = SpecialVars::default();
    ok(&this(&mut b), C::Assign, &none);
    ok(&globals(&mut b), C::Assign, &none);
}

#[test]
fn lvalue_rejects_calls_temporaries_and_nullsafe() {
    let mut b = B::new();
    let sv = sv(&mut b);

    bad(&b.call("f", vec![]), C::Assign, &sv, K::FunctionResult);
    bad(&b.fcc("f"), C::Assign, &sv, K::FunctionResult);
    let a = b.var("a");
    bad(&b.mcall(a, "m", false), C::Assign, &sv, K::MethodResult);
    bad(&b.scall("A", "m"), C::Assign, &sv, K::MethodResult);
    bad(
        &b.call("f", vec![]),
        C::CompoundAssign,
        &sv,
        K::FunctionResult,
    );
    bad(&b.call("f", vec![]), C::Unset, &sv, K::FunctionResult);
    bad(&b.call("f", vec![]), C::Foreach, &sv, K::FunctionResult);
    bad(&b.call("f", vec![]), C::AssignRef, &sv, K::FunctionResult);

    // temporaries as the whole target or as the chain root
    bad(&int(1), C::Assign, &sv, K::Temporary);
    bad(
        &bin(BinOp::Add, b.var("a"), int(1)),
        C::Assign,
        &sv,
        K::Temporary,
    );
    let n = b.new_("A");
    let e = b.prop(n, "x", false);
    bad(&e, C::Assign, &sv, K::Temporary);
    bad(
        &index(arr(vec![item(int(1))]), int(0)),
        C::Assign,
        &sv,
        K::Temporary,
    );
    bad(&index(b.str_("abc"), int(0)), C::Assign, &sv, K::Temporary);
    bad(
        &index(b.cconst("A", "B"), int(0)),
        C::Assign,
        &sv,
        K::Temporary,
    );
    bad(&index(b.konst("FOO"), int(0)), C::Assign, &sv, K::Temporary);
    let y = Expr::Yield {
        key: None,
        value: None,
        span: S,
    };
    bad(&index(y, int(0)), C::Assign, &sv, K::Temporary);
    // the error points at the root, not the whole chain
    let root = Expr::Int(1, sp(3, 4));
    let chain = Expr::Index {
        base: Box::new(root),
        index: Some(Box::new(int(0))),
        span: sp(3, 7),
    };
    assert_eq!(
        lvalue::validate(&chain, C::Assign, &sv),
        Err(lvalue::LvalueError {
            kind: K::Temporary,
            span: sp(3, 4)
        })
    );

    // nullsafe anywhere in the chain, including the class of `::`
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&e, C::Assign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    let e = b.prop(e, "c", false);
    bad(&e, C::Assign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&index(e, int(0)), C::Assign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let cls = ClassRef::Expr(Box::new(b.prop(a, "b", true)));
    let e = b.sprop(cls, "c");
    bad(&e, C::Assign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.mcall(a, "m", true);
    let e = b.prop(e, "c", false);
    bad(&e, C::Assign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&e, C::Unset, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&e, C::CompoundAssign, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&e, C::AssignRef, &sv, K::NullsafeWrite);
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(&e, C::ByRefArg, &sv, K::NullsafeRef);
    let a = b.var("a");
    let e = b.mcall(a, "m", true);
    bad(&e, C::ByRefArg, &sv, K::NullsafeRef);
}

#[test]
fn lvalue_rejects_append_in_read_and_unset() {
    let mut b = B::new();
    let sv = sv(&mut b);

    bad(&append(b.var("a")), C::CoalesceAssign, &sv, K::AppendRead);
    bad(
        &index(append(b.var("a")), int(0)),
        C::CoalesceAssign,
        &sv,
        K::AppendRead,
    );
    bad(&append(b.var("a")), C::Unset, &sv, K::AppendUnset);
    bad(
        &index(append(b.var("a")), int(0)),
        C::Unset,
        &sv,
        K::AppendUnset,
    );
    let e = append(b.var("a"));
    let e = b.prop(e, "x", false);
    bad(&e, C::Unset, &sv, K::AppendUnset);
    // the error points at the innermost `[]`
    let inner = Expr::Index {
        base: Box::new(b.var("a")),
        index: None,
        span: sp(0, 4),
    };
    let outer = Expr::Index {
        base: Box::new(inner),
        index: Some(Box::new(int(0))),
        span: sp(0, 7),
    };
    assert_eq!(
        lvalue::validate(&outer, C::CoalesceAssign, &sv),
        Err(lvalue::LvalueError {
            kind: K::AppendRead,
            span: sp(0, 4)
        })
    );

    // rvalue positions
    assert_eq!(
        lvalue::validate_rvalue(&append(b.var("a"))).map_err(|e| e.kind),
        Err(K::AppendRead)
    );
    assert_eq!(
        lvalue::validate_rvalue(&index(append(b.var("a")), int(0))).map_err(|e| e.kind),
        Err(K::AppendRead)
    );
    assert_eq!(
        lvalue::validate_rvalue(&arr_with(vec![item(b.var("a"))], ArraySyntax::List))
            .map_err(|e| e.kind),
        Err(K::ListAsRvalue)
    );
    assert_eq!(lvalue::validate_rvalue(&index(b.var("a"), int(0))), Ok(()));
    assert_eq!(
        lvalue::validate_rvalue(&arr(vec![item(b.var("a"))])),
        Ok(())
    );
}

#[test]
fn lvalue_rejects_bad_patterns() {
    let mut b = B::new();
    let sv = sv(&mut b);

    bad(&arr(vec![]), C::Assign, &sv, K::EmptyList);
    bad(&arr(vec![skip()]), C::Assign, &sv, K::EmptyList);
    bad(
        &arr(vec![item(b.var("a")), item(arr(vec![]))]),
        C::Assign,
        &sv,
        K::EmptyList,
    );
    bad(
        &arr(vec![item(b.var("a")), item(arr(vec![skip(), skip()]))]),
        C::Assign,
        &sv,
        K::EmptyList,
    );
    let kx = b.str_("x");
    bad(
        &arr(vec![kitem(kx, b.var("a")), item(b.var("b"))]),
        C::Assign,
        &sv,
        K::MixedKeys,
    );
    let kx = b.str_("x");
    bad(
        &arr(vec![item(b.var("b")), kitem(kx, b.var("a"))]),
        C::Assign,
        &sv,
        K::MixedKeys,
    );
    let (kx, ky) = (b.str_("x"), b.str_("y"));
    bad(
        &arr(vec![kitem(kx, b.var("a")), skip(), kitem(ky, b.var("b"))]),
        C::Assign,
        &sv,
        K::SkippedInKeyed,
    );
    let kx = b.str_("x");
    bad(
        &arr(vec![skip(), kitem(kx, b.var("a"))]),
        C::Assign,
        &sv,
        K::SkippedInKeyed,
    );
    bad(
        &arr_with(vec![item(b.var("a"))], ArraySyntax::Long),
        C::Assign,
        &sv,
        K::LongArrayTarget,
    );
    let inner = arr_with(vec![item(b.var("b"))], ArraySyntax::List);
    bad(
        &arr(vec![item(b.var("a")), item(inner)]),
        C::Assign,
        &sv,
        K::MixedSyntax,
    );
    let inner = arr(vec![item(b.var("b"))]);
    bad(
        &arr_with(vec![item(b.var("a")), item(inner)], ArraySyntax::List),
        C::Assign,
        &sv,
        K::MixedSyntax,
    );
    bad(
        &arr(vec![spread_item(b.var("a"))]),
        C::Assign,
        &sv,
        K::SpreadInList,
    );
    bad(
        &arr(vec![ref_item(arr(vec![item(b.var("a"))]))]),
        C::Assign,
        &sv,
        K::RefOnNestedList,
    );
    let a = b.var("a");
    let e = b.prop(a, "b", true);
    bad(
        &arr(vec![item(e)]),
        C::Assign,
        &sv,
        K::ListElementNotWritable,
    );
    bad(
        &arr(vec![item(int(1))]),
        C::Assign,
        &sv,
        K::ListElementNotWritable,
    );
    let n = b.new_("A");
    let e = b.prop(n, "x", false);
    bad(
        &arr(vec![item(e)]),
        C::Assign,
        &sv,
        K::ListElementNotWritable,
    );
    bad(
        &arr(vec![item(b.call("f", vec![]))]),
        C::Assign,
        &sv,
        K::FunctionResult,
    );
    let a = b.var("a");
    let e = b.mcall(a, "m", false);
    bad(&arr(vec![item(e)]), C::Assign, &sv, K::MethodResult);
    // foreach key and the contexts whose grammar has no patterns
    bad(
        &arr(vec![item(b.var("a"))]),
        C::ForeachKey,
        &sv,
        K::ListAsKey,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::AssignRef,
        &sv,
        K::ListNotAllowed,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::CompoundAssign,
        &sv,
        K::ListNotAllowed,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::CoalesceAssign,
        &sv,
        K::ListNotAllowed,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::Unset,
        &sv,
        K::ListNotAllowed,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::ByRefArg,
        &sv,
        K::NotReferenceable,
    );
    bad(
        &arr(vec![item(b.var("a"))]),
        C::Global,
        &sv,
        K::NotSimpleVariable,
    );
}

#[test]
fn lvalue_global_and_by_ref_arg_shapes() {
    let mut b = B::new();
    let sv = sv(&mut b);

    bad(
        &index(b.var("a"), int(0)),
        C::Global,
        &sv,
        K::NotSimpleVariable,
    );
    let a = b.var("a");
    let e = b.prop(a, "b", false);
    bad(&e, C::Global, &sv, K::NotSimpleVariable);
    let c = b.cls("A");
    let e = b.sprop(c, "b");
    bad(&e, C::Global, &sv, K::NotSimpleVariable);

    bad(&int(1), C::ByRefArg, &sv, K::NotReferenceable);
    bad(&b.new_("A"), C::ByRefArg, &sv, K::NotReferenceable);
    bad(&b.fcc("f"), C::ByRefArg, &sv, K::NotReferenceable);
    bad(
        &index(arr(vec![item(int(1))]), int(0)),
        C::ByRefArg,
        &sv,
        K::NotReferenceable,
    );
    bad(
        &bin(BinOp::Add, b.var("a"), int(1)),
        C::ByRefArg,
        &sv,
        K::NotReferenceable,
    );
}

#[test]
fn lvalue_reference_sources() {
    let mut b = B::new();
    let sv = sv(&mut b);
    let src = |e: &Expr| lvalue::validate_ref_source(e, &sv).map_err(|e| e.kind);

    assert_eq!(src(&b.var("a")), Ok(()));
    assert_eq!(src(&append(b.var("a"))), Ok(()));
    assert_eq!(src(&b.call("f", vec![])), Ok(()));
    let a = b.var("b");
    assert_eq!(src(&b.mcall(a, "m", false)), Ok(()));
    assert_eq!(src(&b.scall("A", "m")), Ok(()));
    let c = b.cls("A");
    let e = b.sprop(c, "b");
    assert_eq!(src(&e), Ok(()));
    let vv = Expr::VarVar {
        name: Box::new(b.var("b")),
        span: S,
    };
    assert_eq!(src(&vv), Ok(()));
    assert_eq!(src(&index(b.var("GLOBALS"), b.str_("x"))), Ok(()));
    assert_eq!(src(&b.var("this")), Ok(()));
    assert_eq!(src(&Expr::Temp(TempId(3), S)), Ok(()));

    assert_eq!(src(&int(1)), Err(K::NotReferenceable));
    assert_eq!(
        src(&bin(BinOp::Add, b.var("b"), int(1))),
        Err(K::NotReferenceable)
    );
    assert_eq!(src(&b.new_("A")), Err(K::NotReferenceable));
    assert_eq!(src(&arr(vec![item(int(1))])), Err(K::NotReferenceable));
    assert_eq!(src(&b.fcc("f")), Err(K::NotReferenceable));
    let closure = Expr::ArrowFn(Box::new(ArrowFn {
        static_: false,
        by_ref: false,
        attrs: vec![],
        params: vec![],
        ret: None,
        body: int(1),
        doc: None,
        span: S,
    }));
    assert_eq!(src(&closure), Err(K::NotReferenceable));
    let a = b.var("b");
    let e = b.prop(a, "c", true);
    assert_eq!(src(&e), Err(K::NullsafeRef));
    let a = b.var("b");
    let e = b.mcall(a, "m", true);
    assert_eq!(src(&e), Err(K::NullsafeRef));
    assert_eq!(
        src(&index(arr(vec![item(int(1))]), int(0))),
        Err(K::Temporary)
    );
    assert_eq!(src(&b.var("GLOBALS")), Err(K::GlobalsRef));
    assert_eq!(src(&b.varvar_str("GLOBALS")), Err(K::GlobalsRef));
    assert_eq!(src(&append(b.var("GLOBALS"))), Err(K::GlobalsAppend));
}

#[test]
fn lvalue_codes_and_messages() {
    assert_eq!(K::Temporary.code(), "RPHP_E0020");
    assert_eq!(K::NullsafeWrite.code(), "RPHP_E0021");
    assert_eq!(K::ThisReassign.code(), "RPHP_E0022");
    assert_eq!(K::GlobalsAppend.code(), "RPHP_E0023");
    assert_eq!(K::AppendRead.code(), "RPHP_E0024");
    assert_eq!(K::MixedKeys.code(), "RPHP_E0025");
    assert_eq!(K::ThisReassign.message(), "Cannot re-assign $this");
    let e = lvalue::LvalueError {
        kind: K::AppendUnset,
        span: S,
    };
    assert_eq!(
        (e.code(), e.message()),
        (lvalue::E_APPEND, "Cannot use [] for unsetting")
    );

    let mut it = Interner::new();
    assert_eq!(SpecialVars::lookup(&it), SpecialVars::default());
    let sv = SpecialVars::intern(&mut it);
    assert_eq!(SpecialVars::lookup(&it), sv);
    assert_eq!(sv.this, it.get(b"this"));
}

// ----- misc helpers ------------------------------------------------------------------

#[test]
fn constant_shape_helper() {
    let mut b = B::new();
    assert!(int(1).is_constant_shape());
    assert!(b.konst("PHP_EOL").is_constant_shape());
    assert!(b.cconst("A", "B").is_constant_shape());
    let kx = b.str_("k");
    assert!(arr(vec![kitem(kx, bin(BinOp::Add, int(1), int(2)))]).is_constant_shape());
    assert!(b.new_("A").is_constant_shape());
    assert!(!b.var("a").is_constant_shape());
    assert!(!b.call("f", vec![]).is_constant_shape());
    assert!(!arr(vec![item(b.var("a"))]).is_constant_shape());
    assert!(!un(UnOp::PreInc, int(1)).is_constant_shape());
    assert!(int(1).is_literal());
    assert!(b.var("a").is_variable_like());
    assert!(b.call("f", vec![]).is_call());
    assert!(BinOp::Coalesce.has_compound_assign());
    assert!(!BinOp::Eq.has_compound_assign());
}

#[test]
fn spans_are_reachable_on_every_node() {
    let mut b = B::new();
    let e = Expr::Const(Name::new(b.id("X"), NameKind::FullyQualified, sp(1, 2)));
    assert_eq!(e.span(), sp(1, 2));
    let s = Stmt::Nop { span: sp(4, 5) };
    assert_eq!(s.span(), sp(4, 5));
    let c = ClassRef::Expr(Box::new(Expr::Var(b.id("x"), sp(7, 9))));
    assert_eq!(c.span(), sp(7, 9));
    assert_eq!(MemberName::Ident(b.id("m"), sp(2, 3)).span(), sp(2, 3));
    assert_eq!(ConstSel::Class(sp(0, 5)).span(), sp(0, 5));
    assert!(Modifiers::default().is_empty());
    assert!(Modifiers::default().span.is_empty());
}

#[test]
fn node_sizes_stay_reasonable() {
    let expr = size_of::<Expr>();
    let stmt = size_of::<Stmt>();
    eprintln!(
        "size_of: Expr={expr} Stmt={stmt} Name={} Type={} Param={} Member={} Arg={} ArrayItem={}",
        size_of::<Name>(),
        size_of::<Type>(),
        size_of::<Param>(),
        size_of::<Member>(),
        size_of::<Arg>(),
        size_of::<ArrayItem>(),
    );
    assert!(
        expr <= 112,
        "Expr grew to {expr} bytes; box the new payload"
    );
    assert!(
        stmt <= 224,
        "Stmt grew to {stmt} bytes; box the new payload"
    );
}
