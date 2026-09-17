//! Tree traversal.
//!
//! [`Visitor`] walks an immutable tree; [`VisitorMut`] rewrites one in place
//! (the resolver fills [`Name::resolved`](super::name::Name::resolved), the
//! desugarer replaces nodes). Both traits have a `visit_*` method per node
//! family whose default calls the matching free `walk_*` function, which
//! visits the children in source order. Override `visit_*`, call `walk_*`
//! from the override to continue below.
//!
//! The immutable walkers live at `visit::walk_*`; the mutable ones at
//! `visit::mutable::walk_*`. Both are generated from one traversal
//! definition, so they never drift apart.
//!
//! Leaf families without children (`Name`, `Modifiers`, `ClosureUse`) have a
//! `visit_*` hook but no `walk_*`.

macro_rules! define_visitor {
    ($(#[$doc:meta])* $Trait:ident $(, $mut:ident)?) => {
        $(#[$doc])*
        pub trait $Trait {
            /// Visit a whole program.
            fn visit_program(&mut self, p: & $($mut)? Program) { walk_program(self, p) }
            /// Visit a statement list (a block body).
            fn visit_block(&mut self, stmts: & $($mut)? [Stmt]) { walk_block(self, stmts) }
            /// Visit a statement.
            fn visit_stmt(&mut self, s: & $($mut)? Stmt) { walk_stmt(self, s) }
            /// Visit an expression.
            fn visit_expr(&mut self, e: & $($mut)? Expr) { walk_expr(self, e) }
            /// Visit a written name (leaf).
            fn visit_name(&mut self, _n: & $($mut)? Name) {}
            /// Visit a type.
            fn visit_type(&mut self, t: & $($mut)? Type) { walk_type(self, t) }
            /// Visit a class reference.
            fn visit_class_ref(&mut self, c: & $($mut)? ClassRef) { walk_class_ref(self, c) }
            /// Visit a member name.
            fn visit_member_name(&mut self, m: & $($mut)? MemberName) { walk_member_name(self, m) }
            /// Visit a class-constant selector.
            fn visit_const_sel(&mut self, c: & $($mut)? ConstSel) { walk_const_sel(self, c) }
            /// Visit a callee.
            fn visit_callee(&mut self, c: & $($mut)? Callee) { walk_callee(self, c) }
            /// Visit a first-class-callable target.
            fn visit_callable_target(&mut self, t: & $($mut)? CallableTarget) { walk_callable_target(self, t) }
            /// Visit a `new` target.
            fn visit_new_target(&mut self, t: & $($mut)? NewTarget) { walk_new_target(self, t) }
            /// Visit a call argument.
            fn visit_arg(&mut self, a: & $($mut)? Arg) { walk_arg(self, a) }
            /// Visit an array element.
            fn visit_array_item(&mut self, i: & $($mut)? ArrayItem) { walk_array_item(self, i) }
            /// Visit an interpolation part.
            fn visit_interp_part(&mut self, p: & $($mut)? InterpPart) { walk_interp_part(self, p) }
            /// Visit a `match` arm.
            fn visit_match_arm(&mut self, a: & $($mut)? MatchArm) { walk_match_arm(self, a) }
            /// Visit a closure `use` capture (leaf).
            fn visit_closure_use(&mut self, _u: & $($mut)? ClosureUse) {}
            /// Visit a closure.
            fn visit_closure(&mut self, c: & $($mut)? Closure) { walk_closure(self, c) }
            /// Visit an arrow function.
            fn visit_arrow_fn(&mut self, f: & $($mut)? ArrowFn) { walk_arrow_fn(self, f) }
            /// Visit an `elseif` clause.
            fn visit_else_if(&mut self, e: & $($mut)? ElseIf) { walk_else_if(self, e) }
            /// Visit a `switch` case.
            fn visit_case(&mut self, c: & $($mut)? Case) { walk_case(self, c) }
            /// Visit a `catch` clause.
            fn visit_catch(&mut self, c: & $($mut)? Catch) { walk_catch(self, c) }
            /// Visit a `static $x` item.
            fn visit_static_var(&mut self, s: & $($mut)? StaticVar) { walk_static_var(self, s) }
            /// Visit a `declare` directive.
            fn visit_directive(&mut self, d: & $($mut)? Directive) { walk_directive(self, d) }
            /// Visit a `use` item.
            fn visit_use_item(&mut self, u: & $($mut)? UseItem) { walk_use_item(self, u) }
            /// Visit a `const` item.
            fn visit_const_item(&mut self, c: & $($mut)? ConstItem) { walk_const_item(self, c) }
            /// Visit a function declaration.
            fn visit_func_decl(&mut self, f: & $($mut)? FuncDecl) { walk_func_decl(self, f) }
            /// Visit a class-like declaration.
            fn visit_class_like(&mut self, c: & $($mut)? ClassLike) { walk_class_like(self, c) }
            /// Visit a class member.
            fn visit_member(&mut self, m: & $($mut)? Member) { walk_member(self, m) }
            /// Visit a class-constant member.
            fn visit_const_member(&mut self, c: & $($mut)? ConstMember) { walk_const_member(self, c) }
            /// Visit a property member.
            fn visit_prop_member(&mut self, p: & $($mut)? PropMember) { walk_prop_member(self, p) }
            /// Visit a property item.
            fn visit_prop_item(&mut self, p: & $($mut)? PropItem) { walk_prop_item(self, p) }
            /// Visit a method.
            fn visit_method_decl(&mut self, m: & $($mut)? MethodDecl) { walk_method_decl(self, m) }
            /// Visit an enum case.
            fn visit_enum_case(&mut self, c: & $($mut)? EnumCase) { walk_enum_case(self, c) }
            /// Visit a trait use.
            fn visit_trait_use(&mut self, t: & $($mut)? TraitUse) { walk_trait_use(self, t) }
            /// Visit a trait adaptation.
            fn visit_adaptation(&mut self, a: & $($mut)? Adaptation) { walk_adaptation(self, a) }
            /// Visit a parameter.
            fn visit_param(&mut self, p: & $($mut)? Param) { walk_param(self, p) }
            /// Visit a property hook.
            fn visit_hook(&mut self, h: & $($mut)? Hook) { walk_hook(self, h) }
            /// Visit a hook body.
            fn visit_hook_body(&mut self, b: & $($mut)? HookBody) { walk_hook_body(self, b) }
            /// Visit an attribute group.
            fn visit_attr_group(&mut self, g: & $($mut)? AttrGroup) { walk_attr_group(self, g) }
            /// Visit an attribute.
            fn visit_attr(&mut self, a: & $($mut)? Attr) { walk_attr(self, a) }
            /// Visit a modifier set (leaf).
            fn visit_modifiers(&mut self, _m: & $($mut)? Modifiers) {}
        }

        /// Walk the statements of a program.
        pub fn walk_program<V: $Trait + ?Sized>(v: &mut V, p: & $($mut)? Program) {
            v.visit_block(& $($mut)? p.items);
        }

        /// Walk each statement of a block.
        pub fn walk_block<V: $Trait + ?Sized>(v: &mut V, stmts: & $($mut)? [Stmt]) {
            for s in stmts {
                v.visit_stmt(s);
            }
        }

        /// Walk the children of a statement.
        pub fn walk_stmt<V: $Trait + ?Sized>(v: &mut V, s: & $($mut)? Stmt) {
            match s {
                Stmt::InlineHtml { .. }
                | Stmt::Break { .. }
                | Stmt::Continue { .. }
                | Stmt::Goto { .. }
                | Stmt::Label { .. }
                | Stmt::HaltCompiler { .. }
                | Stmt::Nop { .. } => {}
                Stmt::Expr { expr, .. } => v.visit_expr(expr),
                Stmt::Echo { args, .. } => {
                    for a in args {
                        v.visit_expr(a);
                    }
                }
                Stmt::Block { body, .. } => v.visit_block(body),
                Stmt::If { cond, then, elseifs, else_, .. } => {
                    v.visit_expr(cond);
                    v.visit_block(then);
                    for e in elseifs {
                        v.visit_else_if(e);
                    }
                    if let Some(e) = else_ {
                        v.visit_block(e);
                    }
                }
                Stmt::While { cond, body, .. } => {
                    v.visit_expr(cond);
                    v.visit_block(body);
                }
                Stmt::DoWhile { body, cond, .. } => {
                    v.visit_block(body);
                    v.visit_expr(cond);
                }
                Stmt::For { init, cond, step, body, .. } => {
                    for e in init {
                        v.visit_expr(e);
                    }
                    for e in cond {
                        v.visit_expr(e);
                    }
                    for e in step {
                        v.visit_expr(e);
                    }
                    v.visit_block(body);
                }
                Stmt::Foreach { subject, key, value, body, .. } => {
                    v.visit_expr(subject);
                    if let Some(k) = key {
                        v.visit_expr(k);
                    }
                    v.visit_expr(value);
                    v.visit_block(body);
                }
                Stmt::Switch { subject, cases, .. } => {
                    v.visit_expr(subject);
                    for c in cases {
                        v.visit_case(c);
                    }
                }
                Stmt::Return { value, .. } => {
                    if let Some(e) = value {
                        v.visit_expr(e);
                    }
                }
                Stmt::Try { body, catches, finally, .. } => {
                    v.visit_block(body);
                    for c in catches {
                        v.visit_catch(c);
                    }
                    if let Some(f) = finally {
                        v.visit_block(f);
                    }
                }
                Stmt::Global { vars, .. } => {
                    for e in vars {
                        v.visit_expr(e);
                    }
                }
                Stmt::StaticVar { vars, .. } => {
                    for s in vars {
                        v.visit_static_var(s);
                    }
                }
                Stmt::Unset { targets, .. } => {
                    for e in targets {
                        v.visit_expr(e);
                    }
                }
                Stmt::Declare { directives, body, .. } => {
                    for d in directives {
                        v.visit_directive(d);
                    }
                    if let Some(b) = body {
                        v.visit_block(b);
                    }
                }
                Stmt::Namespace { name, body, .. } => {
                    if let Some(n) = name {
                        v.visit_name(n);
                    }
                    v.visit_block(body);
                }
                Stmt::Use { prefix, items, .. } => {
                    if let Some(p) = prefix {
                        v.visit_name(p);
                    }
                    for it in items {
                        v.visit_use_item(it);
                    }
                }
                Stmt::ConstDecl { attrs, items, .. } => {
                    for g in attrs {
                        v.visit_attr_group(g);
                    }
                    for it in items {
                        v.visit_const_item(it);
                    }
                }
                Stmt::Func(f) => v.visit_func_decl(f),
                Stmt::ClassLike(c) => v.visit_class_like(c),
            }
        }

        /// Walk the children of an expression.
        pub fn walk_expr<V: $Trait + ?Sized>(v: &mut V, e: & $($mut)? Expr) {
            match e {
                Expr::Null(_)
                | Expr::Bool(..)
                | Expr::Int(..)
                | Expr::Float(..)
                | Expr::Str(..)
                | Expr::Var(..)
                | Expr::MagicConst { .. }
                | Expr::Temp(..)
                | Expr::Error(_) => {}
                Expr::Interp { parts, .. } | Expr::ShellExec { parts, .. } => {
                    for p in parts {
                        v.visit_interp_part(p);
                    }
                }
                Expr::VarVar { name, .. } => v.visit_expr(name),
                Expr::Array { items, .. } => {
                    for it in items {
                        v.visit_array_item(it);
                    }
                }
                Expr::Index { base, index, .. } => {
                    v.visit_expr(base);
                    if let Some(i) = index {
                        v.visit_expr(i);
                    }
                }
                Expr::Prop { obj, name, .. } => {
                    v.visit_expr(obj);
                    v.visit_member_name(name);
                }
                Expr::StaticProp { class, name, .. } => {
                    v.visit_class_ref(class);
                    v.visit_member_name(name);
                }
                Expr::ClassConst { class, name, .. } => {
                    v.visit_class_ref(class);
                    v.visit_const_sel(name);
                }
                Expr::Const(n) => v.visit_name(n),
                Expr::Call { callee, args, .. } => {
                    v.visit_callee(callee);
                    for a in args {
                        v.visit_arg(a);
                    }
                }
                Expr::MethodCall { obj, name, args, .. } => {
                    v.visit_expr(obj);
                    v.visit_member_name(name);
                    for a in args {
                        v.visit_arg(a);
                    }
                }
                Expr::StaticCall { class, name, args, .. } => {
                    v.visit_class_ref(class);
                    v.visit_member_name(name);
                    for a in args {
                        v.visit_arg(a);
                    }
                }
                Expr::Callable { target, .. } => v.visit_callable_target(target),
                Expr::New { class, args, .. } => {
                    v.visit_new_target(class);
                    for a in args {
                        v.visit_arg(a);
                    }
                }
                Expr::Clone { expr, with, .. } => {
                    v.visit_expr(expr);
                    if let Some(w) = with {
                        v.visit_expr(w);
                    }
                }
                Expr::Unary { expr, .. } => v.visit_expr(expr),
                Expr::Binary { lhs, rhs, .. } => {
                    v.visit_expr(lhs);
                    v.visit_expr(rhs);
                }
                Expr::Assign { target, value, .. } => {
                    v.visit_expr(target);
                    v.visit_expr(value);
                }
                Expr::Ternary { cond, then, else_, .. } => {
                    v.visit_expr(cond);
                    if let Some(t) = then {
                        v.visit_expr(t);
                    }
                    v.visit_expr(else_);
                }
                Expr::Isset { vars, .. } => {
                    for e in vars {
                        v.visit_expr(e);
                    }
                }
                Expr::Empty { expr, .. }
                | Expr::Eval { code: expr, .. }
                | Expr::Print { expr, .. }
                | Expr::Throw { expr, .. }
                | Expr::YieldFrom { expr, .. }
                | Expr::Include { path: expr, .. } => v.visit_expr(expr),
                Expr::Exit { arg, .. } => {
                    if let Some(a) = arg {
                        v.visit_expr(a);
                    }
                }
                Expr::Closure(c) => v.visit_closure(c),
                Expr::ArrowFn(f) => v.visit_arrow_fn(f),
                Expr::Match { subject, arms, .. } => {
                    v.visit_expr(subject);
                    for a in arms {
                        v.visit_match_arm(a);
                    }
                }
                Expr::Yield { key, value, .. } => {
                    if let Some(k) = key {
                        v.visit_expr(k);
                    }
                    if let Some(val) = value {
                        v.visit_expr(val);
                    }
                }
                Expr::InstanceOf { expr, class, .. } => {
                    v.visit_expr(expr);
                    v.visit_class_ref(class);
                }
                Expr::Let { init, body, .. } => {
                    v.visit_expr(init);
                    v.visit_expr(body);
                }
                Expr::Seq(exprs, _) => {
                    for e in exprs {
                        v.visit_expr(e);
                    }
                }
            }
        }

        /// Walk the children of a type.
        pub fn walk_type<V: $Trait + ?Sized>(v: &mut V, t: & $($mut)? Type) {
            match & $($mut)? t.kind {
                TypeKind::Named(n) => v.visit_name(n),
                TypeKind::Builtin(_) => {}
                TypeKind::Nullable(inner) => v.visit_type(inner),
                TypeKind::Union(ts) | TypeKind::Intersection(ts) => {
                    for t in ts {
                        v.visit_type(t);
                    }
                }
            }
        }

        /// Walk the children of a class reference.
        pub fn walk_class_ref<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? ClassRef) {
            match c {
                ClassRef::Named(n) => v.visit_name(n),
                ClassRef::SelfKw(_) | ClassRef::Static(_) | ClassRef::Parent(_) => {}
                ClassRef::Expr(e) => v.visit_expr(e),
            }
        }

        /// Walk the children of a member name.
        pub fn walk_member_name<V: $Trait + ?Sized>(v: &mut V, m: & $($mut)? MemberName) {
            match m {
                MemberName::Ident(..) => {}
                MemberName::Expr(e) => v.visit_expr(e),
            }
        }

        /// Walk the children of a class-constant selector.
        pub fn walk_const_sel<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? ConstSel) {
            match c {
                ConstSel::Ident(..) | ConstSel::Class(_) => {}
                ConstSel::Expr(e) => v.visit_expr(e),
            }
        }

        /// Walk the children of a callee.
        pub fn walk_callee<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? Callee) {
            match c {
                Callee::Name(n) => v.visit_name(n),
                Callee::Expr(e) => v.visit_expr(e),
            }
        }

        /// Walk the children of a first-class-callable target.
        pub fn walk_callable_target<V: $Trait + ?Sized>(v: &mut V, t: & $($mut)? CallableTarget) {
            match t {
                CallableTarget::Func(c) => v.visit_callee(c),
                CallableTarget::Method { obj, name } => {
                    v.visit_expr(obj);
                    v.visit_member_name(name);
                }
                CallableTarget::Static { class, name } => {
                    v.visit_class_ref(class);
                    v.visit_member_name(name);
                }
            }
        }

        /// Walk the children of a `new` target.
        pub fn walk_new_target<V: $Trait + ?Sized>(v: &mut V, t: & $($mut)? NewTarget) {
            match t {
                NewTarget::Ref(c) => v.visit_class_ref(c),
                NewTarget::Anon(c) => v.visit_class_like(c),
            }
        }

        /// Walk the value of an argument.
        pub fn walk_arg<V: $Trait + ?Sized>(v: &mut V, a: & $($mut)? Arg) {
            v.visit_expr(& $($mut)? a.value);
        }

        /// Walk the key and value of an array element.
        pub fn walk_array_item<V: $Trait + ?Sized>(v: &mut V, i: & $($mut)? ArrayItem) {
            if let Some(k) = & $($mut)? i.key {
                v.visit_expr(k);
            }
            if let Some(val) = & $($mut)? i.value {
                v.visit_expr(val);
            }
        }

        /// Walk the expression of an interpolation part.
        pub fn walk_interp_part<V: $Trait + ?Sized>(v: &mut V, p: & $($mut)? InterpPart) {
            match p {
                InterpPart::Lit(..) => {}
                InterpPart::Expr(e) => v.visit_expr(e),
            }
        }

        /// Walk the conditions and body of a `match` arm.
        pub fn walk_match_arm<V: $Trait + ?Sized>(v: &mut V, a: & $($mut)? MatchArm) {
            if let Some(conds) = & $($mut)? a.conds {
                for c in conds {
                    v.visit_expr(c);
                }
            }
            v.visit_expr(& $($mut)? a.body);
        }

        /// Walk the children of a closure.
        pub fn walk_closure<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? Closure) {
            for g in & $($mut)? c.attrs {
                v.visit_attr_group(g);
            }
            for p in & $($mut)? c.params {
                v.visit_param(p);
            }
            for u in & $($mut)? c.uses {
                v.visit_closure_use(u);
            }
            if let Some(t) = & $($mut)? c.ret {
                v.visit_type(t);
            }
            v.visit_block(& $($mut)? c.body);
        }

        /// Walk the children of an arrow function.
        pub fn walk_arrow_fn<V: $Trait + ?Sized>(v: &mut V, f: & $($mut)? ArrowFn) {
            for g in & $($mut)? f.attrs {
                v.visit_attr_group(g);
            }
            for p in & $($mut)? f.params {
                v.visit_param(p);
            }
            if let Some(t) = & $($mut)? f.ret {
                v.visit_type(t);
            }
            v.visit_expr(& $($mut)? f.body);
        }

        /// Walk the condition and body of an `elseif`.
        pub fn walk_else_if<V: $Trait + ?Sized>(v: &mut V, e: & $($mut)? ElseIf) {
            v.visit_expr(& $($mut)? e.cond);
            v.visit_block(& $($mut)? e.body);
        }

        /// Walk the condition and body of a case.
        pub fn walk_case<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? Case) {
            if let Some(e) = & $($mut)? c.cond {
                v.visit_expr(e);
            }
            v.visit_block(& $($mut)? c.body);
        }

        /// Walk the types and body of a catch clause.
        pub fn walk_catch<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? Catch) {
            for t in & $($mut)? c.types {
                v.visit_name(t);
            }
            v.visit_block(& $($mut)? c.body);
        }

        /// Walk the initializer of a static variable.
        pub fn walk_static_var<V: $Trait + ?Sized>(v: &mut V, s: & $($mut)? StaticVar) {
            if let Some(e) = & $($mut)? s.init {
                v.visit_expr(e);
            }
        }

        /// Walk the value of a directive.
        pub fn walk_directive<V: $Trait + ?Sized>(v: &mut V, d: & $($mut)? Directive) {
            v.visit_expr(& $($mut)? d.value);
        }

        /// Walk the name of a `use` item.
        pub fn walk_use_item<V: $Trait + ?Sized>(v: &mut V, u: & $($mut)? UseItem) {
            v.visit_name(& $($mut)? u.name);
        }

        /// Walk the value of a `const` item.
        pub fn walk_const_item<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? ConstItem) {
            v.visit_expr(& $($mut)? c.value);
        }

        /// Walk the children of a function declaration.
        pub fn walk_func_decl<V: $Trait + ?Sized>(v: &mut V, f: & $($mut)? FuncDecl) {
            for g in & $($mut)? f.attrs {
                v.visit_attr_group(g);
            }
            for p in & $($mut)? f.params {
                v.visit_param(p);
            }
            if let Some(t) = & $($mut)? f.ret {
                v.visit_type(t);
            }
            v.visit_block(& $($mut)? f.body);
        }

        /// Walk the children of a class-like declaration.
        pub fn walk_class_like<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? ClassLike) {
            for g in & $($mut)? c.attrs {
                v.visit_attr_group(g);
            }
            v.visit_modifiers(& $($mut)? c.modifiers);
            for n in & $($mut)? c.extends {
                v.visit_name(n);
            }
            for n in & $($mut)? c.implements {
                v.visit_name(n);
            }
            if let Some(t) = & $($mut)? c.backing {
                v.visit_type(t);
            }
            for m in & $($mut)? c.members {
                v.visit_member(m);
            }
        }

        /// Dispatch on the member kind.
        pub fn walk_member<V: $Trait + ?Sized>(v: &mut V, m: & $($mut)? Member) {
            match m {
                Member::Const(c) => v.visit_const_member(c),
                Member::Prop(p) => v.visit_prop_member(p),
                Member::Method(m) => v.visit_method_decl(m),
                Member::EnumCase(c) => v.visit_enum_case(c),
                Member::TraitUse(t) => v.visit_trait_use(t),
            }
        }

        /// Walk the children of a class-constant member.
        pub fn walk_const_member<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? ConstMember) {
            for g in & $($mut)? c.attrs {
                v.visit_attr_group(g);
            }
            v.visit_modifiers(& $($mut)? c.modifiers);
            if let Some(t) = & $($mut)? c.ty {
                v.visit_type(t);
            }
            for it in & $($mut)? c.items {
                v.visit_const_item(it);
            }
        }

        /// Walk the children of a property member.
        pub fn walk_prop_member<V: $Trait + ?Sized>(v: &mut V, p: & $($mut)? PropMember) {
            for g in & $($mut)? p.attrs {
                v.visit_attr_group(g);
            }
            v.visit_modifiers(& $($mut)? p.modifiers);
            if let Some(t) = & $($mut)? p.ty {
                v.visit_type(t);
            }
            for it in & $($mut)? p.items {
                v.visit_prop_item(it);
            }
            for h in & $($mut)? p.hooks {
                v.visit_hook(h);
            }
        }

        /// Walk the default of a property item.
        pub fn walk_prop_item<V: $Trait + ?Sized>(v: &mut V, p: & $($mut)? PropItem) {
            if let Some(d) = & $($mut)? p.default {
                v.visit_expr(d);
            }
        }

        /// Walk the children of a method.
        pub fn walk_method_decl<V: $Trait + ?Sized>(v: &mut V, m: & $($mut)? MethodDecl) {
            for g in & $($mut)? m.attrs {
                v.visit_attr_group(g);
            }
            v.visit_modifiers(& $($mut)? m.modifiers);
            for p in & $($mut)? m.params {
                v.visit_param(p);
            }
            if let Some(t) = & $($mut)? m.ret {
                v.visit_type(t);
            }
            if let Some(b) = & $($mut)? m.body {
                v.visit_block(b);
            }
        }

        /// Walk the children of an enum case.
        pub fn walk_enum_case<V: $Trait + ?Sized>(v: &mut V, c: & $($mut)? EnumCase) {
            for g in & $($mut)? c.attrs {
                v.visit_attr_group(g);
            }
            if let Some(e) = & $($mut)? c.value {
                v.visit_expr(e);
            }
        }

        /// Walk the traits and adaptations of a trait use.
        pub fn walk_trait_use<V: $Trait + ?Sized>(v: &mut V, t: & $($mut)? TraitUse) {
            for n in & $($mut)? t.traits {
                v.visit_name(n);
            }
            for a in & $($mut)? t.adaptations {
                v.visit_adaptation(a);
            }
        }

        /// Walk the names of a trait adaptation.
        pub fn walk_adaptation<V: $Trait + ?Sized>(v: &mut V, a: & $($mut)? Adaptation) {
            match a {
                Adaptation::Precedence { trait_, insteadof, .. } => {
                    v.visit_name(trait_);
                    for n in insteadof {
                        v.visit_name(n);
                    }
                }
                Adaptation::Alias { trait_, .. } => {
                    if let Some(t) = trait_ {
                        v.visit_name(t);
                    }
                }
            }
        }

        /// Walk the children of a parameter.
        pub fn walk_param<V: $Trait + ?Sized>(v: &mut V, p: & $($mut)? Param) {
            for g in & $($mut)? p.attrs {
                v.visit_attr_group(g);
            }
            if let Some(t) = & $($mut)? p.ty {
                v.visit_type(t);
            }
            if let Some(d) = & $($mut)? p.default {
                v.visit_expr(d);
            }
            if let Some(m) = & $($mut)? p.promote {
                v.visit_modifiers(m);
            }
            for h in & $($mut)? p.hooks {
                v.visit_hook(h);
            }
        }

        /// Walk the children of a property hook.
        pub fn walk_hook<V: $Trait + ?Sized>(v: &mut V, h: & $($mut)? Hook) {
            for g in & $($mut)? h.attrs {
                v.visit_attr_group(g);
            }
            if let Some(params) = & $($mut)? h.params {
                for p in params {
                    v.visit_param(p);
                }
            }
            v.visit_hook_body(& $($mut)? h.body);
        }

        /// Walk the children of a hook body.
        pub fn walk_hook_body<V: $Trait + ?Sized>(v: &mut V, b: & $($mut)? HookBody) {
            match b {
                HookBody::Expr(e) => v.visit_expr(e),
                HookBody::Block(stmts) => v.visit_block(stmts),
                HookBody::Abstract => {}
            }
        }

        /// Walk the attributes of a group.
        pub fn walk_attr_group<V: $Trait + ?Sized>(v: &mut V, g: & $($mut)? AttrGroup) {
            for a in & $($mut)? g.attrs {
                v.visit_attr(a);
            }
        }

        /// Walk the name and arguments of an attribute.
        pub fn walk_attr<V: $Trait + ?Sized>(v: &mut V, a: & $($mut)? Attr) {
            v.visit_name(& $($mut)? a.name);
            for arg in & $($mut)? a.args {
                v.visit_arg(arg);
            }
        }
    };
}

