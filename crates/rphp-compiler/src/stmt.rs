//! Statement lowering.
//!
//! The lowered slice: `echo`, expression statements, `if`/`elseif`/`else`
//! (elseifs nest as `else { if ... }`), `while`, `foreach` over plain
//! variables, `return`, blocks, empty statements, inline HTML, and the global
//! `namespace { }` wrapper. Function and class declarations emit no code in a
//! frame (the driver compiles them as their own units); every other statement
//! is reported as `RPHP_E0300`.

use rphp_ast::v2::{Expr, Stmt};
use rphp_bytecode::{Const, Op};
use rphp_value::Str;

use crate::func::FnCompiler;
use crate::unsupported;

impl FnCompiler<'_> {
    pub(crate) fn compile_stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.compile_stmt(s);
        }
    }

    /// Compile the body of a compound statement. Declarations inside it are
    /// conditional (PHP only hoists from the top-level statement list, plain
    /// blocks and global namespace bodies), which the slice does not lower.
    fn compile_nested(&mut self, stmts: &[Stmt]) {
        let saved = self.at_top_level;
        self.at_top_level = false;
        self.compile_stmts(stmts);
        self.at_top_level = saved;
    }

    pub(crate) fn compile_stmt(&mut self, s: &Stmt) {
        self.mark_line(s.span());
        match s {
            Stmt::Echo { args, .. } => {
                for a in args {
                    let mark = self.temp_top;
                    let r = self.compile_expr(a);
                    self.emit(Op::Echo { src: r });
                    self.free_to(mark);
                }
            }
            Stmt::InlineHtml { text, .. } => {
                // Text outside the PHP tags is echoed verbatim.
                let k = self.push_const(Const::Str(Str::new(self.interner().resolve(*text))));
                let mark = self.temp_top;
                let dst = self.alloc_temp();
                self.emit(Op::LoadConst { dst, k });
                self.emit(Op::Echo { src: dst });
                self.free_to(mark);
            }
            Stmt::Expr { expr, .. } => {
                let mark = self.temp_top;
                self.compile_expr(expr);
                self.free_to(mark);
            }
            Stmt::If {
                cond,
                then,
                elseifs,
                else_,
                ..
            } => self.compile_if(cond, then, elseifs, else_.as_deref()),
            Stmt::While { cond, body, .. } => {
                let ltop = self.here();
                let mark = self.temp_top;
                let rc = self.compile_expr(cond);
                let jf = self.emit(Op::JmpIfFalse {
                    cond: rc,
                    target: 0,
                });
                self.free_to(mark);
                self.compile_nested(body);
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                self.patch(jf, lend);
            }
            Stmt::Foreach {
                subject,
                key,
                value,
                by_ref,
                body,
                span,
            } => {
                if *by_ref {
                    unsupported(self.diags, *span, "foreach by reference");
                }
                let Expr::Var(value_var, _) = &**value else {
                    unsupported(
                        self.diags,
                        value.span(),
                        "foreach non-variable value target",
                    );
                    return;
                };
                let key_var = match key.as_deref() {
                    None => None,
                    Some(Expr::Var(k, _)) => Some(*k),
                    Some(other) => {
                        unsupported(self.diags, other.span(), "foreach non-variable key target");
                        return;
                    }
                };
                let mark = self.temp_top;
                // Snapshot the subject into a temp we own — a separate COW handle
                // — so the body mutating the original array cannot disturb the
                // iteration (PHP foreach iterates over a copy).
                let sr = self.compile_expr(subject);
                let arr = self.alloc_temp();
                self.emit(Op::Move { dst: arr, src: sr });
                // The iteration cursor (a position into the entry list).
                let cursor = self.alloc_temp();
                let k0 = self.push_const(Const::Int(0));
                self.emit(Op::LoadConst { dst: cursor, k: k0 });
                let val_dst = self.var_reg(*value_var);
                let key_dst = match key_var {
                    Some(k) => self.var_reg(k),
                    None => self.alloc_temp(), // throwaway key sink
                };
                let ltop = self.here();
                let next = self.emit(Op::ForeachNext {
                    arr,
                    cursor,
                    key_dst,
                    val_dst,
                    target: 0,
                });
                self.compile_nested(body);
                self.emit(Op::Jmp { target: ltop });
                let lend = self.here();
                self.patch(next, lend);
                self.free_to(mark);
            }
            Stmt::Return { value, .. } => match value {
                Some(e) => {
                    let mark = self.temp_top;
                    let r = self.compile_expr(e);
                    self.emit(Op::Ret { src: Some(r) });
                    self.free_to(mark);
                }
                None => {
                    self.emit(Op::Ret { src: None });
                }
            },
            Stmt::Block { body, .. } => self.compile_stmts(body),
            Stmt::Nop { .. } => {}
            // Function and class declarations are compiled as their own
            // `Function`s/`Class`es by the driver; they emit no code in a frame.
            // The driver only hoists top-level declarations, so any that reach
            // a nested body are conditional declarations — not lowered yet.
            Stmt::Func(f) => {
                if !self.at_top_level {
                    unsupported(self.diags, f.span, "nested function declaration");
                }
            }
            Stmt::ClassLike(c) => {
                if !self.at_top_level {
                    unsupported(self.diags, c.span, "nested class declaration");
                }
            }
            Stmt::Namespace {
                name: None, body, ..
            } => {
                // The global `namespace { }` wrapper; its declarations were
                // hoisted by the driver.
                self.compile_stmts(body);
            }
            Stmt::Namespace { span, .. } => {
                unsupported(self.diags, *span, "namespace declaration");
            }
            Stmt::Use { span, .. } => unsupported(self.diags, *span, "use import"),
            Stmt::ConstDecl { span, .. } => unsupported(self.diags, *span, "const declaration"),
            Stmt::DoWhile { span, .. } => unsupported(self.diags, *span, "do-while loop"),
            Stmt::For { span, .. } => unsupported(self.diags, *span, "for loop"),
            Stmt::Switch { span, .. } => unsupported(self.diags, *span, "switch statement"),
            Stmt::Break { span, .. } => unsupported(self.diags, *span, "break"),
            Stmt::Continue { span, .. } => unsupported(self.diags, *span, "continue"),
            Stmt::Try { span, .. } => unsupported(self.diags, *span, "try/catch/finally"),
            Stmt::Goto { span, .. } => unsupported(self.diags, *span, "goto"),
            Stmt::Label { span, .. } => unsupported(self.diags, *span, "label"),
            Stmt::Global { span, .. } => unsupported(self.diags, *span, "global statement"),
            Stmt::StaticVar { span, .. } => unsupported(self.diags, *span, "static variable"),
            Stmt::Unset { span, .. } => unsupported(self.diags, *span, "unset"),
            Stmt::Declare { span, .. } => unsupported(self.diags, *span, "declare"),
            Stmt::HaltCompiler { span } => unsupported(self.diags, *span, "__halt_compiler"),
        }
    }

    /// `if (cond) then [elseif (c) body]* [else body]`. Each `elseif` is
    /// lowered as an `if` nested in the previous branch's `else`.
    fn compile_if(
        &mut self,
        cond: &Expr,
        then: &[Stmt],
        elseifs: &[rphp_ast::v2::ElseIf],
        else_: Option<&[Stmt]>,
    ) {
        let mark = self.temp_top;
        let rc = self.compile_expr(cond);
        let jf = self.emit(Op::JmpIfFalse {
            cond: rc,
            target: 0,
        });
        self.free_to(mark);
        self.compile_nested(then);
        let has_else = !elseifs.is_empty() || else_.is_some();
        if !has_else {
            let lend = self.here();
            self.patch(jf, lend);
            return;
        }
        let jend = self.emit(Op::Jmp { target: 0 });
        let lelse = self.here();
        self.patch(jf, lelse);
        match elseifs.split_first() {
            Some((first, rest)) => {
                self.mark_line(first.span);
                self.compile_if(&first.cond, &first.body, rest, else_);
            }
            None => self.compile_nested(else_.unwrap_or(&[])),
        }
        let lend = self.here();
        self.patch(jend, lend);
    }
}
