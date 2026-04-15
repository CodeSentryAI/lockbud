extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_public_bridge;
extern crate rustc_span;

use itertools::Itertools;
use log::trace;
use rustc_public::{mir, rustc_internal, ty};
use rustc_public_bridge::IndexedVal;
use std::{
    collections::{HashMap, VecDeque},
    mem,
    ops::{Deref, DerefMut},
    panic,
};

use charon_lib::{ast::*, raise_error, register_error, ullbc_ast::*};

use crate::translate::translate_ctx::ItemTransCtx;

/// A translation context for function bodies.
pub(crate) struct BodyTransCtx<'tcx, 'tctx, 'ictx> {
    /// The translation context for the item.
    pub i_ctx: &'ictx mut ItemTransCtx<'tcx, 'tctx>,

    pub generic_args: Option<ty::GenericArgs>,
    pub local_decls: &'ictx [mir::LocalDecl],
    pub signature: &'ictx mut FunSig,

    /// The (regular) variables in the current function body.
    pub locals: Locals,
    /// The map from rust variable indices to translated variables indices.
    pub locals_map: HashMap<usize, LocalId>,
    /// The translated blocks.
    pub blocks: IndexMap<BlockId, BlockData>,
    /// The map from rust blocks to translated blocks.
    /// Note that when translating terminators like DropAndReplace, we might have
    /// to introduce new blocks which don't appear in the original MIR.
    pub blocks_map: HashMap<mir::BasicBlockIdx, BlockId>,
    /// We register the blocks to translate in a stack, so as to avoid
    /// writing the translation functions as recursive functions. We do
    /// so because we had stack overflows in the past.
    pub blocks_stack: VecDeque<mir::BasicBlockIdx>,
}

impl<'tcx, 'tctx, 'ictx> Deref for BodyTransCtx<'tcx, 'tctx, 'ictx> {
    type Target = ItemTransCtx<'tcx, 'tctx>;
    fn deref(&self) -> &Self::Target {
        self.i_ctx
    }
}
impl<'tcx, 'tctx, 'ictx> DerefMut for BodyTransCtx<'tcx, 'tctx, 'ictx> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.i_ctx
    }
}

impl<'tcx, 'tctx, 'ictx> BodyTransCtx<'tcx, 'tctx, 'ictx> {
    pub(crate) fn new(
        i_ctx: &'ictx mut ItemTransCtx<'tcx, 'tctx>,
        local_decls: &'ictx [mir::LocalDecl],
        signature: &'ictx mut FunSig,
        generic_args: Option<ty::GenericArgs>,
    ) -> Self {
        BodyTransCtx {
            i_ctx,
            local_decls,
            signature,
            generic_args,
            locals: Default::default(),
            locals_map: Default::default(),
            blocks: Default::default(),
            blocks_map: Default::default(),
            blocks_stack: Default::default(),
        }
    }
}

/// A translation context for function blocks.
pub(crate) struct BlockTransCtx<'tcx, 'tctx, 'ictx, 'bctx> {
    /// The translation context for the item.
    pub b_ctx: &'bctx mut BodyTransCtx<'tcx, 'tctx, 'ictx>,
    /// List of currently translated statements
    pub statements: Vec<Statement>,
}

impl<'tcx, 'tctx, 'ictx, 'bctx> BlockTransCtx<'tcx, 'tctx, 'ictx, 'bctx> {
    pub(crate) fn new(b_ctx: &'bctx mut BodyTransCtx<'tcx, 'tctx, 'ictx>) -> Self {
        BlockTransCtx {
            b_ctx,
            statements: Vec::new(),
        }
    }
}

impl<'tcx, 'tctx, 'ictx, 'bctx> Deref for BlockTransCtx<'tcx, 'tctx, 'ictx, 'bctx> {
    type Target = BodyTransCtx<'tcx, 'tctx, 'ictx>;
    fn deref(&self) -> &Self::Target {
        self.b_ctx
    }
}
impl<'tcx, 'tctx, 'ictx, 'bctx> DerefMut for BlockTransCtx<'tcx, 'tctx, 'ictx, 'bctx> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.b_ctx
    }
}

impl ItemTransCtx<'_, '_> {
    pub fn translate_variant_id(&self, id: ty::VariantIdx) -> VariantId {
        VariantId::new(id.to_index())
    }

    fn translate_field_id(&self, id: mir::FieldIdx) -> FieldId {
        FieldId::new(id)
    }
}

