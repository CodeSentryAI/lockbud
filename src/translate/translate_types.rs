extern crate rustc_abi;
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_public_bridge;
extern crate rustc_span;

use crate::translate::translate_crate::FAKE_DYN_TRAIT;

use super::translate_ctx::*;
use charon_lib::ast::*;
use charon_lib::ids::IndexVec;
use charon_lib::{raise_error, register_error};
use core::convert::*;
use log::trace;
use rustc_middle::ty as rustc_ty;
use rustc_public::{mir, ty};

impl<'tcx, 'ctx> ItemTransCtx<'tcx, 'ctx> {
    // Translate a region
    pub(crate) fn translate_region(
        &mut self,
        span: Span,
        region: &ty::Region,
    ) -> Result<Region, Error> {
        use ty::RegionKind::*;
        match &region.kind {
            ReErased => Ok(Region::Erased),
            ReStatic => Ok(Region::Static),
            ReBound(..) | ReEarlyParam(..) => Ok(Region::Erased),
            RePlaceholder(..) => {
                // Shouldn't exist outside of type inference.
                raise_error!(
                    self,
                    span,
                    "Should not exist outside of type inference: {region:?}"
                )
            }
        }
    }

    pub(crate) fn translate_int_ty(&mut self, it: &ty::IntTy) -> IntTy {
        match it {
            ty::IntTy::I8 => IntTy::I8,
            ty::IntTy::I16 => IntTy::I16,
            ty::IntTy::I32 => IntTy::I32,
            ty::IntTy::I64 => IntTy::I64,
            ty::IntTy::I128 => IntTy::I128,
            ty::IntTy::Isize => IntTy::Isize,
        }
    }

    pub(crate) fn translate_uint_ty(&mut self, it: &ty::UintTy) -> UIntTy {
        match it {
            ty::UintTy::U8 => UIntTy::U8,
            ty::UintTy::U16 => UIntTy::U16,
            ty::UintTy::U32 => UIntTy::U32,
            ty::UintTy::U64 => UIntTy::U64,
            ty::UintTy::U128 => UIntTy::U128,
            ty::UintTy::Usize => UIntTy::Usize,
        }
    }

    pub(crate) fn dummy_trait_decl_ref(&self) -> TraitDeclRef {
        TraitDeclRef {
            id: FAKE_DYN_TRAIT,
            generics: Box::new(GenericArgs::empty()),
        }
    }

    pub(crate) fn dummy_trait_impl_ref(&self) -> TraitImplRef {
        TraitImplRef {
            id: TraitImplId::ZERO,
            generics: Box::new(GenericArgs::empty()),
        }
    }

    pub(crate) fn dummy_trait_ref(&self) -> TraitRef {
        TraitRef::new(
            TraitRefKind::Dyn,
            RegionBinder::empty(self.dummy_trait_decl_ref()),
        )
    }

    pub(crate) fn dummy_dyn_ty(&self) -> TyKind {
        TyKind::DynTrait(DynPredicate {
            binder: Binder {
                params: GenericParams {
                    regions: vec![].into(),
                    types: vec![TypeParam::new(TypeVarId::ZERO, "dyn".into())].into(),
                    const_generics: vec![].into(),
                    trait_clauses: vec![TraitParam {
                        clause_id: TraitClauseId::ZERO,
                        span: None,
                        origin: PredicateOrigin::Dyn,
                        trait_: RegionBinder::empty(self.dummy_trait_decl_ref()),
                    }]
                    .into(),
                    regions_outlive: vec![].into(),
                    types_outlive: vec![].into(),
                    trait_type_constraints: vec![].into(),
                },
                skip_binder: TyKind::TypeVar(DeBruijnVar::bound(
                    DeBruijnId::zero(),
                    TypeVarId::ZERO,
                ))
                .into_ty(),
                kind: BinderKind::Dyn,
            },
        })
    }