mod imm {
    use super::super::attr::{Attr, AttrGroup};
    use super::super::decl::{
        Adaptation, ClassLike, ConstMember, EnumCase, FuncDecl, Hook, HookBody, Member, MethodDecl,
        Modifiers, Param, PropItem, PropMember, TraitUse,
    };
    use super::super::expr::{
        Arg, ArrayItem, ArrowFn, CallableTarget, Callee, Closure, ClosureUse, ConstSel, Expr,
        InterpPart, MatchArm, NewTarget,
    };
    use super::super::name::{ClassRef, MemberName, Name};
    use super::super::stmt::{
        Case, Catch, ConstItem, Directive, ElseIf, Program, StaticVar, Stmt, UseItem,
    };
    use super::super::types::{Type, TypeKind};

    define_visitor!(
        /// Immutable traversal. Every `visit_*` defaults to the matching
        /// `walk_*`, which visits the children in source order.
        Visitor
    );
}

pub use imm::*;

/// In-place traversal: the same traversal as [`Visitor`] over `&mut` nodes.
pub mod mutable {
    use super::super::attr::{Attr, AttrGroup};
    use super::super::decl::{
        Adaptation, ClassLike, ConstMember, EnumCase, FuncDecl, Hook, HookBody, Member, MethodDecl,
        Modifiers, Param, PropItem, PropMember, TraitUse,
    };
    use super::super::expr::{
        Arg, ArrayItem, ArrowFn, CallableTarget, Callee, Closure, ClosureUse, ConstSel, Expr,
        InterpPart, MatchArm, NewTarget,
    };
    use super::super::name::{ClassRef, MemberName, Name};
    use super::super::stmt::{
        Case, Catch, ConstItem, Directive, ElseIf, Program, StaticVar, Stmt, UseItem,
    };
    use super::super::types::{Type, TypeKind};

    define_visitor!(
        /// Mutable traversal for in-place rewriting (resolution, desugaring).
        /// Every `visit_*` defaults to the matching `walk_*` in this module.
        VisitorMut,
        mut
    );
}

pub use mutable::VisitorMut;