impl BodyTransCtx<'_, '_, '_> {
    pub(crate) fn translate_local(&self, local: &mir::Local) -> Option<LocalId> {
        self.locals_map.get(local).copied()
    }

    pub(crate) fn push_var(&mut self, rid: usize, ty: Ty, name: Option<String>, span: Span) {
        let local_id = self.locals.locals.push_with(|index| Local {
            index,
            name,
            ty,
            span,
        });
        self.locals_map.insert(rid, local_id);
    }

    fn subst_ty(&mut self, span: Span, ty: ty::Ty) -> Result<ty::Ty, Error> {
        // For some extremely bizarre reason, constructor functions (e.g. Option::Some)
        // have their locals non-substituted, despite the function having the right generics.
        // We thus do it manually, which is... not good.
        match ty.kind() {
            ty::TyKind::Param(param) => Ok(self.generic_args.as_ref().unwrap()[param]),
            ty::TyKind::RigidTy(rigid) => match rigid {
                ty::RigidTy::Adt(def, args)
                    if args
                        .0
                        .iter()
                        .any(|a| a.ty().is_some_and(|ty| ty.kind().rigid().is_none())) =>
                {
                    let new_args: Vec<_> = args
                        .clone()
                        .0
                        .into_iter()
                        .map(|arg| -> Result<_, Error> {
                            match arg {
                                ty::GenericArgKind::Type(ty) => {
                                    let ty = self.subst_ty(span, ty)?;
                                    Ok(ty::GenericArgKind::Type(ty))
                                }
                                _ => Ok(arg),
                            }
                        })
                        .try_collect()?;
                    Ok(ty::Ty::from_rigid_kind(ty::RigidTy::Adt(
                        def,
                        ty::GenericArgs(new_args),
                    )))
                }
                _ => Ok(ty),
            },

            _ => Ok(ty),
        }
    }

    fn translate_ty(&mut self, span: Span, ty: ty::Ty) -> Result<Ty, Error> {
        let ty = self.subst_ty(span, ty)?;
        self.i_ctx.translate_ty(span, ty)
    }

    /// Translate a function's local variables by adding them in the environment.
    fn translate_body_locals(&mut self, body: &mir::Body) -> Result<(), Error> {
        // Translate the parameters
        for (index, var) in body.locals().iter().enumerate() {
            trace!("Translating local of index {} and type {:?}", index, var.ty);

            // Find the name of the variable
            let name: Option<String> = body.var_debug_info.iter().find_map(|v| {
                if let mir::VarDebugInfoContents::Place(place) = &v.value
                    && place.projection.is_empty()
                    && place.local == index
                {
                    Some(v.name.clone())
                } else {
                    None
                }
            });

            // Translate the type
            let span = self.translate_span_from_smir(&var.span);
            let ty = self.translate_ty(span, var.ty)?;

            // Add the variable to the environment
            self.push_var(index, ty, name, span);
        }

        Ok(())
    }

    /// Translate a basic block id and register it, if it hasn't been done.
    fn translate_basic_block_id(&mut self, block_id: mir::BasicBlockIdx) -> BlockId {
        match self.blocks_map.get(&block_id) {
            Some(id) => *id,
            // Generate a fresh id - this also registers the block
            None => {
                // Push to the stack of blocks awaiting translation
                self.blocks_stack.push_back(block_id);
                let id = self.blocks.reserve_slot();
                // Insert in the map
                self.blocks_map.insert(block_id, id);
                id
            }
        }
    }

    fn translate_basic_block(&mut self, block: &mir::BasicBlock) -> Result<BlockData, Error> {
        // Translate the statements
        let mut block_ctx = BlockTransCtx::new(self);
        for statement in &block.statements {
            block_ctx.translate_statement(statement)?;
        }

        // Translate the terminator
        let terminator = block_ctx.translate_terminator(&block.terminator)?;

        Ok(BlockData {
            statements: block_ctx.statements,
            terminator,
        })
    }

    // The body is translated as if the locals are:
    // ret value, ..., arg-1, ..., arg-N, ...
    // However, there is only one argument with the tupled arg-1..N arguments;
    // we must thus shift all locals with index >=spread_arg by 1, and add a new local
    // for the tupled arg, giving us:
    // ret value, ..., args, arg-1, ..., arg-N, ...
    // We then add N statements of the form `locals[I+N+1] := move locals[I].N`,
    // to destructure the arguments.
    // We also modify the signature, by adding the tupled argument.
    // The spread_arg is the local of the spread argument; it's None if one needs to be
    // added, for closures.
    fn spread_argument(&mut self, spread_loc_opt: Option<usize>) -> Result<(), Error> {
        let spread_loc = spread_loc_opt.unwrap_or_else(|| {
            let spread_tys: Vec<_> = self.signature.inputs.iter().cloned().skip(1).collect();
            let inputs_tupled = Ty::mk_tuple(spread_tys.clone());
            let mut old_locals = mem::take(&mut self.locals.locals).into_iter();

            // keep the return place and the first argument (the self place)
            self.locals.locals.extend(old_locals.by_ref().take(2));
            let spread_loc = self
                .locals
                .new_var(Some("spread_args".to_string()), inputs_tupled)
                .as_local()
                .unwrap()
                .index();
            self.locals
                .locals
                .extend(old_locals.update(|l| l.index += 1));

            // update the rest of the locals
            self.blocks.dyn_visit_mut(|local: &mut LocalId| {
                let idx = local.index();
                if idx >= spread_loc {
                    *local = LocalId::new(idx + 1)
                }
            });

            spread_loc
        });

        let TyKind::Adt(TypeDeclRef {
            id: TypeId::Tuple,
            generics,
        }) = self.locals.locals[spread_loc].ty.kind()
        else {
            raise_error!(
                self,
                Span::dummy(),
                "Expected a tuple type for spread argument"
            );
        };
        let inputs_untupled: Vec<_> = generics.types.clone().into_iter().collect();

        // Update the signature
        let inputs = &mut self.signature.inputs;
        let inputs_spread = inputs.split_off(inputs.len() - inputs_untupled.len());
        inputs.push(Ty::mk_tuple(inputs_spread));
        self.locals.arg_count = inputs.len();

        // we only need to re-add the projections when spread_arg is not provided
        if spread_loc_opt.is_some() {
            return Ok(());
        }

        // Add projections as prelude
        let tupled_arg = self.locals.place_for_var(LocalId::from_raw(spread_loc));
        let spread_arg_count = inputs_untupled.len();
        let new_stts = inputs_untupled
            .iter()
            .cloned()
            .enumerate()
            .filter_map(|(i, ty)| {
                let idx = LocalId::new(spread_loc + i + 1);
                let Some(local) = self.locals.locals.get(idx) else {
                    return None;
                };
                assert_eq!(local.ty, ty);
                let place = Place::new(local.index, local.ty.clone());
                let nth_field = tupled_arg.clone().project(
                    ProjectionElem::Field(FieldProjKind::Tuple(spread_arg_count), FieldId::new(i)),
                    ty,
                );
                Some(Statement::new(
                    Span::dummy(),
                    StatementKind::Assign(place, Rvalue::Use(Operand::Move(nth_field))),
                ))
            });
        self.blocks[BlockId::ZERO].statements.splice(0..0, new_stts);
        Ok(())
    }

    /// Translate a function body.
    pub fn translate_body(
        &mut self,
        span: Span,
        instance: mir::mono::Instance,
        body: &mir::Body,
    ) -> Body {
        // Stopgap measure because there are still many panics in charon and hax.
        let mut this = panic::AssertUnwindSafe(&mut *self);
        let res = panic::catch_unwind(move || this.translate_body_aux(instance, body));
        match res {
            Ok(Ok(body)) => body,
            // Translation error
            Ok(Err(e)) => {
                println!(
                    "Thread errored when extracting body of {}: {e:?}",
                    instance.name()
                );
                Body::Error(e)
            }
            Err(_) => {
                let e = register_error!(self, span, "Thread panicked when extracting body.");
                Body::Error(e)
            }
        }
    }

    fn translate_body_aux(
        &mut self,
        instance: mir::mono::Instance,
        body: &mir::Body,
    ) -> Result<Body, Error> {
        // Compute the span information
        let span = self.translate_span_from_smir(&body.span);

        // Initialize the local variables
        trace!("Translating the body locals");
        self.locals.arg_count = self.signature.inputs.len();
        self.translate_body_locals(&body)?;

        // Translate the expression body
        trace!("Translating the expression body");

        // Register the start block
        let id = self.translate_basic_block_id(0);
        assert!(id == START_BLOCK_ID);

        // For as long as there are blocks in the stack, translate them
        while let Some(mir_block_id) = self.blocks_stack.pop_front() {
            let mir_block = body.blocks.get(mir_block_id).unwrap();
            let block_id = self.translate_basic_block_id(mir_block_id);
            let block = self.translate_basic_block(mir_block)?;
            self.blocks.set_slot(block_id, block);
        }

        // We might need to tuple arguments, and possibly create a local too
        if let Some(spread_local) = body.spread_arg() {
            self.spread_argument(Some(spread_local))?;
        } else if self.instance_is_closure(instance) {
            self.spread_argument(None)?;
        }

        // Create the body
        Ok(Body::Unstructured(ExprBody {
            span,
            bound_body_regions: 0,
            locals: mem::take(&mut self.locals),
            body: mem::take(&mut self.blocks).make_contiguous(),
            comments: vec![],
        }))
    }
}