    pub fn maybe_uninit_bytes(&mut self, span: Span, len: usize) -> Result<Ty, Error> {
        let maybe_uninit = self
            .t_ctx
            .tcx
            .require_lang_item(rustc_hir::LangItem::MaybeUninit, rustc_span::DUMMY_SP);
        let u8_ty = ty::Ty::from_rigid_kind(ty::RigidTy::Uint(ty::UintTy::U8));
        let len_cg = ty::TyConst::try_from_target_usize(len as u64)?;
        let array_ty = ty::Ty::new_array_with_const_len(u8_ty, len_cg);
        let array_ty = rustc_public::rustc_internal::internal(self.t_ctx.tcx, array_ty);
        let maybe_uninit = self
            .t_ctx
            .tcx
            .type_of(maybe_uninit)
            .instantiate(self.t_ctx.tcx, &[array_ty.into()]);
        let maybe_uninit: ty::Ty = rustc_public::rustc_internal::stable(maybe_uninit);
        self.translate_ty(span, maybe_uninit)
    }

    /// Translate a Ty.
    ///
    /// Typically used in this module to translate the fields of a structure/
    /// enumeration definition, or later to translate the type of a variable.
    ///
    /// Note that we take as parameter a function to translate regions, because
    /// regions can be translated in several manners (non-erased region or erased
    /// regions), in which case the return type is different.
    pub(crate) fn translate_ty(&mut self, span: Span, mir_ty: ty::Ty) -> Result<Ty, Error> {
        trace!("{:?}", mir_ty);
        if let Some(ty) = self.t_ctx.type_trans_cache.get(&mir_ty).cloned() {
            return Ok(ty.clone());
        }

        let kind = mir_ty.kind();
        let Some(ty) = kind.rigid() else {
            match kind {
                ty::TyKind::Alias(..) => raise_error!(
                    self,
                    span,
                    "translate_ty got an alias: {:?}\nTrace: {}",
                    kind,
                    std::backtrace::Backtrace::force_capture()
                ),
                ty::TyKind::Bound(..) => raise_error!(
                    self,
                    span,
                    "translate_ty got a bound variable: {:?}\nTrace: {}",
                    kind,
                    std::backtrace::Backtrace::force_capture()
                ),
                ty::TyKind::Param(ty) => {
                    let id = TypeVarId::from_raw(ty.index as usize);
                    let ty = TyKind::TypeVar(TypeDbVar::Free(id)).into_ty();
                    self.t_ctx.type_trans_cache.insert(mir_ty, ty.clone());
                    register_error!(self, span, "translate_ty got a type parameter: {:?}", ty);
                    return Ok(ty);
                }
                ty::TyKind::RigidTy(..) => unreachable!(),
            }
        };

        let kind = match ty {
            ty::RigidTy::Bool => TyKind::Literal(LiteralTy::Bool),
            ty::RigidTy::Char => TyKind::Literal(LiteralTy::Char),
            ty::RigidTy::Int(int_ty) => {
                TyKind::Literal(LiteralTy::Int(self.translate_int_ty(int_ty)))
            }
            ty::RigidTy::Uint(int_ty) => {
                TyKind::Literal(LiteralTy::UInt(self.translate_uint_ty(int_ty)))
            }
            ty::RigidTy::Float(float_ty) => {
                use ty::FloatTy;
                TyKind::Literal(LiteralTy::Float(match float_ty {
                    FloatTy::F16 => charon_lib::ast::types::FloatTy::F16,
                    FloatTy::F32 => charon_lib::ast::types::FloatTy::F32,
                    FloatTy::F64 => charon_lib::ast::types::FloatTy::F64,
                    FloatTy::F128 => charon_lib::ast::types::FloatTy::F128,
                }))
            }
            ty::RigidTy::Never => TyKind::Never,

            ty::RigidTy::Adt(item, generics) => {
                // FIXME: generics?
                trace!("Adt: {:?}", item.0);
                let id = self.register_type_decl_id(span, *item, generics.clone());
                // no generics since it's monomorphic
                let tref = TypeDeclRef {
                    id: TypeId::Adt(id),
                    generics: Box::new(GenericArgs::empty()),
                };

                // Return the instantiated ADT
                TyKind::Adt(tref)
            }
            ty::RigidTy::Str => {
                let tref = TypeDeclRef::new(TypeId::Builtin(BuiltinTy::Str), GenericArgs::empty());
                TyKind::Adt(tref)
            }
            ty::RigidTy::Array(ty, const_param) => {
                let c = self.translate_tyconst_to_const_expr(span, const_param)?;
                let ty = self.translate_ty(span, *ty)?;
                TyKind::Array(ty, Box::new(c))
            }
            ty::RigidTy::Slice(ty) => {
                let ty = self.translate_ty(span, *ty)?;
                TyKind::Slice(ty)
            }
            ty::RigidTy::Ref(region, ty, mutability) => {
                trace!("Ref");

                let region = self.translate_region(span, region)?;
                let ty = self.translate_ty(span, *ty)?;
                let kind = RefKind::mutable(matches!(mutability, mir::Mutability::Mut));
                TyKind::Ref(region, ty, kind)
            }
            ty::RigidTy::RawPtr(ty, mutability) => {
                trace!("RawPtr: {:?}", (ty, mutability));
                let ty = self.translate_ty(span, *ty)?;
                let kind = RefKind::mutable(matches!(mutability, mir::Mutability::Mut));
                TyKind::RawPtr(ty, kind)
            }
            ty::RigidTy::Tuple(substs) => {
                let params = substs
                    .iter()
                    .map(|ty| self.translate_ty(span, *ty))
                    .try_collect()?;
                let tref = TypeDeclRef::new(TypeId::Tuple, GenericArgs::new_for_builtin(params));
                TyKind::Adt(tref)
            }

            ty::RigidTy::Foreign(fdef) => {
                let type_id = self.register_foreign_type_decl_id(span, *fdef);
                TyKind::Adt(TypeDeclRef {
                    id: TypeId::Adt(type_id),
                    generics: Box::new(GenericArgs::empty()),
                })
            }

            ty::RigidTy::FnPtr(bsig) => {
                trace!("Arrow");
                trace!("bound vars: {:?}", bsig.bound_vars);
                let sig = &bsig.value;
                let inputs = sig
                    .inputs()
                    .iter()
                    .map(|x| self.translate_ty(span, *x))
                    .try_collect()?;
                let output = self.translate_ty(span, sig.output())?;

                // let sig =
                //     RegionBinder::
                //     self.translate_region_binder(span, sig, |ctx, sig| {
                //     let inputs = sig
                //         .inputs
                //         .iter()
                //         .map(|x| ctx.translate_ty(span, x))
                //         .try_collect()?;
                //     let output = ctx.translate_ty(span, &sig.output)?;
                //     Ok((inputs, output))
                // })?;
                TyKind::FnPtr(RegionBinder::empty(FunSig {
                    inputs,
                    output,
                    is_unsafe: false,
                }))
            }
            ty::RigidTy::FnDef(item, args) => {
                let instance = mir::mono::Instance::resolve(*item, args)?;
                let fn_id = self.register_fun_decl_id(span, instance);
                let fnref = RegionBinder::empty(FnPtr {
                    kind: Box::new(FnPtrKind::Fun(FunId::Regular(fn_id))),
                    generics: Box::new(GenericArgs::empty()),
                });
                TyKind::FnDef(fnref)
            }
            ty::RigidTy::Closure(def, gargs) => {
                let id = self.register_closure_type_decl_id(span, *def, gargs.clone());
                let tref = TypeDeclRef {
                    id: TypeId::Adt(id),
                    generics: Box::new(GenericArgs::empty()),
                };

                // Return the instantiated ADT
                TyKind::Adt(tref)
            }

            ty::RigidTy::Dynamic(_existential_preds, _region) => {
                // TODO: we don't translate the predicates yet because our machinery can't handle
                // it.
                self.dummy_dyn_ty()
            }

            ty::RigidTy::Coroutine(..) => {
                raise_error!(self, span, "Coroutine types are not supported yet")
            }
            ty::RigidTy::Pat(..) => {
                raise_error!(self, span, "Pat types are not supported yet")
            }
            ty::RigidTy::CoroutineClosure(..) => {
                raise_error!(self, span, "Coroutine closure types are not supported yet")
            }
            ty::RigidTy::CoroutineWitness(..) => {
                raise_error!(self, span, "Coroutine witness types are not supported yet")
            }
        };
        let ty = kind.into_ty();
        self.t_ctx.type_trans_cache.insert(mir_ty, ty.clone());
        Ok(ty)
    }

