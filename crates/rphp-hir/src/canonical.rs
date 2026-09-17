//! The canonical-subset invariant (ADR-026), checked by [`check`] and
//! asserted by [`crate::lower`] in debug builds.
//!
//! A canonical tree has:
//!
//! * no `If` with `elseif` clauses;
//! * no `??=` (`Assign.op == Some(Coalesce)`), no short ternary
//!   (`Ternary.then == None`), no `?->` link (`nullsafe == true`), no `|>`,
//!   no destructuring pattern as an assignment or `foreach` target, no
//!   `list()` syntax anywhere, no `isset` with more than one operand;
//! * no `get => e` / `set => e` hook body, and every `set` hook carries an
//!   explicit parameter list;
//! * `readonly` on every property member and promoted parameter of a
//!   `readonly class`;
//! * `resolved` filled on every class, function and constant reference
//!   (`use` items and `namespace` names are declarations, not references);
//! * no magic constant except the dynamic ones (`__CLASS__` in traits and
//!   anonymous classes, `__METHOD__`/`__FUNCTION__` where they embed an
//!   anonymous class name);
//! * no `Name::class` (folded to a string);
//! * no `Expr::Error`.

use rphp_ast::v2::visit::{walk_expr, walk_stmt, Visitor};
use rphp_ast::v2::{
    ArraySyntax, Attr, Callee, ClassLike, ClassRef, ConstSel, Expr, Hook, HookBody, HookKind,
    MagicKind, Member, Name, Param, Program, Stmt, Type, TypeKind,
};
use rphp_span::Span;

/// The [`Violation::what`] text for a parser recovery node.
pub const PARSE_ERROR_PLACEHOLDER: &str = "parse-error placeholder";

/// One place where the tree leaves the canonical subset.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Violation {
    /// What was found.
    pub what: String,
    /// Where.
    pub span: Span,
}

/// Collect every violation of the canonical-subset invariant in `program`.
pub fn check(program: &Program) -> Vec<Violation> {
    let mut c = Checker {
        out: Vec::new(),
        readonly_class: false,
    };
    c.visit_block(&program.items);
    c.out
}

struct Checker {
    out: Vec<Violation>,
    readonly_class: bool,
}

impl Checker {
    fn hit(&mut self, what: &str, span: Span) {
        self.out.push(Violation {
            what: what.to_owned(),
            span,
        });
    }

    fn check_resolved(&mut self, n: &Name, what: &str) {
        if n.resolved.is_none() {
            self.hit(what, n.span);
        }
    }

    fn check_params(&mut self, params: &[Param], in_readonly_class: bool) {
        for p in params {
            if in_readonly_class {
                if let Some(m) = &p.promote {
                    if !m.readonly {
                        self.hit(
                            "promoted parameter of a readonly class without readonly",
                            p.span,
                        );
                    }
                }
            }
        }
    }
}

