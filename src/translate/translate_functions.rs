//! Translate functions from the rust compiler MIR to our internal representation.
//! Our internal representation is very close to MIR, but is more convenient for
//! us to handle, and easier to maintain - rustc's representation can evolve
//! independently.

extern crate rustc_middle;
extern crate rustc_public;

use super::translate_ctx::*;
use charon_lib::ast::*;
use charon_lib::{raise_error, register_error};
use rustc_public::{CrateDef, mir, rustc_internal, ty};

impl ItemTransCtx<'_, '_> {
    pub fn requires_caller_location(&self, instance: mir::mono::Instance) -> bool {
        let instance_internal = rustc_internal::internal(self.t_ctx.tcx, instance);
        instance_internal
            .def
            .requires_caller_location(self.t_ctx.tcx)
    }

    /// If this instance is a closure, meaning in MIR it has untupled arguments both in the
    /// signature and the body.
    pub fn instance_is_closure(&self, def: mir::mono::Instance) -> bool {
        let def = rustc_internal::internal(self.t_ctx.tcx, def.def.def_id());
        self.t_ctx.tcx.is_closure_like(def)
    }

    fn get_function_ins_outs_sure_function(
        &mut self,
        def: mir::mono::Instance,
    ) -> Result<(Vec<ty::Ty>, ty::Ty), Error> {
        // Translate the signature

        // Rust may add an implicit call location argument to the function, to improve error
        // tracking; this argument however doesn't exist when calling the function or for the
        // body's locals; we must remove it manually.
        // https://rustc-dev-guide.rust-lang.org/backend/implicit-caller-location.html
        let instance_abi = def.fn_abi()?;
        let arg_count = if self.requires_caller_location(def) {
            instance_abi.args.len() - 1
        } else {
            instance_abi.args.len()
        };

        let inputs: Vec<_> = instance_abi.args[0..arg_count]
            .iter()
            .map(|arg| arg.ty)
            .collect();

        Ok((inputs, instance_abi.ret.ty))
    }

    pub(crate) fn get_function_ins_outs(
        &mut self,
        span: Span,
        def: mir::mono::Instance,
    ) -> Result<(Vec<ty::Ty>, ty::Ty), Error> {
        let Ok(crate_item) = rustc_public::CrateItem::try_from(def) else {
            // worth a shot
            return self.get_function_ins_outs_sure_function(def);
        };
        match crate_item.kind() {
            rustc_public::ItemKind::Fn | rustc_public::ItemKind::Ctor(_) => {
                self.get_function_ins_outs_sure_function(def)
            }
            rustc_public::ItemKind::Static => {
                let stt: mir::mono::StaticDef = crate_item.try_into()?;
                Ok((vec![], stt.ty()))
            }
            kind => {
                raise_error!(
                    self,
                    span,
                    "Unexpected item kind in translate signature: {kind:?}"
                )
            }
        }
    }

    /// Translate a function's signature
    pub(crate) fn translate_function_signature(
        &mut self,
        def: mir::mono::Instance,
        span: Span,
    ) -> Result<FunSig, Error> {
        let (inputs, outputs) = self.get_function_ins_outs(span, def)?;

        let inputs = inputs
            .into_iter()
            .map(|ty| self.translate_ty(span, ty))
            .try_collect()?;
        let output = self.translate_ty(span, outputs)?;

        Ok(FunSig {
            is_unsafe: false,
            inputs,
            output,
        })
    }
}