    /// Translate generic args. Don't call directly; use `translate_xxx_ref` as much as possible.
    // pub fn translate_generic_args(
    //     &mut self,
    //     span: Span,
    //     substs: &[ty::GenericArgKind],
    // ) -> Result<GenericArgs, Error> {
    //     use ty::GenericArgKind::*;
    //     trace!("{:?}", substs);

    //     let mut regions = Vector::new();
    //     let mut types = Vector::new();
    //     let mut const_generics = Vector::new();
    //     for param in substs {
    //         match param {
    //             Type(param_ty) => {
    //                 types.push(self.translate_ty(span, *param_ty)?);
    //             }
    //             Lifetime(region) => {
    //                 regions.push(self.translate_region(span, region)?);
    //             }
    //             Const(c) => {
    //                 const_generics.push(self.translate_tyconst_to_const_generic(span, c)?);
    //             }
    //         }
    //     }

    //     Ok(GenericArgs {
    //         regions,
    //         types,
    //         const_generics,
    //         trait_refs: vec![].into(),
    //     })
    // }

    /// Translate a Dynamically Sized Type metadata kind.
    ///
    /// Returns `None` if the type is generic, or if it is not a DST.
    pub fn translate_ptr_metadata(&self) -> PtrMetadata {
        PtrMetadata::None
        // // prepare the call to the method
        // use rustc_middle::ty;
        // let tcx = self.t_ctx.tcx;
        // let rdefid = self.def_id.as_rust_def_id().unwrap();
        // let ty_env = mir::State::new_from_state_and_id(&self.t_ctx.hax_state, rdefid).typing_env();
        // // This `skip_binder` is ok because it's an `EarlyBinder`.
        // let ty = tcx.type_of(rdefid).skip_binder();

        // // call the key method
        // match tcx
        //     .struct_tail_raw(
        //         ty,
        //         |ty| tcx.try_normalize_erasing_regions(ty_env, ty).unwrap_or(ty),
        //         || {},
        //     )
        //     .kind()
        // {
        //     ty::Foreign(..) => Some(PtrMetadata::None),
        //     ty::Str | ty::Slice(..) => Some(PtrMetadata::Length),
        //     ty::Dynamic(..) => Some(PtrMetadata::VTable(VTable)),
        //     // This is NOT accurate -- if there is no generic clause that states `?Sized`
        //     // Then it will be safe to return `Some(PtrMetadata::None)`.
        //     // TODO: inquire the generic clause to get the accurate info.
        //     ty::Placeholder(..) | ty::Infer(..) | ty::Param(..) | ty::Bound(..) => None,
        //     _ => Some(PtrMetadata::None),
        // }
    }