impl<'tcx> BlockTransCtx<'tcx, '_, '_, '_> {
    fn translate_binaryop_kind(&mut self, _span: Span, binop: mir::BinOp) -> Result<BinOp, Error> {
        Ok(match binop {
            mir::BinOp::BitXor => BinOp::BitXor,
            mir::BinOp::BitAnd => BinOp::BitAnd,
            mir::BinOp::BitOr => BinOp::BitOr,
            mir::BinOp::Eq => BinOp::Eq,
            mir::BinOp::Lt => BinOp::Lt,
            mir::BinOp::Le => BinOp::Le,
            mir::BinOp::Ne => BinOp::Ne,
            mir::BinOp::Ge => BinOp::Ge,
            mir::BinOp::Gt => BinOp::Gt,
            mir::BinOp::Div => BinOp::Div(OverflowMode::UB),
            mir::BinOp::Rem => BinOp::Rem(OverflowMode::Wrap),
            mir::BinOp::Add => BinOp::Add(OverflowMode::Wrap),
            mir::BinOp::AddUnchecked => BinOp::Add(OverflowMode::UB),
            mir::BinOp::Sub => BinOp::Sub(OverflowMode::Wrap),
            mir::BinOp::SubUnchecked => BinOp::Sub(OverflowMode::UB),
            mir::BinOp::Mul => BinOp::Mul(OverflowMode::Wrap),
            mir::BinOp::MulUnchecked => BinOp::Mul(OverflowMode::UB),
            mir::BinOp::Shl => BinOp::Shl(OverflowMode::Wrap),
            mir::BinOp::ShlUnchecked => BinOp::Shl(OverflowMode::UB),
            mir::BinOp::Shr => BinOp::Shr(OverflowMode::Wrap),
            mir::BinOp::ShrUnchecked => BinOp::Shr(OverflowMode::UB),
            mir::BinOp::Cmp => BinOp::Cmp,
            mir::BinOp::Offset => BinOp::Offset,
        })
    }

    fn translate_checked_binaryop_kind(
        &mut self,
        span: Span,
        binop: mir::BinOp,
    ) -> Result<BinOp, Error> {
        Ok(match binop {
            mir::BinOp::Add => BinOp::AddChecked,
            mir::BinOp::Sub => BinOp::SubChecked,
            mir::BinOp::Mul => BinOp::MulChecked,
            _ => raise_error!(self, span, "Invalid checked binop: {:?}", binop),
        })
    }

    fn deref_middle_tys(&self, l: ty::Ty, r: ty::Ty) -> Option<(ty::Ty, ty::Ty)> {
        // See:
        // https://github.com/rust-lang/rust/blob/b3f8586fb1e4859678d6b231e780ff81801d2282/compiler/rustc_codegen_ssa/src/base.rs#L220
        let lk = l.kind();
        let Some(lr) = lk.rigid() else {
            return None;
        };
        let rk = r.kind();
        let Some(rr) = rk.rigid() else {
            return None;
        };

        match (lr, rr) {
            (ty::RigidTy::Ref(_, a, _), ty::RigidTy::Ref(_, b, _) | ty::RigidTy::RawPtr(b, _))
            | (ty::RigidTy::RawPtr(a, _), ty::RigidTy::RawPtr(b, _)) => Some((*a, *b)),
            (ty::RigidTy::Adt(def_a, args1), ty::RigidTy::Adt(def_b, args2)) => {
                // find the only field that is not a ZST
                let non_zst: Vec<_> = def_a.variants()[0]
                    .fields()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, f)| {
                        let ty = f.ty_with_args(&args1);
                        if let Ok(layout) = ty.layout()
                            && !layout.shape().is_1zst()
                        {
                            Some((i, ty))
                        } else {
                            None
                        }
                    })
                    .rev()
                    .collect();
                if non_zst.len() == 0 {
                    return None;
                }
                // the last field should be the DST for the right-hand side
                let (idx, ty_l) = non_zst[0];
                let ty_r = def_b.variants()[0].fields()[idx].ty_with_args(&args2);
                self.deref_middle_tys(ty_l, ty_r)
            }
            _ => None,
        }
    }

    fn translate_unsizing_metadata(
        &mut self,
        span: Span,
        src_ty: ty::Ty,
        tgt_ty: ty::Ty,
    ) -> Result<UnsizingMetadata, Error> {
        let tcx = self.t_ctx.tcx;

        let Some((src_ty, tgt_ty)) = self.deref_middle_tys(src_ty, tgt_ty) else {
            println!("Couldn't deref middle for {src_ty:?} => {tgt_ty:?}");
            return Ok(UnsizingMetadata::Unknown);
        };

        use rustc_middle::ty;
        let src_ty = rustc_internal::internal(tcx, src_ty);
        let tgt_ty = rustc_internal::internal(tcx, tgt_ty);
        let typing_env = ty::TypingEnv::fully_monomorphized();
        let (src_ty, tgt_ty) = tcx.struct_lockstep_tails_for_codegen(src_ty, tgt_ty, typing_env);
        match (src_ty.kind(), tgt_ty.kind()) {
            (ty::Array(_, len), ty::Slice(_) | ty::Str) => {
                let len = rustc_internal::stable(len);
                let len = self.translate_tyconst_to_const_expr(span, &len)?;
                Ok(UnsizingMetadata::Length(Box::new(len)))
            }
            (ty::Dynamic(from_preds, ..), ty::Dynamic(to_preds, ..)) => {
                // see
                // https://github.com/rust-lang/rust/blob/9725c4baacef19345e13f91b27e66e10ef5592ae/compiler/rustc_codegen_ssa/src/base.rs#L171-L208
                let target_principal = to_preds.principal();
                if from_preds.principal() != target_principal
                    && target_principal.is_some()
                    && let Some(vptr_entry_idx) =
                        self.t_ctx.tcx.supertrait_vtable_slot((src_ty, tgt_ty))
                {
                    Ok(UnsizingMetadata::VTableUpcast(vec![FieldId::new(
                        vptr_entry_idx,
                    )]))
                } else {
                    // Fallback to original vtable
                    Ok(UnsizingMetadata::VTableUpcast(vec![]))
                }
            }
            (_, ty::Dynamic(preds, ..)) => {
                let self_ty = rustc_public::rustc_internal::stable(src_ty);
                let trait_ref = preds.principal().map(|bound_tref| {
                    let ex_trait_ref = self
                        .t_ctx
                        .tcx
                        .instantiate_bound_regions_with_erased(bound_tref);
                    let stable = rustc_public::rustc_internal::stable(ex_trait_ref);
                    stable.with_self_ty(self_ty)
                });
                let vtable_global = self.register_vtable(span, self_ty, trait_ref);
                let vtable_ref = GlobalDeclRef {
                    id: vtable_global,
                    generics: Box::new(GenericArgs::empty()),
                };
                let meta = ConstantExpr {
                    kind: ConstantExprKind::Global(vtable_ref),
                    ty: TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty(),
                };
                Ok(UnsizingMetadata::VTable(
                    self.dummy_trait_ref(),
                    Box::new(meta),
                ))
            }
            _ => {
                println!("Unknown unsize for ({src_ty:?}) => ({tgt_ty:?})");
                Ok(UnsizingMetadata::Unknown)
            }
        }
    }

    fn translate_projection(
        &mut self,
        span: Span,
        of_place: Place,
        place_ty: ty::Ty,
        variant_id: Option<VariantId>,
        proj: &mir::ProjectionElem,
    ) -> Result<(Place, ty::Ty, Option<VariantId>), Error> {
        let proj_ty = proj.ty(place_ty)?;
        let ty = self.translate_ty(span, proj_ty)?;

        let proj_elem = match proj {
            mir::ProjectionElem::Deref => ProjectionElem::Deref,
            mir::ProjectionElem::Index(local) => {
                let var_id = self.translate_local(local).unwrap();
                let local = self.locals.place_for_var(var_id);
                ProjectionElem::Index {
                    offset: Box::new(Operand::Copy(local)),
                    from_end: false,
                }
            }
            mir::ProjectionElem::ConstantIndex {
                offset, from_end, ..
            } => {
                let idx = ConstantExpr {
                    kind: ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Unsigned(
                        UIntTy::Usize,
                        *offset as u128,
                    ))),
                    ty: TyKind::Literal(LiteralTy::UInt(UIntTy::Usize)).into_ty(),
                };
                ProjectionElem::Index {
                    offset: Box::new(Operand::Const(Box::new(idx))),
                    from_end: *from_end,
                }
            }
            mir::ProjectionElem::Subslice { from, to, from_end } => {
                let from = ConstantExpr {
                    kind: ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Unsigned(
                        UIntTy::Usize,
                        *from as u128,
                    ))),
                    ty: TyKind::Literal(LiteralTy::UInt(UIntTy::Usize)).into_ty(),
                };
                let to = ConstantExpr {
                    kind: ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Unsigned(
                        UIntTy::Usize,
                        *to as u128,
                    ))),
                    ty: TyKind::Literal(LiteralTy::UInt(UIntTy::Usize)).into_ty(),
                };
                ProjectionElem::Subslice {
                    from: Box::new(Operand::Const(Box::new(from))),
                    to: Box::new(Operand::Const(Box::new(to))),
                    from_end: *from_end,
                }
            }
            mir::ProjectionElem::Downcast(variant) => {
                return Ok((of_place, proj_ty, Some(self.translate_variant_id(*variant))));
            }
            mir::ProjectionElem::Field(field_idx, _) => {
                let kind = match of_place.ty().kind() {
                    TyKind::Adt(TypeDeclRef {
                        id: TypeId::Tuple,
                        generics,
                    }) => FieldProjKind::Tuple(generics.types.elem_count()),
                    TyKind::Adt(TypeDeclRef {
                        id: TypeId::Adt(id),
                        ..
                    }) => FieldProjKind::Adt(*id, variant_id),
                    kind => unreachable!("Unexpected type in field projection: {kind:?}"),
                };
                ProjectionElem::Field(kind, FieldId::from_usize(*field_idx))
            }
            mir::ProjectionElem::OpaqueCast(..) => {
                raise_error!(self, span, "Unexpected ProjectionElem::OpaqueCast");
            }
        };
        let place = of_place.project(proj_elem, ty);
        Ok((place, proj_ty, None))
    }

    /// Translate a place
    /// TODO: Hax represents places in a different manner than MIR. We should
    /// update our representation of places to match the Hax representation.
    fn translate_place(&mut self, span: Span, place: &mir::Place) -> Result<Place, Error> {
        let var_id = self.translate_local(&place.local).unwrap();
        let local = self.locals.place_for_var(var_id);
        let local_ty = self.local_decls[place.local].ty;
        place
            .projection
            .iter()
            .try_fold(
                (local, local_ty, None),
                |(place, place_ty, variant), proj| {
                    self.translate_projection(span, place, place_ty, variant, proj)
                },
            )
            .map(|(place, _, _)| place)
    }

    /// Translate an operand with its type
    fn translate_operand_with_type(
        &mut self,
        span: Span,
        operand: &mir::Operand,
    ) -> Result<(Operand, Ty), Error> {
        match operand {
            mir::Operand::Copy(place) => {
                let p = self.translate_place(span, place)?;
                let ty = p.ty().clone();
                Ok((Operand::Copy(p), ty))
            }
            mir::Operand::Move(place) => {
                let p = self.translate_place(span, place)?;
                let ty = p.ty().clone();
                Ok((Operand::Move(p), ty))
            }
            mir::Operand::Constant(const_op) => {
                let ty = self.translate_ty(span, const_op.ty())?;
                let cexpr = match &const_op.const_.kind() {
                    ty::ConstantKind::Allocated(alloc) => {
                        self.translate_allocation(span, alloc, ty.kind(), const_op.ty())?
                    }
                    ty::ConstantKind::ZeroSized => {
                        self.translate_zst_constant(span, ty.kind(), const_op.ty())?
                    }
                    ty::ConstantKind::Param(_) => ConstantExpr {
                        kind: ConstantExprKind::Opaque("Unhandled: Param".into()),
                        ty: ty.clone(),
                    },
                    ty::ConstantKind::Ty(_) => ConstantExpr {
                        kind: ConstantExprKind::Opaque("Unhandled: ty".into()),
                        ty: ty.clone(),
                    },
                    ty::ConstantKind::Unevaluated(_) => ConstantExpr {
                        kind: ConstantExprKind::Opaque("Unhandled: uneval".into()),
                        ty: ty.clone(),
                    },
                };
                Ok((Operand::Const(Box::new(cexpr)), ty))
            }
            mir::Operand::RuntimeChecks(check) => {
                let op = match check {
                    mir::RuntimeChecks::UbChecks => NullOp::UbChecks,
                    mir::RuntimeChecks::OverflowChecks => NullOp::OverflowChecks,
                    mir::RuntimeChecks::ContractChecks => NullOp::ContractChecks,
                };
                let local = self.locals.new_var(None, Ty::mk_bool());
                self.statements.push(Statement {
                    span,
                    kind: StatementKind::StorageLive(local.as_local().unwrap()),
                    comments_before: vec![],
                });
                self.statements.push(Statement {
                    span,
                    kind: StatementKind::Assign(
                        local.clone(),
                        Rvalue::NullaryOp(op, Ty::mk_bool()),
                    ),
                    comments_before: vec![],
                });
                Ok((Operand::Move(local), Ty::mk_bool()))
            }
        }
    }

    /// Translate an operand
    fn translate_operand(&mut self, span: Span, operand: &mir::Operand) -> Result<Operand, Error> {
        Ok(self.translate_operand_with_type(span, operand)?.0)
    }

    /// Translate a `BorrowKind`
    fn translate_borrow_kind(&mut self, borrow_kind: mir::BorrowKind) -> BorrowKind {
        match borrow_kind {
            mir::BorrowKind::Shared => BorrowKind::Shared,
            mir::BorrowKind::Mut { kind } => match kind {
                mir::MutBorrowKind::Default => BorrowKind::Mut,
                mir::MutBorrowKind::TwoPhaseBorrow => BorrowKind::TwoPhaseMut,
                mir::MutBorrowKind::ClosureCapture => BorrowKind::UniqueImmutable,
            },
            mir::BorrowKind::Fake(mir::FakeBorrowKind::Shallow) => BorrowKind::Shallow,
            // This one is used only in deref patterns.
            mir::BorrowKind::Fake(mir::FakeBorrowKind::Deep) => unimplemented!(),
        }
    }

    /// Translate an rvalue
    fn translate_rvalue(
        &mut self,
        span: Span,
        rvalue: &mir::Rvalue,
        tgt_ty: &Ty,
    ) -> Result<Rvalue, Error> {
        match rvalue {
            mir::Rvalue::Use(operand) => Ok(Rvalue::Use(self.translate_operand(span, operand)?)),
            mir::Rvalue::CopyForDeref(place) => {
                // According to the documentation, it seems to be an optimisation
                // for drop elaboration. We treat it as a regular copy.
                let place = self.translate_place(span, place)?;
                Ok(Rvalue::Use(Operand::Copy(place)))
            }
            mir::Rvalue::Repeat(operand, cnst) => {
                let c = self.translate_tyconst_to_const_expr(span, cnst)?;
                let (operand, t) = self.translate_operand_with_type(span, operand)?;
                // Remark: we could desugar this into a function call later.
                Ok(Rvalue::Repeat(operand, t, Box::new(c)))
            }
            mir::Rvalue::Ref(_region, borrow_kind, place) => {
                let place = self.translate_place(span, place)?;
                let borrow_kind = self.translate_borrow_kind(*borrow_kind);
                Ok(Rvalue::Ref {
                    place,
                    kind: borrow_kind,
                    ptr_metadata: Operand::mk_const_unit(),
                })
            }
            mir::Rvalue::AddressOf(mtbl, place) => {
                let mtbl = match mtbl {
                    mir::RawPtrKind::Mut => RefKind::Mut,
                    mir::RawPtrKind::Const => RefKind::Shared,
                    mir::RawPtrKind::FakeForPtrMetadata => RefKind::Shared,
                };
                let place = self.translate_place(span, place)?;
                Ok(Rvalue::RawPtr {
                    place,
                    kind: mtbl,
                    ptr_metadata: Operand::mk_const_unit(),
                })
            }
            mir::Rvalue::Len(place) => {
                let place = self.translate_place(span, place)?;
                let ty = place.ty().clone();
                let cg = match ty.kind() {
                    TyKind::Array(_, len) => Some(len.clone()),
                    TyKind::Slice(_) => None,
                    _ => unreachable!(),
                };
                Ok(Rvalue::Len(place, ty, cg))
            }
            mir::Rvalue::Cast(cast_kind, mir_operand, mir_tgt_ty) => {
                trace!("Rvalue::Cast: {:?}", rvalue);
                // Translate the target type
                let tgt_ty = self.translate_ty(span, *mir_tgt_ty)?;

                // Translate the operand
                let (mut operand, src_ty) = self.translate_operand_with_type(span, mir_operand)?;

                let cast_kind = match cast_kind {
                    mir::CastKind::IntToInt
                    | mir::CastKind::IntToFloat
                    | mir::CastKind::FloatToInt
                    | mir::CastKind::FloatToFloat => {
                        let tgt_ty = *tgt_ty.kind().as_literal().unwrap();
                        let src_ty = *src_ty.kind().as_literal().unwrap();
                        CastKind::Scalar(src_ty, tgt_ty)
                    }
                    mir::CastKind::PointerCoercion(
                        mir::PointerCoercion::ClosureFnPointer(_),
                        ..,
                    ) => {
                        let op_ty = mir_operand.ty(self.local_decls)?;
                        let op_ty_kind = op_ty.kind();
                        let Some(ty::RigidTy::Closure(def, args)) = op_ty_kind.rigid() else {
                            raise_error!(self, span, "ClosureFnPointer without closure?");
                        };
                        let fn_id = self.register_closure_as_fn_id(span, *def, args.clone());
                        let fn_ptr = FnPtr {
                            kind: Box::new(FnPtrKind::Fun(FunId::Regular(fn_id))),
                            generics: Box::new(GenericArgs::empty()),
                        };
                        let src_ty = TyKind::FnDef(RegionBinder::empty(fn_ptr.clone())).into_ty();
                        operand = Operand::Const(Box::new(ConstantExpr {
                            kind: ConstantExprKind::FnDef(fn_ptr),
                            ty: src_ty.clone(),
                        }));
                        CastKind::FnPtr(src_ty, tgt_ty)
                    }
                    mir::CastKind::PtrToPtr
                    | mir::CastKind::FnPtrToPtr
                    | mir::CastKind::PointerExposeAddress
                    | mir::CastKind::PointerWithExposedProvenance
                    | mir::CastKind::PointerCoercion(
                        mir::PointerCoercion::MutToConstPointer
                        | mir::PointerCoercion::ArrayToPointer,
                        ..,
                    ) => CastKind::RawPtr(src_ty, tgt_ty),
                    mir::CastKind::PointerCoercion(
                        mir::PointerCoercion::UnsafeFnPointer
                        | mir::PointerCoercion::ReifyFnPointer(_),
                        ..,
                    ) => CastKind::FnPtr(src_ty, tgt_ty),
                    mir::CastKind::Subtype | mir::CastKind::Transmute => {
                        CastKind::Transmute(src_ty, tgt_ty)
                    }
                    mir::CastKind::PointerCoercion(mir::PointerCoercion::Unsize) => {
                        let mir_src_ty = mir_operand.ty(self.local_decls)?;
                        let unsizing_meta =
                            self.translate_unsizing_metadata(span, mir_src_ty, *mir_tgt_ty)?;
                        CastKind::Unsize(src_ty.clone(), tgt_ty.clone(), unsizing_meta)
                    }
                };
                Ok(Rvalue::UnaryOp(UnOp::Cast(cast_kind), operand))
            }
            mir::Rvalue::BinaryOp(binop, left, right) => Ok(Rvalue::BinaryOp(
                self.translate_binaryop_kind(span, *binop)?,
                self.translate_operand(span, left)?,
                self.translate_operand(span, right)?,
            )),
            mir::Rvalue::CheckedBinaryOp(binop, left, right) => Ok(Rvalue::BinaryOp(
                self.translate_checked_binaryop_kind(span, *binop)?,
                self.translate_operand(span, left)?,
                self.translate_operand(span, right)?,
            )),
            mir::Rvalue::UnaryOp(unop, operand) => {
                let operand = self.translate_operand(span, operand)?;
                let unop = match unop {
                    mir::UnOp::Not => UnOp::Not,
                    mir::UnOp::Neg => UnOp::Neg(OverflowMode::UB),
                    mir::UnOp::PtrMetadata => match operand {
                        Operand::Copy(p) | Operand::Move(p) => {
                            return Ok(Rvalue::Use(Operand::Copy(
                                p.project(ProjectionElem::PtrMetadata, tgt_ty.clone()),
                            )));
                        }
                        Operand::Const(_) => panic!("const metadata"),
                    },
                };
                Ok(Rvalue::UnaryOp(unop, operand))
            }
            mir::Rvalue::Discriminant(place) => {
                let place = self.translate_place(span, place)?;
                Ok(Rvalue::Discriminant(place))
            }
            mir::Rvalue::Aggregate(aggregate_kind, operands) => {
                // It seems this instruction is not present in certain passes:
                // for example, it seems it is not used in optimized MIR, where
                // ADT initialization is split into several instructions, for
                // instance:
                // ```
                // p = Pair { x:xv, y:yv };
                // ```
                // Might become:
                // ```
                // p.x = x;
                // p.y = yv;
                // ```

                // First translate the operands
                let operands_t: Vec<Operand> = operands
                    .iter()
                    .map(|op| self.translate_operand(span, op))
                    .try_collect()?;

                match aggregate_kind {
                    mir::AggregateKind::Array(ty) => {
                        let t_ty = self.translate_ty(span, *ty)?;
                        let cg = ConstantExpr::mk_usize(ScalarValue::Unsigned(
                            UIntTy::Usize,
                            operands_t.len() as u128,
                        ));
                        Ok(Rvalue::Aggregate(
                            AggregateKind::Array(t_ty, Box::new(cg)),
                            operands_t,
                        ))
                    }
                    mir::AggregateKind::Tuple => {
                        let tref = TypeDeclRef::new(TypeId::Tuple, GenericArgs::empty());
                        Ok(Rvalue::Aggregate(
                            AggregateKind::Adt(tref, None, None),
                            operands_t,
                        ))
                    }
                    mir::AggregateKind::Adt(
                        item,
                        variant_idx,
                        generics,
                        _user_annotation,
                        field_index,
                    ) => {
                        use ty::AdtKind;
                        trace!("{:?}", rvalue);

                        let id = self.register_type_decl_id(span, *item, generics.clone());
                        let tref = TypeDeclRef {
                            id: TypeId::Adt(id),
                            generics: Box::new(GenericArgs::empty()),
                        };
                        let variant_id = match item.kind() {
                            AdtKind::Struct | AdtKind::Union => None,
                            AdtKind::Enum => Some(self.translate_variant_id(*variant_idx)),
                        };
                        let field_id = match item.kind() {
                            AdtKind::Struct | AdtKind::Enum => None,
                            AdtKind::Union => Some(self.translate_field_id(field_index.unwrap())),
                        };

                        let akind = AggregateKind::Adt(tref, variant_id, field_id);
                        Ok(Rvalue::Aggregate(akind, operands_t))
                    }
                    mir::AggregateKind::Closure(def, args) => {
                        let id = self.register_closure_type_decl_id(span, *def, args.clone());
                        let tref = TypeDeclRef {
                            id: TypeId::Adt(id),
                            generics: Box::new(GenericArgs::empty()),
                        };
                        let akind = AggregateKind::Adt(tref, None, None);
                        Ok(Rvalue::Aggregate(akind, operands_t))
                    }
                    mir::AggregateKind::RawPtr(ty, is_mut) => {
                        // TODO: replace with a call to `ptr::from_raw_parts`.
                        let t_ty = self.translate_ty(span, *ty)?;
                        let mutability = if matches!(is_mut, mir::Mutability::Mut) {
                            RefKind::Mut
                        } else {
                            RefKind::Shared
                        };

                        let akind = AggregateKind::RawPtr(t_ty, mutability);

                        Ok(Rvalue::Aggregate(akind, operands_t))
                    }
                    mir::AggregateKind::Coroutine(..)
                    | mir::AggregateKind::CoroutineClosure(..) => {
                        raise_error!(self, span, "Coroutines are not supported");
                    }
                }
            }
            mir::Rvalue::ShallowInitBox(op, ty) => {
                let op = self.translate_operand(span, op)?;
                let ty = self.translate_ty(span, *ty)?;
                Ok(Rvalue::ShallowInitBox(op, ty))
            }
            mir::Rvalue::ThreadLocalRef(def) => {
                let const_ty = match tgt_ty.kind() {
                    TyKind::Ref(_, ty, _) | TyKind::RawPtr(ty, _) => ty.clone(),
                    _ => raise_error!(
                        self,
                        span,
                        "ThreadLocalRef target type must be a reference or raw pointer"
                    ),
                };

                let stt: mir::mono::StaticDef = def.clone().try_into()?;
                let global = self.register_global_from_static(span, stt);
                let place = Place::new_global(
                    GlobalDeclRef {
                        id: global,
                        generics: Box::new(GenericArgs::empty()),
                    },
                    const_ty,
                );

                register_error!(
                    self,
                    span,
                    "Obol translated a ThreadLocalRef as just a constant allocation; concurrency is not yet supported."
                );

                match tgt_ty.kind() {
                    TyKind::Ref(_, _, kind) => Ok(Rvalue::Ref {
                        place,
                        kind: kind.clone().into(),
                        ptr_metadata: Operand::mk_const_unit(),
                    }),
                    TyKind::RawPtr(_, rk) => Ok(Rvalue::RawPtr {
                        place,
                        kind: rk.clone().into(),
                        ptr_metadata: Operand::mk_const_unit(),
                    }),
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Translate a statement
    ///
    /// We return an option, because we ignore some statements (`Nop`, `StorageLive`...)
    fn translate_statement(&mut self, statement: &mir::Statement) -> Result<(), Error> {
        trace!("About to translate statement (MIR) {:?}", statement);
        let span = self.t_ctx.translate_span_from_smir(&statement.span);

        let t_statement: Option<StatementKind> = match &statement.kind {
            mir::StatementKind::Assign(place, rvalue) => {
                let t_place = self.translate_place(span, &place)?;
                let t_rvalue = self.translate_rvalue(span, &rvalue, &t_place.ty)?;
                Some(StatementKind::Assign(t_place, t_rvalue))
            }
            mir::StatementKind::SetDiscriminant {
                place,
                variant_index,
            } => {
                let t_place = self.translate_place(span, &place)?;
                let variant_id = self.translate_variant_id(*variant_index);
                Some(StatementKind::SetDiscriminant(t_place, variant_id))
            }
            mir::StatementKind::StorageLive(local) => {
                let var_id = self.translate_local(&local).unwrap();
                Some(StatementKind::StorageLive(var_id))
            }
            mir::StatementKind::StorageDead(local) => {
                let var_id = self.translate_local(&local).unwrap();
                Some(StatementKind::StorageDead(var_id))
            }
            // This asserts the operand true on pain of UB. We treat it like a normal assertion.
            mir::StatementKind::Intrinsic(mir::NonDivergingIntrinsic::Assume(op)) => {
                let op = self.translate_operand(span, &op)?;
                Some(StatementKind::Assert {
                    assert: Assert {
                        cond: op,
                        expected: true,
                        check_kind: None,
                    },
                    on_failure: AbortKind::UndefinedBehavior,
                })
            }
            mir::StatementKind::Intrinsic(mir::NonDivergingIntrinsic::CopyNonOverlapping(
                mir::CopyNonOverlapping { src, dst, count },
            )) => {
                let src = self.translate_operand(span, &src)?;
                let dst = self.translate_operand(span, &dst)?;
                let count = self.translate_operand(span, &count)?;
                Some(StatementKind::CopyNonOverlapping(Box::new(
                    CopyNonOverlapping { src, dst, count },
                )))
            }
            mir::StatementKind::PlaceMention(p) => {
                let place = self.translate_place(span, p)?;
                if place.is_local() {
                    None
                } else {
                    Some(StatementKind::PlaceMention(place))
                }
            }
            // This is for the stacked borrows memory model.
            mir::StatementKind::Retag(_, _) => None,
            // These two are only there to make borrow-checking accept less code, and are removed
            // in later MIRs.
            mir::StatementKind::FakeRead(..) => None,
            // There are user-provided type annotations with no semantic effect (since we get a
            // fully-typechecked MIR (TODO: this isn't quite true with opaque types, we should
            // really use promoted MIR)).
            mir::StatementKind::AscribeUserType { .. } => None,
            // Used for coverage instrumentation.
            mir::StatementKind::Coverage(_) => None,
            // Used in the interpreter to check that const code doesn't run for too long or even
            // indefinitely.
            mir::StatementKind::ConstEvalCounter => None,
            // Semantically equivalent to `Nop`, used only for rustc lints.
            mir::StatementKind::Nop => None,
        };

        // Add the span information
        t_statement.map(|kind| self.statements.push(Statement::new(span, kind)));
        Ok(())
    }

    /// Translate a terminator
    fn translate_terminator(&mut self, terminator: &mir::Terminator) -> Result<Terminator, Error> {
        trace!("About to translate terminator (MIR) {:?}", terminator);
        // Compute the span information beforehand (we might need it to introduce
        // intermediate statements - we desugar some terminators)
        let span = self.translate_span_from_smir(&terminator.span);

        // Translate the terminator
        let t_terminator: TerminatorKind = match &terminator.kind {
            mir::TerminatorKind::Goto { target } => {
                let target = self.translate_basic_block_id(*target);
                TerminatorKind::Goto { target }
            }
            mir::TerminatorKind::SwitchInt { discr, targets } => {
                // Translate the operand which gives the discriminant
                let (discr, discr_ty) = self.translate_operand_with_type(span, discr)?;

                // Translate the switch targets
                // let targets = targets.
                let targets = self.translate_switch_targets(span, &discr_ty, targets)?;

                TerminatorKind::Switch { discr, targets }
            }
            mir::TerminatorKind::Resume => TerminatorKind::UnwindResume,
            mir::TerminatorKind::Abort => TerminatorKind::Abort(AbortKind::UnwindTerminate),
            mir::TerminatorKind::Return => TerminatorKind::Return,
            // A MIR `Unreachable` terminator indicates undefined behavior of the rust abstract
            // machine.
            mir::TerminatorKind::Unreachable => TerminatorKind::Abort(AbortKind::UndefinedBehavior),
            mir::TerminatorKind::Drop {
                place,
                target,
                unwind, // We consider that panic is an error, and don't model unwinding
            } => 'drop_case: {
                let target = self.translate_basic_block_id(*target);
                let place_ty = place.ty(self.local_decls)?;
                let drop_shim = mir::mono::Instance::resolve_drop_in_place(place_ty);

                if drop_shim.is_empty_shim() {
                    // If the drop shim is empty, we don't need to do anything.
                    break 'drop_case TerminatorKind::Goto { target };
                }

                let place = self.translate_place(span, place)?;

                // need to make a pointer to the local we're dropping
                let ptr_place = self.locals.new_var(
                    None,
                    TyKind::RawPtr(place.ty.clone(), RefKind::Mut).into_ty(),
                );
                let unit_place = self.locals.new_var(None, Ty::mk_unit());
                self.statements.push(Statement::new(
                    span,
                    StatementKind::Assign(
                        ptr_place.clone(),
                        Rvalue::RawPtr {
                            place: place.clone(),
                            kind: RefKind::Mut,
                            ptr_metadata: Operand::mk_const_unit(),
                        },
                    ),
                ));
                let operand = Operand::Move(ptr_place.clone());

                let drop_fn_id = self.register_fun_decl_id(span, drop_shim);
                let on_unwind = self.translate_unwind(span, unwind);
                let fn_ptr = FnPtr {
                    kind: Box::new(FnPtrKind::Fun(FunId::Regular(drop_fn_id))),
                    generics: Box::new(GenericArgs::empty()),
                };
                let func = if matches!(place.ty().kind(), TyKind::DynTrait(_)) {
                    let unit_ptr_ty = TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty();
                    let unit_ptr_ptr_ty = TyKind::RawPtr(unit_ptr_ty, RefKind::Shared).into_ty();
                    let unit_ptr_ptr_ptr_ty =
                        TyKind::RawPtr(unit_ptr_ptr_ty, RefKind::Shared).into_ty();
                    let drop_pointer = ptr_place
                        .project(ProjectionElem::PtrMetadata, unit_ptr_ptr_ptr_ty)
                        .deref();
                    FnOperand::Dynamic(Operand::Copy(drop_pointer))
                } else {
                    FnOperand::Regular(fn_ptr)
                };
                TerminatorKind::Call {
                    call: Call {
                        func,
                        args: vec![operand],
                        dest: unit_place,
                    },
                    target,
                    on_unwind,
                }
            }
            mir::TerminatorKind::Call {
                func,
                args,
                destination,
                target,
                unwind,
            } => self.translate_function_call(span, func, args, destination, target, unwind)?,
            mir::TerminatorKind::Assert {
                cond,
                expected,
                msg,
                target,
                unwind, // We model unwinding as an effet, we don't represent it in control flow
            } => {
                let kind = self.translate_assert_kind(span, msg)?;
                let assert = Assert {
                    cond: self.translate_operand(span, cond)?,
                    expected: *expected,
                    check_kind: Some(kind),
                };
                let target = self.translate_basic_block_id(*target);
                let on_unwind = self.translate_unwind(span, unwind);
                TerminatorKind::Assert {
                    assert,
                    target,
                    on_unwind,
                }
            }
            mir::TerminatorKind::InlineAsm { .. } => {
                raise_error!(self, span, "Inline assembly is not supported");
            }
        };

        // Add the span information
        Ok(Terminator::new(span, t_terminator))
    }

    /// Translate switch targets
    fn translate_switch_targets(
        &mut self,
        span: Span,
        switch_ty: &Ty,
        targets: &mir::SwitchTargets,
    ) -> Result<SwitchTargets, Error> {
        trace!("targets: {:?}", targets);
        let switch_ty = *switch_ty.kind().as_literal().unwrap();
        match switch_ty {
            LiteralTy::Bool => {
                assert_eq!(targets.len(), 2);
                let (val, target) = targets.branches().next().unwrap();
                // It seems the block targets are inverted
                assert_eq!(val, 0u128);
                let if_block = self.translate_basic_block_id(targets.otherwise());
                let else_block = self.translate_basic_block_id(target);
                Ok(SwitchTargets::If(if_block, else_block))
            }
            LiteralTy::Int(int_ty) => {
                let targets_ullbc: Vec<(Literal, BlockId)> = targets
                    .branches()
                    .map(|(v, tgt)| {
                        let v = Literal::Scalar(ScalarValue::from_le_bytes(
                            IntegerTy::Signed(int_ty),
                            v.to_le_bytes(),
                        ));
                        let tgt = self.translate_basic_block_id(tgt);
                        (v, tgt)
                    })
                    .collect();
                let otherwise = self.translate_basic_block_id(targets.otherwise());
                Ok(SwitchTargets::SwitchInt(
                    LiteralTy::Int(int_ty),
                    targets_ullbc,
                    otherwise,
                ))
            }
            LiteralTy::UInt(uint_ty) => {
                let targets_ullbc: Vec<(Literal, BlockId)> = targets
                    .branches()
                    .map(|(v, tgt)| {
                        let v = Literal::Scalar(ScalarValue::from_le_bytes(
                            IntegerTy::Unsigned(uint_ty),
                            v.to_le_bytes(),
                        ));
                        let tgt = self.translate_basic_block_id(tgt);
                        (v, tgt)
                    })
                    .collect();
                let otherwise = self.translate_basic_block_id(targets.otherwise());
                Ok(SwitchTargets::SwitchInt(
                    LiteralTy::UInt(uint_ty),
                    targets_ullbc,
                    otherwise,
                ))
            }
            LiteralTy::Char => {
                let targets_ullbc: Vec<(Literal, BlockId)> = targets
                    .branches()
                    .map(|(v, tgt)| {
                        let b: u128 = u128::from_le_bytes(v.to_le_bytes());
                        let v = Literal::char_from_le_bytes(b);
                        let tgt = self.translate_basic_block_id(tgt);
                        (v, tgt)
                    })
                    .collect();
                let otherwise = self.translate_basic_block_id(targets.otherwise());
                Ok(SwitchTargets::SwitchInt(
                    LiteralTy::Char,
                    targets_ullbc,
                    otherwise,
                ))
            }
            _ => raise_error!(self, span, "Can't match on type {switch_ty}"),
        }
    }

    fn translate_assert_kind(
        &mut self,
        span: Span,
        kind: &mir::AssertMessage,
    ) -> Result<BuiltinAssertKind, Error> {
        match kind {
            mir::AssertMessage::BoundsCheck { len, index } => {
                let len = self.translate_operand(span, len)?;
                let index = self.translate_operand(span, index)?;
                Ok(BuiltinAssertKind::BoundsCheck { len, index })
            }
            mir::AssertMessage::Overflow(binop, left, right) => {
                let binop = self.translate_binaryop_kind(span, *binop)?;
                let left = self.translate_operand(span, left)?;
                let right = self.translate_operand(span, right)?;
                Ok(BuiltinAssertKind::Overflow(binop, left, right))
            }
            mir::AssertMessage::OverflowNeg(operand) => {
                let operand = self.translate_operand(span, operand)?;
                Ok(BuiltinAssertKind::OverflowNeg(operand))
            }
            mir::AssertMessage::DivisionByZero(operand) => {
                let operand = self.translate_operand(span, operand)?;
                Ok(BuiltinAssertKind::DivisionByZero(operand))
            }
            mir::AssertMessage::RemainderByZero(operand) => {
                let operand = self.translate_operand(span, operand)?;
                Ok(BuiltinAssertKind::RemainderByZero(operand))
            }
            mir::AssertMessage::MisalignedPointerDereference { required, found } => {
                let required = self.translate_operand(span, required)?;
                let found = self.translate_operand(span, found)?;
                Ok(BuiltinAssertKind::MisalignedPointerDereference { required, found })
            }
            mir::AssertMessage::NullPointerDereference => {
                Ok(BuiltinAssertKind::NullPointerDereference)
            }
            mir::AssertMessage::InvalidEnumConstruction(operand) => {
                let operand = self.translate_operand(span, operand)?;
                Ok(BuiltinAssertKind::InvalidEnumConstruction(operand))
            }
            mir::AssertMessage::ResumedAfterDrop(..)
            | mir::AssertMessage::ResumedAfterPanic(..)
            | mir::AssertMessage::ResumedAfterReturn(..) => {
                raise_error!(self, span, "Coroutines are not supported");
            }
        }
    }

    fn translate_unwind(&mut self, span: Span, unwind: &mir::UnwindAction) -> BlockId {
        match unwind {
            mir::UnwindAction::Continue => {
                let unwind_continue = Terminator::new(span, TerminatorKind::UnwindResume);
                self.blocks.push(unwind_continue.into_block())
            }
            mir::UnwindAction::Unreachable => {
                let abort =
                    Terminator::new(span, TerminatorKind::Abort(AbortKind::UndefinedBehavior));
                self.blocks.push(abort.into_block())
            }
            mir::UnwindAction::Terminate => {
                let abort =
                    Terminator::new(span, TerminatorKind::Abort(AbortKind::UnwindTerminate));
                self.blocks.push(abort.into_block())
            }
            mir::UnwindAction::Cleanup(bb) => self.translate_basic_block_id(*bb),
        }
    }

    /// Translate a function call statement.
    /// Note that `body` is the body of the function being translated, not of the
    /// function referenced in the function call: we need it in order to translate
    /// the blocks we go to after the function call returns.
    #[allow(clippy::too_many_arguments)]
    fn translate_function_call(
        &mut self,
        span: Span,
        fun: &mir::Operand,
        args: &Vec<mir::Operand>,
        destination: &mir::Place,
        target: &Option<usize>,
        unwind: &mir::UnwindAction,
    ) -> Result<TerminatorKind, Error> {
        // There are two cases, depending on whether this is a "regular"
        // call to a top-level function identified by its id, or if we
        // are using a local function pointer (i.e., the operand is a "move").
        let lval = self.translate_place(span, destination)?;
        let fn_args = self.translate_arguments(span, args)?;
        // Translate the function operand.
        let fn_ty = fun.ty(self.local_decls)?;
        let mut extra_stts: Vec<StatementKind> = vec![];
        let fn_operand = match fn_ty.kind() {
            ty::TyKind::RigidTy(ty::RigidTy::FnDef(fn_def, args)) => {
                let instance = mir::mono::Instance::resolve(fn_def, &args)?;

                let fn_id = self.register_fun_decl_id(span, instance);
                let fn_ptr = FnPtr {
                    kind: Box::new(FnPtrKind::Fun(FunId::Regular(fn_id))),
                    generics: Box::new(GenericArgs::empty()),
                };
                // If this is a VTable call, do something else
                if let mir::mono::InstanceKind::Virtual { idx } = instance.kind {
                    let mut dyn_element_place = match &fn_args[0] {
                        Operand::Copy(place) | Operand::Move(place) => place.clone(),
                        Operand::Const(_) => {
                            panic!("Unexpected constant as receiver for dyn trait method call")
                        }
                    };

                    if matches!(dyn_element_place.ty.kind(), TyKind::DynTrait(_)) {
                        let PlaceKind::Projection(dyn_ref_place, ProjectionElem::Deref) =
                            dyn_element_place.kind
                        else {
                            panic!("dyn variable without a deref?")
                        };
                        dyn_element_place = *dyn_ref_place.clone();
                    }

                    let unit_ptr_ty = TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty();
                    let unit_ptr_ptr_ty =
                        TyKind::RawPtr(unit_ptr_ty.clone(), RefKind::Shared).into_ty();
                    let unit_ptr_ptr_ptr_ty =
                        TyKind::RawPtr(unit_ptr_ptr_ty.clone(), RefKind::Shared).into_ty();
                    let offset = Operand::Const(Box::new(ConstantExpr {
                        kind: ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Unsigned(
                            UIntTy::Usize,
                            idx as u128,
                        ))),
                        ty: Ty::mk_usize(),
                    }));
                    let vtable_ptr =
                        dyn_element_place.project(ProjectionElem::PtrMetadata, unit_ptr_ptr_ptr_ty);
                    let fn_pointer =
                        Rvalue::BinaryOp(BinOp::Offset, Operand::Copy(vtable_ptr), offset);
                    let fn_pointer_place = self.locals.new_var(None, unit_ptr_ptr_ty.clone());
                    extra_stts.push(StatementKind::Assign(fn_pointer_place.clone(), fn_pointer));
                    FnOperand::Dynamic(Operand::Copy(fn_pointer_place.deref()))
                } else {
                    FnOperand::Regular(fn_ptr)
                }
            }
            _ => {
                let fun = self.translate_operand(span, fun)?;
                FnOperand::Dynamic(fun)
            }
        };
        let call = Call {
            func: fn_operand,
            args: fn_args,
            dest: lval,
        };

        let target = match target {
            Some(target) => self.translate_basic_block_id(*target),
            None => {
                let abort =
                    Terminator::new(span, TerminatorKind::Abort(AbortKind::UndefinedBehavior));
                self.blocks.push(abort.into_block())
            }
        };
        let on_unwind = self.translate_unwind(span, unwind);
        self.statements.extend(
            extra_stts
                .into_iter()
                .map(|kind| Statement::new(span, kind)),
        );
        Ok(TerminatorKind::Call {
            call,
            target,
            on_unwind,
        })
    }

    /// Evaluate function arguments in a context, and return the list of computed
    /// values.
    fn translate_arguments(
        &mut self,
        span: Span,
        args: &Vec<mir::Operand>,
    ) -> Result<Vec<Operand>, Error> {
        args.iter()
            .map(|arg| self.translate_operand(span, arg))
            .try_collect()
    }
}
