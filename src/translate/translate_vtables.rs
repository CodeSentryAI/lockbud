extern crate rustc_middle;
extern crate rustc_public;

use super::translate_ctx::*;

use charon_lib::{ast::*, ids::IndexVec, ullbc_ast::*};
use rustc_middle::ty as mty;
use rustc_public::{mir::mono::Instance, ty};

impl<'tcx, 'ctx> ItemTransCtx<'tcx, 'ctx> {
    pub fn translate_vtable_init(
        mut self,
        def_id: FunDeclId,
        item_meta: ItemMeta,
        ty: ty::Ty,
        principal: Option<ty::TraitRef>,
    ) -> Result<FunDecl, Error> {
        let global = self.register_vtable(Span::dummy(), ty, principal.clone());

        let mut statements: Vec<StatementKind> = vec![];
        let mut locals = Locals::new(0);

        let inner_ty = TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty();
        let ret_entries = locals.new_var(Some("vtable".into()), inner_ty.clone());

        let entries = if let Some(trait_ref) = principal {
            let trait_ref = rustc_public::rustc_internal::internal(self.t_ctx.tcx, trait_ref);
            self.t_ctx.tcx.vtable_entries(trait_ref)
        } else {
            mty::TyCtxt::COMMON_VTABLE_ENTRIES
        };

        //
        // We translate the VTable creation into:
        //
        // drop = <const fn ptr> as *const ();
        // size = <const usize> as *const ();
        // align = <const usize> as *const ();
        // entry1 = <fn ptr> as *const ();
        // ...
        // entryN = <fn ptr> as *const ();
        // entry_array = [drop, size, align, entry1, ..., entryN];
        // entry_array_ref = &entry_array as *const [*const (); N];
        // return entry_array_ref as *const ();
        //

        let mut cast_to_unit_ptr = |name: String, op: Operand| -> Operand {
            let local = locals.new_var(Some(name), inner_ty.clone());
            let op_ty = op.ty();
            let cast_kind = if matches!(op_ty.kind(), TyKind::FnDef(..)) {
                CastKind::FnPtr(op_ty.clone(), inner_ty.clone())
            } else {
                // We want a transmute, instead of a RawPtr, as that would imply a int-ptr
                // cast with provenance, which we don't need for size/alignment constants
                // (we're just cheating, really)
                CastKind::Transmute(op_ty.clone(), inner_ty.clone())
            };
            statements.push(StatementKind::Assign(
                local.clone(),
                Rvalue::UnaryOp(UnOp::Cast(cast_kind), op),
            ));
            Operand::Move(local)
        };

        use mty::VtblEntry::*;
        let layout = ty.layout()?.shape();
        let entries: Vec<Operand> =
            entries
                .iter()
                .filter_map(|vtable_entry| match vtable_entry {
                    MetadataDropInPlace => {
                        let drop = Instance::resolve_drop_in_place(ty.clone());
                        let drop_fn = self.register_fun_decl_id(Span::dummy(), drop);
                        let fn_ptr = FnPtr {
                            kind: Box::new(FnPtrKind::Fun(FunId::Regular(drop_fn))),
                            generics: Box::new(GenericArgs::empty()),
                        };
                        Some(cast_to_unit_ptr(
                            "drop".into(),
                            Operand::Const(Box::new(ConstantExpr {
                                ty: TyKind::FnDef(RegionBinder::empty(fn_ptr.clone())).into_ty(),
                                kind: ConstantExprKind::FnDef(fn_ptr),
                            })),
                        ))
                    }
                    MetadataSize => Some(cast_to_unit_ptr(
                        "size".into(),
                        Operand::Const(Box::new(ConstantExpr {
                            ty: Ty::mk_usize(),
                            kind: ConstantExprKind::Literal(Literal::Scalar(
                                ScalarValue::Unsigned(UIntTy::Usize, layout.size.bytes() as u128),
                            )),
                        })),
                    )),
                    MetadataAlign => Some(cast_to_unit_ptr(
                        "align".into(),
                        Operand::Const(Box::new(ConstantExpr {
                            ty: Ty::mk_usize(),
                            kind: ConstantExprKind::Literal(Literal::Scalar(
                                ScalarValue::Unsigned(UIntTy::Usize, layout.abi_align as u128),
                            )),
                        })),
                    )),
                    Method(instance) => {
                        let instance = rustc_public::rustc_internal::stable(instance);
                        let name = instance.trimmed_name();
                        let fun = self.register_fun_decl_id(Span::dummy(), instance);
                        let fn_ptr = FnPtr {
                            kind: Box::new(FnPtrKind::Fun(FunId::Regular(fun))),
                            generics: Box::new(GenericArgs::empty()),
                        };
                        Some(cast_to_unit_ptr(
                            name,
                            Operand::Const(Box::new(ConstantExpr {
                                ty: TyKind::FnDef(RegionBinder::empty(fn_ptr.clone())).into_ty(),
                                kind: ConstantExprKind::FnDef(fn_ptr),
                            })),
                        ))
                    }
                    TraitVPtr(super_trait) => {
                        let super_trait = rustc_public::rustc_internal::stable(super_trait);
                        let vtable = self.register_vtable(Span::dummy(), ty, Some(super_trait));
                        Some(Operand::Copy(Place {
                            kind: PlaceKind::Global(GlobalDeclRef {
                                id: vtable,
                                generics: Box::new(GenericArgs::empty()),
                            }),
                            ty: inner_ty.clone(),
                        }))
                    }
                    Vacant => None,
                })
                .collect();

        let entry_count =
            ConstantExpr::mk_usize(ScalarValue::Unsigned(UIntTy::Usize, entries.len() as u128));
        let entry_array_ty =
            TyKind::Array(inner_ty.clone(), Box::new(entry_count.clone())).into_ty();
        let entry_array = locals.new_var(Some("entry_array".into()), entry_array_ty.clone());
        statements.push(StatementKind::Assign(
            entry_array.clone(),
            Rvalue::Aggregate(
                AggregateKind::Array(inner_ty.clone(), Box::new(entry_count.clone())),
                entries,
            ),
        ));

        let entry_array_ref_ty = TyKind::RawPtr(entry_array_ty.clone(), RefKind::Shared).into_ty();
        let entry_array_ref =
            locals.new_var(Some("entry_array_ref".into()), entry_array_ref_ty.clone());
        statements.push(StatementKind::Assign(
            entry_array_ref.clone(),
            Rvalue::RawPtr {
                place: entry_array,
                kind: RefKind::Shared,
                ptr_metadata: Operand::mk_const_unit(),
            },
        ));

        statements.push(StatementKind::Assign(
            ret_entries,
            Rvalue::UnaryOp(
                UnOp::Cast(CastKind::RawPtr(entry_array_ref_ty, inner_ty)),
                Operand::Move(entry_array_ref),
            ),
        ));

        let body = Body::Unstructured(ExprBody {
            body: IndexVec::from_vec(vec![BlockData {
                statements: statements
                    .into_iter()
                    .map(|k| Statement {
                        span: Span::dummy(),
                        comments_before: vec![],
                        kind: k,
                    })
                    .collect(),
                terminator: Terminator {
                    span: Span::dummy(),
                    comments_before: vec![],
                    kind: TerminatorKind::Return,
                },
            }]),
            bound_body_regions: 0,
            locals,
            span: Span::dummy(),
            comments: vec![],
        });

        Ok(FunDecl {
            def_id,
            item_meta,
            signature: FunSig {
                is_unsafe: false,
                inputs: vec![],
                output: TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty(),
            },
            generics: GenericParams::empty(),
            src: ItemSource::TopLevel,
            is_global_initializer: Some(global),
            body,
        })
    }

    pub fn translate_vtable(
        mut self,
        def_id: GlobalDeclId,
        item_meta: ItemMeta,
        from_ty: ty::Ty,
        principal: Option<ty::TraitRef>,
    ) -> Result<GlobalDecl, Error> {
        let init = self.register_vtable_init(Span::dummy(), from_ty, principal);
        Ok(GlobalDecl {
            def_id,
            item_meta,
            generics: GenericParams::empty(),
            global_kind: GlobalKind::Static,
            src: ItemSource::TopLevel,
            ty: TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty(),
            init,
        })
    }
}