    /// Translate a type layout.
    ///
    /// Translates the layout as queried from rustc into
    /// the more restricted [`Layout`].
    pub fn translate_layout(
        &mut self,
        def: &ty::AdtDef,
        genargs: &ty::GenericArgs,
    ) -> Option<Layout> {
        use rustc_abi as r_abi;

        fn translate_variant_layout(
            variant_layout: &r_abi::LayoutData<r_abi::FieldIdx, r_abi::VariantIdx>,
            tag: Option<ScalarValue>,
        ) -> VariantLayout {
            let field_offsets = match &variant_layout.fields {
                r_abi::FieldsShape::Arbitrary { offsets, .. } => {
                    offsets.iter().map(|o| o.bytes()).collect()
                }
                r_abi::FieldsShape::Union(x) => IndexVec::from_vec(vec![0].repeat(x.get())),
                r_abi::FieldsShape::Primitive => IndexVec::new(),
                r_abi::FieldsShape::Array { .. } => panic!("Unexpected layout shape"),
            };
            VariantLayout {
                field_offsets,
                uninhabited: variant_layout.is_uninhabited(),
                tag,
            }
        }

        fn translate_primitive_int(int_ty: r_abi::Integer, signed: bool) -> IntegerTy {
            if signed {
                IntegerTy::Signed(match int_ty {
                    r_abi::Integer::I8 => IntTy::I8,
                    r_abi::Integer::I16 => IntTy::I16,
                    r_abi::Integer::I32 => IntTy::I32,
                    r_abi::Integer::I64 => IntTy::I64,
                    r_abi::Integer::I128 => IntTy::I128,
                })
            } else {
                IntegerTy::Unsigned(match int_ty {
                    r_abi::Integer::I8 => UIntTy::U8,
                    r_abi::Integer::I16 => UIntTy::U16,
                    r_abi::Integer::I32 => UIntTy::U32,
                    r_abi::Integer::I64 => UIntTy::U64,
                    r_abi::Integer::I128 => UIntTy::U128,
                })
            }
        }

        // If layout computation returns an error, we return `None`.
        let ty = def.ty_with_args(genargs);
        let layout = match ty.layout() {
            Ok(layout) => layout,
            Err(e) => {
                register_error!(
                    self,
                    Span::dummy(),
                    "Failed to compute layout for type `{ty:?}`, error: {e:?}"
                );
                return None;
            }
        };
        let layout = rustc_public::rustc_internal::internal(self.t_ctx.tcx, layout);
        let ty = rustc_public::rustc_internal::internal(self.t_ctx.tcx, ty);

        let (size, align) = (
            Some(layout.size().bytes()),
            Some(layout.align().abi.bytes()),
        );

        // Get the layout of the discriminant when there is one (even if it is encoded in a niche).
        let discriminant_layout = match layout.variants() {
            r_abi::Variants::Multiple {
                tag,
                tag_encoding,
                tag_field,
                ..
            } => {
                // The tag_field is the index into the `offsets` vector.
                let r_abi::FieldsShape::Arbitrary { offsets, .. } = layout.fields() else {
                    unreachable!()
                };

                let tag_ty = match tag.primitive() {
                    r_abi::Primitive::Int(int_ty, signed) => {
                        translate_primitive_int(int_ty, signed)
                    }
                    // Try to handle pointer as integers of the same size.
                    r_abi::Primitive::Pointer(_) => IntegerTy::Signed(IntTy::Isize),
                    r_abi::Primitive::Float(_) => {
                        unreachable!()
                    }
                };

                let encoding = match tag_encoding {
                    r_abi::TagEncoding::Direct => TagEncoding::Direct,
                    r_abi::TagEncoding::Niche {
                        untagged_variant, ..
                    } => TagEncoding::Niche {
                        untagged_variant: VariantId::from_usize(r_abi::VariantIdx::as_usize(
                            *untagged_variant,
                        )),
                    },
                };
                offsets.get(*tag_field).map(|s| DiscriminantLayout {
                    offset: r_abi::Size::bytes(*s),
                    tag_ty,
                    encoding,
                })
            }
            r_abi::Variants::Single { .. } | r_abi::Variants::Empty => None,
        };

        let mut variant_layouts: IndexVec<VariantId, VariantLayout> = IndexVec::new();

        match layout.variants() {
            r_abi::Variants::Multiple { variants, .. } => {
                let tag_ty = discriminant_layout
                    .as_ref()
                    .expect("No discriminant layout for enum?")
                    .tag_ty;
                let ptr_size = self.t_ctx.translated.target_information.target_pointer_size;
                let tag_size = r_abi::Size::from_bytes(tag_ty.target_size(ptr_size));

                for (id, variant_layout) in variants.iter_enumerated() {
                    let tag = if variant_layout.is_uninhabited() {
                        None
                    } else {
                        let ty_env = rustc_ty::TypingEnv {
                            param_env: rustc_ty::ParamEnv::empty(),
                            typing_mode: rustc_ty::TypingMode::PostAnalysis,
                        };
                        self.t_ctx
                            .tcx
                            .tag_for_variant(ty_env.as_query_input((ty, id)))
                            .map(|s| match tag_ty {
                                IntegerTy::Signed(int_ty) => {
                                    ScalarValue::from_int(ptr_size, int_ty, s.to_int(tag_size))
                                        .unwrap()
                                }
                                IntegerTy::Unsigned(uint_ty) => {
                                    ScalarValue::from_uint(ptr_size, uint_ty, s.to_uint(tag_size))
                                        .unwrap()
                                }
                            })
                    };
                    variant_layouts.push(translate_variant_layout(variant_layout, tag));
                }
            }
            r_abi::Variants::Single { index } => {
                match layout.fields() {
                    r_abi::FieldsShape::Arbitrary { .. } => {
                        let n_variants = match ty.kind() {
                            _ if let Some(range) = ty.variant_range(self.t_ctx.tcx) => {
                                range.end.index()
                            }
                            _ => 1,
                        };
                        // All the variants not initialized below are uninhabited.
                        variant_layouts = (0..n_variants)
                            .map(|_| VariantLayout {
                                field_offsets: IndexVec::new(),
                                uninhabited: true,
                                tag: None,
                            })
                            .collect();
                        variant_layouts[index.index()] = translate_variant_layout(&layout, None);
                    }
                    r_abi::FieldsShape::Union { .. } => {
                        variant_layouts.push(translate_variant_layout(&layout, None));
                    }
                    _ => {}
                }
            }
            r_abi::Variants::Empty => {}
        }

        Some(Layout {
            size,
            align,
            discriminant_layout,
            uninhabited: layout.is_uninhabited(),
            variant_layouts,
        })
    }