impl Visitor for Checker {
    fn visit_stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::If { elseifs, .. } if !elseifs.is_empty() => self.hit("elseif clause", s.span()),
            Stmt::Foreach { value, .. } if matches!(**value, Expr::Array { .. }) => {
                self.hit("foreach destructuring target", value.span());
            }
            Stmt::Try {
                body,
                catches,
                finally,
                ..
            } => {
                self.visit_block(body);
                for c in catches {
                    for t in &c.types {
                        self.check_resolved(t, "unresolved catch type");
                    }
                    self.visit_block(&c.body);
                }
                if let Some(f) = finally {
                    self.visit_block(f);
                }
                return;
            }
            Stmt::Func(f) => {
                self.check_params(&f.params, false);
            }
            // Declarations, not references: their names stay unresolved.
            Stmt::Use { .. } => return,
            Stmt::Namespace { body, .. } => {
                self.visit_block(body);
                return;
            }
            _ => {}
        }
        walk_stmt(self, s);
    }

    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Error(s) => self.hit(PARSE_ERROR_PLACEHOLDER, *s),
            Expr::Assign {
                op: Some(rphp_ast::v2::BinOp::Coalesce),
                span,
                ..
            } => self.hit("??= assignment", *span),
            Expr::Assign { target, span, .. } if matches!(**target, Expr::Array { .. }) => {
                self.hit("destructuring assignment", *span);
            }
            Expr::Ternary {
                then: None, span, ..
            } => self.hit("short ternary", *span),
            Expr::Prop {
                nullsafe: true,
                span,
                ..
            }
            | Expr::MethodCall {
                nullsafe: true,
                span,
                ..
            } => self.hit("nullsafe link", *span),
            Expr::Binary {
                op: rphp_ast::v2::BinOp::Pipe,
                span,
                ..
            } => self.hit("pipe operator", *span),
            Expr::Isset { vars, span } if vars.len() != 1 => {
                self.hit("isset with several operands", *span);
            }
            Expr::Array {
                syntax: ArraySyntax::List,
                span,
                ..
            } => self.hit("list() syntax", *span),
            Expr::Const(n) => {
                self.check_resolved(n, "unresolved constant");
                return;
            }
            Expr::MagicConst { kind, span } => match kind {
                MagicKind::Class | MagicKind::Method | MagicKind::Function => {}
                _ => self.hit("unsubstituted magic constant", *span),
            },
            Expr::ClassConst {
                class: ClassRef::Named(_),
                name: ConstSel::Class(_),
                span,
            } => {
                self.hit("unfolded Name::class", *span);
                return;
            }
            _ => {}
        }
        walk_expr(self, e);
    }

    fn visit_callee(&mut self, c: &Callee) {
        match c {
            Callee::Name(n) => self.check_resolved(n, "unresolved function name"),
            Callee::Expr(e) => self.visit_expr(e),
        }
    }

    fn visit_class_ref(&mut self, c: &ClassRef) {
        match c {
            ClassRef::Named(n) => self.check_resolved(n, "unresolved class name"),
            ClassRef::Expr(e) => self.visit_expr(e),
            _ => {}
        }
    }

    fn visit_name(&mut self, n: &Name) {
        self.check_resolved(n, "unresolved name");
    }

    fn visit_type(&mut self, t: &Type) {
        match &t.kind {
            TypeKind::Named(n) => self.check_resolved(n, "unresolved type name"),
            TypeKind::Builtin(_) => {}
            TypeKind::Nullable(inner) => self.visit_type(inner),
            TypeKind::Union(ts) | TypeKind::Intersection(ts) => {
                for t in ts {
                    self.visit_type(t);
                }
            }
        }
    }

    fn visit_attr(&mut self, a: &Attr) {
        self.check_resolved(&a.name, "unresolved attribute name");
        for arg in &a.args {
            self.visit_arg(arg);
        }
    }

    fn visit_class_like(&mut self, c: &ClassLike) {
        let saved = self.readonly_class;
        self.readonly_class = c.modifiers.readonly;
        for g in &c.attrs {
            self.visit_attr_group(g);
        }
        for n in c.extends.iter().chain(&c.implements) {
            self.check_resolved(n, "unresolved class name");
        }
        if let Some(b) = &c.backing {
            self.visit_type(b);
        }
        for m in &c.members {
            if let Member::Prop(p) = m {
                if self.readonly_class && !p.modifiers.readonly {
                    self.hit("property of a readonly class without readonly", p.span);
                }
            }
            if let Member::Method(md) = m {
                self.check_params(&md.params, self.readonly_class);
            }
            self.visit_member(m);
        }
        self.readonly_class = saved;
    }

    fn visit_hook(&mut self, h: &Hook) {
        if matches!(h.body, HookBody::Expr(_)) {
            self.hit("hook expression body", h.span);
        }
        if h.kind == HookKind::Set && h.params.is_none() {
            self.hit("set hook without parameter list", h.span);
        }
        rphp_ast::v2::visit::walk_hook(self, h);
    }
}
