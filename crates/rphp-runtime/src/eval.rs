//! `eval()` (plan E7).
//!
//! `eval` is `include` without a file: the string is compiled as a unit and
//! run as an [`FrameKind::Include`] frame that **shares the caller's symbol
//! table**, so the code sees and updates the caller's variables, `$this` and
//! scope. It returns what the code `return`s, or `null` when it falls off the
//! end (unlike `include`, which yields `1`).
//!
//! The code is compiled with an implicit `<?php` already open, and php names
//! the unit `<caller file>(<line>) : eval()'d code` — that name appears in
//! `__FILE__`, in error messages and in a `ParseError`'s `getFile()`, so it is
//! built here rather than left to the compile hook.

use rphp_bytecode::Op;
use rphp_value::Value;

use crate::frame::{FrameKind, RetTarget};
use crate::registry::Unwind;
use crate::Interp;

impl Interp {
    /// `eval($code)`: compile `code` and push a frame for it. `Ok(true)` means
    /// a frame was pushed and the dispatch loop must continue into it.
    pub(crate) fn eval_code(&mut self, code: &[u8], dst_abs: usize) -> Result<bool, Unwind> {
        // php reports the *call site* of `eval`, not the file it lives in.
        let fi = self.frames.len() - 1;
        let (file, line) = {
            let f = &self.frames[fi];
            (self.frame_file(f), self.frame_line(f))
        };
        let name = format!("{file}({line}) : eval()'d code");

        // The compile hook expects a whole unit, so open php mode for it.
        let mut source = Vec::with_capacity(code.len() + 6);
        source.extend_from_slice(b"<?php ");
        source.extend_from_slice(code);

        let Some(hook) = self.compile_hook.take() else {
            return Err(Unwind::error("eval is not available: no compile hook installed"));
        };
        let compiled = hook(self, &source, &name);
        self.compile_hook = Some(hook);
        let module = match compiled {
            Ok(m) => m,
            Err(crate::interp::CompileFailure::Parse { message, line }) => {
                // php throws a `ParseError` naming the eval'd unit.
                return Err(self.parse_error(&message, &name, line));
            }
            Err(crate::interp::CompileFailure::Rejected(lines)) => {
                return Err(Unwind::error(format!(
                    "{name}: the engine cannot lower this code yet:\n{}",
                    lines.join("\n")
                )));
            }
        };
        // A unit's `{main}` ends with the compiler's synthesized `return 1`
        // (what `include` yields). `eval` yields *null* when the code falls
        // off the end, so retarget that one op; an explicit `return` inside
        // the code returns earlier and is unaffected.
        let mut module = module;
        let main_id = module.main;
        if let Some(Op::Ret { src }) = module.funcs[main_id as usize].code.last_mut() {
            *src = None;
        }
        let main = self.load_unit(module)?;
        let func = self.funcs[main as usize].clone();
        let (this, scope, static_class, symtab) = {
            let f = &self.frames[fi];
            (f.this.clone(), f.scope, f.static_class, f.symtab.clone())
        };
        // Sharing the caller's symbol table is what makes `eval` see and
        // update the caller's variables.
        let symtab = symtab.unwrap_or_else(|| self.globals.clone());
        self.push_user_frame(
            func,
            FrameKind::Include,
            &[],
            this,
            scope,
            static_class,
            RetTarget::Reg(dst_abs),
            Some(symtab),
        )?;
        Ok(true)
    }
}