    /// Translate the body of a type declaration.
    ///
    /// Note that the type may be external, in which case we translate the body
    /// only if it is public (i.e., it is a public enumeration, or it is a
    /// struct with only public fields).
    pub(crate) fn translate_adt_def(
        &mut self,
        trans_id: TypeDeclId,
        def_span: Span,
        item_meta: &ItemMeta,
        adt: &ty::AdtDef,
        generics: &ty::GenericArgs,
    ) -> Result<TypeDeclKind, Error> {
        use ty::AdtKind;

        if item_meta.opacity.is_opaque() {
            return Ok(TypeDeclKind::Opaque);
        }

        trace!("{}", trans_id);

        // In case the type is external, check if we should consider the type as
        // transparent (i.e., extract its body). If it is an enumeration, then yes
        // (because the variants of public enumerations are public, together with their
        // fields). If it is a structure, we check if all the fields are public.
        let contents_are_public = match adt.kind() {
            AdtKind::Enum => true,
            AdtKind::Struct | AdtKind::Union => {
                true
                // // Check the unique variant
                // error_assert!(self, def_span, adt.variants.len() == 1);
                // adt.variants[0]
                //     .fields
                //     .iter()
                //     .all(|f| matches!(f.vis, Visibility::Public))
            }
        };

        if item_meta
            .opacity
            .with_content_visibility(contents_are_public)
            .is_opaque()
        {
            return Ok(TypeDeclKind::Opaque);
        }

        // The type is transparent: explore the variants
        let mut variants: IndexVec<VariantId, Variant> = Default::default();
        for (i, var_def) in adt.variants_iter().enumerate() {
            trace!("variant {i}: {var_def:?}");

            let mut fields: IndexVec<FieldId, Field> = Default::default();
            for (j, field_def) in var_def.fields().iter().enumerate() {
                trace!("variant {i}: field {j}: {field_def:?}");
                // Translate the field type
                let ty = field_def.ty_with_args(&generics);
                let ty = self.translate_ty(def_span, ty)?;
                // let field_full_def = self.hax_def(&field_def.did)?;
                // let field_attrs = self.t_ctx.translate_attr_info(&field_full_def);

                // Retrieve the field name.
                let field_name = field_def.name.clone();

                // Store the field
                let field = Field {
                    span: def_span,
                    attr_info: AttrInfo::default(),
                    name: Some(field_name),
                    ty,
                };
                fields.push(field);
            }

            let variant_name = var_def.name().clone();
            let discriminant = if adt.kind().is_enum() {
                let discr = adt.discriminant_for_variant(var_def.idx);

                let ty = self.translate_ty(def_span, discr.ty)?;
                let lit_ty = ty.kind().as_literal().unwrap();
                match Literal::from_bits(lit_ty, discr.val) {
                    Some(lit) => lit,
                    None => raise_error!(self, def_span, "unexpected discriminant type: {ty:?}",),
                }
            } else {
                Literal::Scalar(ScalarValue::Unsigned(UIntTy::U8, 0))
            };

            let mut variant = Variant {
                span: def_span,
                attr_info: AttrInfo::default(),
                name: variant_name,
                fields,
                discriminant,
            };
            // Propagate a `#[charon::variants_prefix(..)]` or `#[charon::variants_suffix(..)]` attribute to the variants.
            if variant.attr_info.rename.is_none() {
                let prefix = item_meta
                    .attr_info
                    .attributes
                    .iter()
                    .filter_map(|a| a.as_variants_prefix())
                    .next()
                    .map(|attr| attr.as_str());
                let suffix = item_meta
                    .attr_info
                    .attributes
                    .iter()
                    .filter_map(|a| a.as_variants_suffix())
                    .next()
                    .map(|attr| attr.as_str());
                if prefix.is_some() || suffix.is_some() {
                    let prefix = prefix.unwrap_or_default();
                    let suffix = suffix.unwrap_or_default();
                    let name = &variant.name;
                    variant.attr_info.rename = Some(format!("{prefix}{name}{suffix}"));
                }
            }
            variants.push(variant);
        }

        // Register the type
        let type_def_kind: TypeDeclKind = match adt.kind() {
            AdtKind::Struct => TypeDeclKind::Struct(variants[0].fields.clone()),
            AdtKind::Enum => TypeDeclKind::Enum(variants),
            AdtKind::Union => TypeDeclKind::Union(variants[0].fields.clone()),
        };

        Ok(type_def_kind)
    }

    pub fn translate_generic_args(
        &mut self,
        span: Span,
        mir_generics: &ty::GenericArgs,
    ) -> Result<GenericArgs, Error> {
        let mut generics = GenericArgs::empty();
        mir_generics
            .0
            .iter()
            .try_for_each(|kind| -> Result<(), Error> {
                match kind {
                    ty::GenericArgKind::Type(ty) => {
                        let ty = self.translate_ty(span, *ty)?;
                        generics.types.push(ty);
                        Ok(())
                    }
                    ty::GenericArgKind::Const(c) => {
                        let c = self.translate_tyconst_to_const_expr(span, c)?;
                        generics.const_generics.push(c);
                        Ok(())
                    }
                    ty::GenericArgKind::Lifetime(region) => {
                        let r = self.translate_region(span, region)?;
                        generics.regions.push(r);
                        Ok(())
                    }
                }
            })?;
        Ok(generics)
    }
}
