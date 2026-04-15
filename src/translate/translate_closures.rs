extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_span;

use log::trace;
use rustc_public::{mir, ty};

use charon_lib::{ast::*, ids::IndexVec, raise_error, register_error, ullbc_ast::*};

use crate::translate::translate_ctx::ItemTransCtx;

impl ItemTransCtx<'_, '_> {
    /// Translate a closure as a struct
    pub(crate) fn translate_closure_as_adt_def(
        &mut self,
        trans_id: TypeDeclId,
        def_span: Span,
        item_meta: &ItemMeta,
        _closure: &ty::ClosureDef,
        args: &ty::GenericArgs,
    ) -> Result<TypeDeclKind, Error> {
        if item_meta.opacity.is_opaque() {
            return Ok(TypeDeclKind::Opaque);
        }

        trace!("{}", trans_id);

        // Closures have a fun time with generics:
        // https://doc.rust-lang.org/beta/nightly-rustc/src/rustc_type_ir/ty_kind/closure.rs.html#12-29
        let tupled_upvars = args.0.last().unwrap().expect_ty().kind();
        let Some(ty::RigidTy::Tuple(state_tys)) = tupled_upvars.rigid() else {
            raise_error!(self, def_span, "Closure state argument is not a tuple?");
        };

        // let upvars =
        let mut fields: IndexVec<FieldId, Field> = Default::default();
        for (j, state_ty) in state_tys.iter().enumerate() {
            // Translate the field type
            let ty = self.translate_ty(def_span, *state_ty)?;

            // Retrieve the field name.
            let field_name = format!("upvar_{j}");

            // Store the field
            let field = Field {
                span: def_span,
                attr_info: AttrInfo::default(),
                name: Some(field_name),
                ty,
            };
            fields.push(field);
        }

        // Register the type
        let type_def_kind = TypeDeclKind::Struct(fields);

        Ok(type_def_kind)
    }

    pub fn translate_closure_src_info(
        &mut self,
        _span: Span,
        _closure: &ty::ClosureDef,
        args: &ty::GenericArgs,
    ) -> Result<ItemSource, Error> {
        let signature = FunSig {
            is_unsafe: false,
            inputs: vec![],
            output: Ty::mk_unit(),
        };
        let args = rustc_public::rustc_internal::internal(self.t_ctx.tcx, args);
        let closure = args.as_closure();
        let kind = match closure.kind() {
            rustc_middle::ty::ClosureKind::FnOnce => ClosureKind::FnOnce,
            rustc_middle::ty::ClosureKind::FnMut => ClosureKind::FnMut,
            rustc_middle::ty::ClosureKind::Fn => ClosureKind::Fn,
        };

        Ok(ItemSource::Closure {
            info: ClosureInfo {
                kind,
                // HACK: put whatever ^-^'
                fn_once_impl: RegionBinder::empty(self.dummy_trait_impl_ref()),
                fn_mut_impl: None,
                fn_impl: None,
                signature: RegionBinder::empty(signature),
            },
        })
    }

    /// Given an item that is a non-capturing closure, generate the equivalent function,
    /// by removing the state from the parameters and untupling the arguments.
    pub fn translate_stateless_closure_as_fn(
        mut self,
        def_id: FunDeclId,
        item_meta: ItemMeta,
        closure: &ty::ClosureDef,
        args: &ty::GenericArgs,
    ) -> Result<FunDecl, Error> {
        let span = item_meta.span;

        // Closures have a fun time with generics:
        // https://doc.rust-lang.org/beta/nightly-rustc/src/rustc_type_ir/ty_kind/closure.rs.html#12-29
        let tupled_upvars = args.0.last().unwrap().expect_ty();
        let tupled_upvars = self.translate_ty(span, *tupled_upvars)?;

        assert!(
            tupled_upvars.as_tuple().is_some_and(IndexMap::is_empty),
            "Only stateless closures can be translated as functions"
        );

        // Translate the function signature
        let instance =
            mir::mono::Instance::resolve_closure(*closure, args, ty::ClosureKind::FnOnce)?;
        let mut signature = self.translate_function_signature(instance, span)?;

        let state_ty = signature.inputs.remove(0);
        let args_untupled = signature.inputs.clone();

        let body = if item_meta.opacity.with_private_contents().is_opaque() {
            Body::Opaque
        } else {
            // Target translation:
            //
            // fn call_fn(arg0: Args[0], ..., argN: Args[N]) -> Output {
            //   let closure: Closure = {};
            //   let args = (arg0, ..., argN);
            //   closure.call(args)
            // }
            let mk_stt = |content| Statement::new(span, content);
            let mk_block = |statements, terminator| -> BlockData {
                BlockData {
                    statements,
                    terminator: Terminator::new(span, terminator),
                }
            };
            let fun_id: FunDeclId = self.register_fun_decl_id(span, instance);
            let fn_op = FnOperand::Regular(FnPtr {
                kind: Box::new(fun_id.into()),
                generics: Box::new(GenericArgs::empty()),
            });

            let mut locals = Locals {
                arg_count: signature.inputs.len(),
                locals: IndexVec::new(),
            };
            let mut statements = vec![];

            let output = locals.new_var(None, signature.output.clone());
            let args: Vec<Place> = signature
                .inputs
                .iter()
                .enumerate()
                .map(|(i, ty)| locals.new_var(Some(format!("arg{}", i + 1)), ty.clone()))
                .collect();
            let args_tuple_ty = Ty::mk_tuple(args_untupled);
            let args_tupled = locals.new_var(Some("args".to_string()), args_tuple_ty.clone());
            let state = locals.new_var(Some("state".to_string()), state_ty.clone());

            statements.push(mk_stt(StatementKind::Assign(
                args_tupled.clone(),
                Rvalue::Aggregate(
                    AggregateKind::Adt(args_tuple_ty.as_adt().unwrap().clone(), None, None),
                    args.into_iter().map(Operand::Move).collect(),
                ),
            )));

            let state_ty_adt = state_ty.as_adt().unwrap();
            statements.push(mk_stt(StatementKind::Assign(
                state.clone(),
                Rvalue::Aggregate(AggregateKind::Adt(state_ty_adt.clone(), None, None), vec![]),
            )));

            let mut blocks = IndexMap::default();
            let start_block = blocks.reserve_slot();
            let ret_block = blocks.push(mk_block(vec![], TerminatorKind::Return));
            let unwind_block = blocks.push(mk_block(vec![], TerminatorKind::UnwindResume));
            let call = TerminatorKind::Call {
                target: ret_block,
                call: Call {
                    func: fn_op,
                    args: vec![Operand::Move(state), Operand::Move(args_tupled)],
                    dest: output,
                },
                on_unwind: unwind_block,
            };
            blocks.set_slot(start_block, mk_block(statements, call));

            let body: ExprBody = GExprBody {
                span,
                locals,
                comments: vec![],
                body: blocks.make_contiguous(),
                bound_body_regions: 0,
            };
            Body::Unstructured(body)
        };

        let src = self.translate_closure_src_info(span, closure, args)?;

        Ok(FunDecl {
            def_id,
            item_meta,
            signature,
            src,
            generics: GenericParams::empty(),
            is_global_initializer: None,
            body,
        })
    }
}
