extern crate rustc_abi;
extern crate rustc_apfloat;
extern crate rustc_public;
extern crate rustc_public_bridge;

use charon_lib::{ast::*, raise_error, register_error};
use itertools::Itertools;
use rustc_apfloat::{Float, ieee};
use rustc_middle::mir::interpret::PointerArithmetic;
use rustc_public::{abi, mir, ty};
use rustc_public_bridge::IndexedVal;
use std::io::Read;

use crate::translate::translate_ctx::ItemTransCtx;

impl<'tcx, 'ctx> ItemTransCtx<'tcx, 'ctx> {
    /// Utility function used to read an allocation data into a unassigned integer.
    fn read_target_uint(&mut self, mut bytes: &[u8]) -> Result<u128, Error> {
        let mut buf = [0u8; size_of::<u128>()];
        match self.t_ctx.tcx.data_layout.endian {
            rustc_abi::Endian::Little => {
                bytes.read_exact(&mut buf[..bytes.len()])?;
                Ok(u128::from_le_bytes(buf))
            }
            rustc_abi::Endian::Big => {
                bytes.read_exact(&mut buf[16 - bytes.len()..])?;
                Ok(u128::from_be_bytes(buf))
            }
        }
    }

    /// Utility function used to read an allocation data into an assigned integer.
    fn read_target_int(&mut self, mut bytes: &[u8]) -> Result<i128, Error> {
        // we do sign extension manually because why not i guess.
        match self.t_ctx.tcx.data_layout.endian {
            rustc_abi::Endian::Little => {
                let sign = bytes.last().copied().unwrap_or(0) & 0x80 != 0;
                let default = if sign { 0xff } else { 0x00 };
                let mut buf = [default; size_of::<i128>()];
                bytes.read_exact(&mut buf[..bytes.len()])?;
                Ok(i128::from_le_bytes(buf))
            }
            rustc_abi::Endian::Big => {
                let sign = bytes.first().copied().unwrap_or(0) & 0x80 != 0;
                let default = if sign { 0xff } else { 0x00 };
                let mut buf = [default; size_of::<i128>()];
                bytes.read_exact(&mut buf[16 - bytes.len()..])?;
                Ok(i128::from_be_bytes(buf))
            }
        }
    }

    pub fn as_init(bytes: &[Option<u8>]) -> Result<Vec<u8>, Error> {
        bytes
            .iter()
            .copied()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| format!("Found uninitialized bytes when reading {bytes:?}").into())
    }

    pub fn as_charon_bytes(
        &mut self,
        span: Span,
        alloc: &ty::Allocation,
        offset: usize,
        size: usize,
    ) -> Vec<Byte> {
        let mut bytes: Vec<Byte> = alloc.bytes.as_slice()[offset..offset + size]
            .iter()
            .map(|b| match b {
                None => Byte::Uninit,
                Some(v) => Byte::Value(*v),
            })
            .collect();

        let ptr_size = self.t_ctx.tcx.pointer_size().bytes_usize();
        for (prov_ofs, prov) in alloc.provenance.ptrs.iter() {
            let prov_alloc: mir::alloc::GlobalAlloc = prov.0.into();
            let prov = match prov_alloc {
                mir::alloc::GlobalAlloc::Function(fun) => {
                    let id = self.register_fun_decl_id(span, fun);
                    Provenance::Function(FunDeclRef {
                        id,
                        generics: Box::new(GenericArgs::empty()),
                    })
                }
                _ => {
                    let id = self.register_global_decl_id(span, prov.0, None);
                    Provenance::Global(GlobalDeclRef {
                        id,
                        generics: Box::new(GenericArgs::empty()),
                    })
                }
            };
            let prov_ofs = *prov_ofs;
            for ptr_i in 0..ptr_size {
                let alloc_i = prov_ofs + ptr_i;
                if offset <= alloc_i && alloc_i < offset + size {
                    bytes[alloc_i] = Byte::Provenance(prov.clone(), ptr_i as u8);
                }
            }
        }

        bytes
    }

    fn provenance_at<'a>(
        &mut self,
        alloc: &'a ty::Allocation,
        offset: usize,
    ) -> Option<&'a ty::Prov> {
        alloc
            .provenance
            .ptrs
            .iter()
            .find(|(o, _)| *o == offset)
            .map(|(_, p)| p)
    }

    pub fn translate_allocation(
        &mut self,
        span: Span,
        alloc: &ty::Allocation,
        ty: &TyKind,
        rty: ty::Ty,
    ) -> Result<ConstantExpr, Error> {
        self.translate_allocation_at(span, alloc, ty, rty, 0)
    }

    pub fn translate_allocation_at(
        &mut self,
        span: Span,
        alloc: &ty::Allocation,
        ty: &TyKind,
        rty: ty::Ty,
        offset: usize,
    ) -> Result<ConstantExpr, Error> {
        let size = rty.layout()?.shape().size.bytes();
        if size == 0 {
            return self.translate_zst_constant(span, ty, rty);
        }
        let bytes = &alloc.bytes.as_slice()[offset..offset + size];
        let kind = match ty {
            TyKind::Literal(lit) => match lit {
                LiteralTy::Int(it) => {
                    ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Signed(
                        it.clone(),
                        self.read_target_int(Self::as_init(bytes)?.as_slice())?,
                    )))
                }
                LiteralTy::UInt(uit) => {
                    ConstantExprKind::Literal(Literal::Scalar(ScalarValue::Unsigned(
                        uit.clone(),
                        self.read_target_uint(Self::as_init(bytes)?.as_slice())?,
                    )))
                }
                LiteralTy::Bool => {
                    let bool = self.read_target_int(Self::as_init(bytes)?.as_slice())?;
                    let res = match bool {
                        0 => false,
                        1 => true,
                        _ => {
                            raise_error!(self, span, "Invalid boolean value in constant: {bool}")
                        }
                    };
                    ConstantExprKind::Literal(Literal::Bool(res))
                }
                LiteralTy::Char => ConstantExprKind::Literal(Literal::Char(
                    char::from_u32(self.read_target_uint(Self::as_init(bytes)?.as_slice())? as u32)
                        .unwrap(),
                )),
                LiteralTy::Float(f) => {
                    let bits = self.read_target_uint(Self::as_init(bytes)?.as_slice())?;
                    let value = match f {
                        FloatTy::F16 => ieee::Half::from_bits(bits).to_string(),
                        FloatTy::F32 => ieee::Single::from_bits(bits).to_string(),
                        FloatTy::F64 => ieee::Double::from_bits(bits).to_string(),
                        FloatTy::F128 => ieee::Quad::from_bits(bits).to_string(),
                    };
                    ConstantExprKind::Literal(Literal::Float(FloatValue {
                        value,
                        ty: f.clone(),
                    }))
                }
            },
            TyKind::Ref(_, subty, _) | TyKind::RawPtr(subty, _) => 'ptr_case: {
                let Some(suballoc) = self.provenance_at(alloc, offset) else {
                    let value = self.read_target_uint(Self::as_init(bytes)?.as_slice())?;
                    break 'ptr_case ConstantExprKind::PtrNoProvenance(value);
                };
                use mir::alloc::GlobalAlloc;
                let alloc_id = suballoc.0;
                let glob_alloc: GlobalAlloc = alloc_id.into();
                match glob_alloc {
                    GlobalAlloc::Memory(suballoc) if subty.is_str() => {
                        let as_str =
                            unsafe { String::from_utf8_unchecked(suballoc.raw_bytes().unwrap()) };
                        ConstantExprKind::Literal(Literal::Str(as_str))
                    }
                    GlobalAlloc::Memory(suballoc) if subty.is_slice() => {
                        let meta_bytes = &alloc.bytes.as_slice()[offset + size / 2..offset + size];
                        let len = self.read_target_uint(Self::as_init(meta_bytes)?.as_slice())?;
                        let subty = subty.as_array_or_slice().unwrap();
                        let rtyk = rty.kind().builtin_deref(true).unwrap().ty.kind();
                        let ty::RigidTy::Slice(rsubty) = rtyk.rigid().unwrap() else {
                            unreachable!("Unexpected rigid type for slice: {rty:?}");
                        };
                        let elem_size = rsubty.layout().unwrap().shape().size.bytes();
                        let sub_constants = (0..len as usize)
                            .map(|i| {
                                let elem_off = elem_size * i;
                                self.translate_allocation_at(
                                    span,
                                    &suballoc,
                                    subty.kind(),
                                    *rsubty,
                                    elem_off,
                                )
                            })
                            .try_collect()?;
                        let len = ConstantExpr::mk_usize(ScalarValue::Unsigned(UIntTy::Usize, len));
                        let sub_constant = ConstantExpr {
                            kind: ConstantExprKind::Array(sub_constants),
                            ty: Ty::mk_array(subty.clone(), len.clone()),
                        };
                        let metadata = Some(UnsizingMetadata::Length(Box::new(len)));

                        if let TyKind::RawPtr(_, rk) = ty {
                            ConstantExprKind::Ptr(*rk, Box::new(sub_constant), metadata)
                        } else {
                            ConstantExprKind::Ref(Box::new(sub_constant), metadata)
                        }
                    }
                    _ => {
                        let metadata = match subty.kind() {
                            TyKind::Slice(_) => {
                                let meta_bytes =
                                    &alloc.bytes.as_slice()[offset + size / 2..offset + size];
                                let len =
                                    self.read_target_uint(Self::as_init(meta_bytes)?.as_slice())?;
                                Some(UnsizingMetadata::Length(Box::new(ConstantExpr::mk_usize(
                                    ScalarValue::Unsigned(UIntTy::Usize, len),
                                ))))
                            }
                            TyKind::DynTrait(_) => {
                                let Some(vtable_prov) =
                                    self.provenance_at(alloc, offset + size / 2)
                                else {
                                    unreachable!("&dyn constant with no vtable provenance?");
                                };
                                let alloc: mir::alloc::GlobalAlloc = vtable_prov.0.into();
                                let vtable_global = match alloc {
                                    mir::alloc::GlobalAlloc::VTable(self_ty, trait_ref) => {
                                        let trait_ref = trait_ref.map(|t| {
                                            let t = rustc_public::rustc_internal::internal(
                                                self.t_ctx.tcx,
                                                t,
                                            );
                                            let t = self
                                                .t_ctx
                                                .tcx
                                                .instantiate_bound_regions_with_erased(t);
                                            let t = rustc_public::rustc_internal::stable(t);
                                            t.with_self_ty(self_ty)
                                        });
                                        self.register_vtable(span, self_ty, trait_ref)
                                    }
                                    _ => {
                                        unreachable!("&dyn constant with non-vtable provenance?");
                                    }
                                };
                                let global_ref = GlobalDeclRef {
                                    id: vtable_global,
                                    generics: Box::new(GenericArgs::empty()),
                                };
                                let meta = ConstantExpr {
                                    kind: ConstantExprKind::Global(global_ref),
                                    ty: TyKind::RawPtr(Ty::mk_unit(), RefKind::Shared).into_ty(),
                                };
                                Some(UnsizingMetadata::VTable(
                                    self.dummy_trait_ref(),
                                    Box::new(meta),
                                ))
                            }
                            _ => None,
                        };

                        let glob_ty = match glob_alloc {
                            // we try checking if there's a static we can use the type of,
                            // to avoid registering unsized globals (a static is always sized)
                            GlobalAlloc::Static(stt) => stt.ty(),
                            _ => rty.kind().builtin_deref(true).unwrap().ty,
                        };

                        let id = self.register_global_decl_id(span, alloc_id, Some(glob_ty));
                        let glob_ty = self.translate_ty(span, glob_ty)?;
                        let generics = match glob_alloc {
                            GlobalAlloc::Static(stt) => {
                                let instance: mir::mono::Instance = stt.into();
                                self.translate_generic_args(span, &instance.args())?
                            }
                            _ => GenericArgs::empty(),
                        };
                        let sub_constant = ConstantExpr {
                            kind: ConstantExprKind::Global(GlobalDeclRef {
                                id,
                                generics: Box::new(generics),
                            }),
                            ty: glob_ty,
                        };

                        if let TyKind::RawPtr(_, rk) = ty {
                            ConstantExprKind::Ptr(*rk, Box::new(sub_constant), metadata)
                        } else {
                            ConstantExprKind::Ref(Box::new(sub_constant), metadata)
                        }
                    }
                }
            }
            TyKind::Adt(TypeDeclRef {
                id: TypeId::Tuple,
                generics,
            }) => {
                let rtyk = rty.kind();
                let ty::RigidTy::Tuple(rtys) = rtyk.rigid().unwrap() else {
                    unreachable!("Unexpected rigid type for tuple: {rty:?}");
                };
                let layout = rty.layout()?.shape();
                let abi::FieldsShape::Arbitrary { offsets } = &layout.fields else {
                    unreachable!("Unexpected layout for tuple: {layout:?}");
                };
                let fields = (0..generics.types.elem_count())
                    .map(|i| {
                        let field_offset = offsets[i].bytes();
                        let field_rty = rtys[i];
                        let field_ty = generics.types.get(TypeVarId::from_usize(i)).unwrap();
                        self.translate_allocation_at(
                            span,
                            alloc,
                            field_ty.kind(),
                            field_rty,
                            field_offset + offset,
                        )
                    })
                    .try_collect()?;
                ConstantExprKind::Adt(None, fields)
            }
            TyKind::Adt(TypeDeclRef {
                id: TypeId::Adt(..),
                ..
            }) => {
                let rtyk = rty.kind();
                let ty::RigidTy::Adt(adt, generics) = rtyk.rigid().unwrap() else {
                    unreachable!("Unexpected rigid type for adt: {rty:?}");
                };
                let layout = rty.layout()?.shape();
                let (variant, rfields, offsets) = match adt.kind() {
                    ty::AdtKind::Struct => {
                        let variant = adt.variants()[0];
                        let rfields = variant
                            .fields()
                            .iter()
                            .map(|f| f.ty_with_args(generics))
                            .collect::<Vec<ty::Ty>>();
                        let abi::FieldsShape::Arbitrary { offsets } = &layout.fields else {
                            unreachable!("Unexpected layout for struct: {layout:?}");
                        };
                        (None, rfields, offsets.clone())
                    }
                    ty::AdtKind::Enum => 'enum_case: {
                        if let abi::VariantsShape::Single { index } = layout.variants {
                            let variant = VariantId::new(index.to_index());
                            let variant_data = adt.variant(index).unwrap();
                            let rfields = variant_data
                                .fields()
                                .iter()
                                .map(|f| f.ty_with_args(generics))
                                .collect::<Vec<ty::Ty>>();
                            let abi::FieldsShape::Arbitrary { offsets } = &layout.fields else {
                                unreachable!("Unexpected layout for struct: {layout:?}");
                            };
                            break 'enum_case (Some(variant), rfields, offsets.clone());
                        };
                        let abi::VariantsShape::Multiple {
                            tag,
                            tag_encoding,
                            tag_field,
                            variants,
                        } = &layout.variants
                        else {
                            unreachable!("Unexpected layout for enum: {layout:?}");
                        };
                        assert!(layout.fields.count() == 1, "Enum with non-1 shared field?");
                        let abi::FieldsShape::Arbitrary { offsets } = &layout.fields else {
                            unreachable!("Unexpected layout for enum: {layout:?}");
                        };
                        let abi::Scalar::Initialized { value: tag_ty, .. } = &tag else {
                            unreachable!("Unexpected tag encoding for enum: {tag:?}");
                        };
                        let (length_bytes, _) = match tag_ty {
                            abi::Primitive::Int { length, signed } => (length.bits() / 8, *signed),
                            abi::Primitive::Pointer(_) => (
                                self.t_ctx.tcx.data_layout.pointer_size().bytes() as usize,
                                false,
                            ),
                            abi::Primitive::Float { .. } => unreachable!("Float tag?"),
                        };
                        let tag_offset = offsets[*tag_field].bytes();
                        let tag_bytes = &bytes[tag_offset..tag_offset + length_bytes];
                        let tag_value =
                            self.read_target_uint(Self::as_init(tag_bytes)?.as_slice())?;
                        let mask = if length_bytes * 8 == 128 {
                            u128::MAX
                        } else {
                            (1u128 << (length_bytes * 8)) - 1
                        };
                        let variant_idx = match tag_encoding {
                            abi::TagEncoding::Direct => adt
                                .variants_iter()
                                .find_map(|v| {
                                    let discr = adt.discriminant_for_variant(v.idx);
                                    ((discr.val & mask) == tag_value).then_some(v.idx)
                                })
                                .unwrap(),
                            abi::TagEncoding::Niche {
                                untagged_variant,
                                niche_variants,
                                niche_start,
                            } => adt
                                .variants_iter()
                                .find_map(|v| {
                                    let discr = adt.discriminant_for_variant(v.idx);
                                    let niche_variants_start =
                                        niche_variants.start().to_index() as u128;

                                    if matches!(tag_ty, abi::Primitive::Int { .. }) {
                                        if discr.val < niche_variants_start {
                                            // The variant is before the niche variants, so it is not tagged.
                                            // Could e.g. be the case if it is uninhabited.
                                            return None;
                                        }
                                        let tag = (discr.val - niche_variants_start)
                                            .wrapping_add(*niche_start);
                                        (tag_value == tag).then_some(v.idx)
                                    } else {
                                        // pointer niche: if no provenance, then niche variant,
                                        // otherwise untagged variant
                                        assert!(
                                            niche_variants.start() == niche_variants.end(),
                                            ">1 niche in ptr niche?"
                                        );
                                        self.provenance_at(alloc, offset)
                                            .is_none()
                                            .then_some(*niche_variants.start())
                                    }
                                })
                                .unwrap_or(*untagged_variant),
                        };
                        let variant = adt.variant(variant_idx).unwrap();
                        let fields = variant
                            .fields()
                            .iter()
                            .map(|f| f.ty_with_args(generics))
                            .collect::<Vec<ty::Ty>>();
                        let variant_idx_charon = self.translate_variant_id(variant_idx);
                        let variant_layout = variants.get(variant_idx.to_index()).unwrap();
                        (
                            Some(variant_idx_charon),
                            fields,
                            variant_layout.offsets.clone(),
                        )
                    }
                    ty::AdtKind::Union => {
                        return Ok(ConstantExpr {
                            kind: ConstantExprKind::RawMemory(
                                self.as_charon_bytes(span, alloc, offset, size),
                            ),
                            ty: ty.clone().into_ty(),
                        });
                    }
                };

                let consts = (0..offsets.len())
                    .map(|field| {
                        let field_offset = offsets[field].bytes();
                        let field_rty = rfields[field];
                        let field_ty = self.translate_ty(span, field_rty)?;
                        self.translate_allocation_at(
                            span,
                            alloc,
                            field_ty.kind(),
                            field_rty,
                            field_offset + offset,
                        )
                    })
                    .try_collect()?;
                ConstantExprKind::Adt(variant.clone(), consts)
            }
            TyKind::Array(subty, _) => {
                let rtyk = rty.kind();
                let ty::RigidTy::Array(subrty, _) = rtyk.rigid().unwrap() else {
                    unreachable!();
                };
                let layout = rty.layout()?.shape();
                let abi::FieldsShape::Array { stride, count } = layout.fields else {
                    unreachable!("Unexpected layout for array: {layout:?}");
                };
                let stride = stride.bytes();
                let elems = (0..count as usize)
                    .map(|i| {
                        let elem_off = stride * i + offset;
                        self.translate_allocation_at(span, alloc, subty, *subrty, elem_off)
                    })
                    .try_collect()?;
                ConstantExprKind::Array(elems)
            }
            TyKind::FnPtr(_) => 'fnptr_case: {
                let Some(prov) = self.provenance_at(alloc, offset) else {
                    let value = self.read_target_uint(Self::as_init(bytes)?.as_slice())?;
                    break 'fnptr_case ConstantExprKind::PtrNoProvenance(value);
                };
                use mir::alloc::GlobalAlloc;
                let glob_alloc: GlobalAlloc = prov.0.into();
                match glob_alloc {
                    GlobalAlloc::Function(instance) => {
                        let TyKind::FnPtr(sig) = ty else {
                            unreachable!("Unexpected type for fn ptr: {ty:?}");
                        };

                        // special-case: if the first argument is ignored, because the signature
                        // is shorter, we do a closure_as_fn conversion
                        let abi = instance.fn_abi()?;
                        let id = if abi.args.len() == sig.skip_binder.inputs.len() + 1
                            && let Some(closure_arg) = abi.args.get(0)
                            && closure_arg.mode == rustc_public::abi::PassMode::Ignore
                            && let ty::TyKind::RigidTy(ty::RigidTy::Closure(closure, args)) =
                                closure_arg.ty.kind()
                        {
                            self.register_closure_as_fn_id(span, closure, args)
                        } else {
                            self.register_fun_decl_id(span, instance)
                        };

                        let generics = self.translate_generic_args(span, &instance.args())?;
                        let fn_ptr = FnPtr {
                            generics: Box::new(generics),
                            kind: Box::new(FnPtrKind::Fun(FunId::Regular(id))),
                        };
                        ConstantExprKind::FnPtr(fn_ptr)
                    }
                    _ => {
                        println!("Gave up for raw memory of fndef with alloc {glob_alloc:?}");
                        ConstantExprKind::RawMemory(self.as_charon_bytes(span, alloc, offset, size))
                    }
                }
            }

            TyKind::Adt(TypeDeclRef {
                id: TypeId::Builtin(BuiltinTy::Box),
                ..
            }) => {
                unreachable!("We never create builtin boxes");
            }
            TyKind::DynTrait(..)
            | TyKind::Slice(..)
            | TyKind::Adt(TypeDeclRef {
                id: TypeId::Builtin(BuiltinTy::Str),
                ..
            }) => {
                unreachable!("Translating unsized constant?");
            }
            TyKind::Error(..)
            | TyKind::Never
            | TyKind::TypeVar(..)
            | TyKind::TraitType(..)
            | TyKind::FnDef(..)
            | TyKind::PtrMetadata(..) => {
                println!("Gave up for raw memory of type {ty:?} with alloc {alloc:?}");
                ConstantExprKind::RawMemory(self.as_charon_bytes(span, alloc, offset, size))
            }
        };
        Ok(ConstantExpr {
            kind,
            ty: ty.clone().into_ty(),
        })
    }

    pub fn translate_zst_constant(
        &mut self,
        span: Span,
        ty: &TyKind,
        rty: ty::Ty,
    ) -> Result<ConstantExpr, Error> {
        let kind = match ty {
            TyKind::FnDef(fnptr) => ConstantExprKind::FnDef(fnptr.skip_binder.clone()),
            TyKind::Array(subty, _) => {
                let rtyk = rty.kind();
                let ty::RigidTy::Array(rty, len) = rtyk.rigid().unwrap() else {
                    unreachable!();
                };
                let len = len.eval_target_usize()?;
                if len > 32 {
                    // FIXME: temporary safeguard for large arrays; ideally we should have some
                    // sort of ConstantExprKind::ArrayRepeat...
                    ConstantExprKind::RawMemory(vec![])
                } else if len == 0 {
                    ConstantExprKind::Array(vec![])
                } else {
                    let cexpr = self.translate_zst_constant(span, subty, *rty)?;
                    ConstantExprKind::Array(vec![cexpr; len as usize])
                }
            }
            TyKind::Adt(TypeDeclRef {
                id: TypeId::Adt(..) | TypeId::Tuple,
                ..
            }) => {
                let rtyk = rty.kind();
                let (variant, rtys) = match rtyk.rigid().unwrap() {
                    ty::RigidTy::Tuple(rtys) => (None, rtys.clone()),
                    ty::RigidTy::Closure(_, generics) => {
                        let tupled_upvars = generics.0.last().unwrap().expect_ty().kind();
                        let Some(ty::RigidTy::Tuple(state_tys)) = tupled_upvars.rigid() else {
                            raise_error!(self, span, "Closure state argument is not a tuple?");
                        };
                        (None, state_tys.clone())
                    }
                    ty::RigidTy::Adt(adt, generics) => {
                        let layout = rty.layout()?.shape();
                        let variant_r = match adt.kind() {
                            ty::AdtKind::Struct => None,
                            ty::AdtKind::Enum => {
                                let abi::VariantsShape::Single { index } = layout.variants else {
                                    println!(
                                        "Unexpected layout for ZST enum\n- Layout: {layout:?}\n- Ty: {rty:?}"
                                    );
                                    return Ok(ConstantExpr {
                                        kind: ConstantExprKind::RawMemory(vec![]),
                                        ty: ty.clone().into_ty(),
                                    });
                                };
                                Some(index)
                            }
                            ty::AdtKind::Union => {
                                return Ok(ConstantExpr {
                                    kind: ConstantExprKind::RawMemory(vec![]),
                                    ty: ty.clone().into_ty(),
                                });
                            }
                        };
                        let variant = variant_r.map(|v| self.translate_variant_id(v));
                        let variant_r = variant_r.unwrap_or(ty::VariantIdx::to_val(0));
                        let fields = adt
                            .variant(variant_r)
                            .unwrap()
                            .fields()
                            .iter()
                            .map(|f| f.ty_with_args(generics))
                            .collect();
                        (variant, fields)
                    }
                    _ => unreachable!("Unexpected rigid type for adt: {rty:?} with kind {rtyk:?}"),
                };
                let fields = rtys
                    .into_iter()
                    .map(|field_rty| {
                        let field_ty = self.translate_ty(span, field_rty)?;
                        self.translate_zst_constant(span, field_ty.kind(), field_rty)
                    })
                    .try_collect()?;
                ConstantExprKind::Adt(variant, fields)
            }
            _ => {
                panic!("Gave up on const for ZST type: {:?}", ty);
                // ConstantExprKind::RawMemory(vec![])
            }
        };
        Ok(ConstantExpr {
            kind,
            ty: ty.clone().into_ty(),
        })
    }

    pub fn translate_tyconst_to_const_expr(
        &mut self,
        span: Span,
        v: &ty::TyConst,
    ) -> Result<ConstantExpr, Error> {
        match v.kind() {
            ty::TyConstKind::Value(rty, alloc) => {
                let ty = self.translate_ty(span, *rty)?;
                self.translate_allocation(span, alloc, ty.kind(), *rty)
            }
            ty::TyConstKind::ZSTValue(rty) => {
                let ty = self.translate_ty(span, *rty)?;
                self.translate_zst_constant(span, ty.kind(), *rty)
            }
            _ => {
                raise_error!(
                    self,
                    span,
                    "Unexpected constant expression kind: {:?}",
                    v.kind()
                )
            }
        }
    }
}
