//! Type representations
//!
//! Stable types matching rustc_public::ty

use std::fmt;

/// A type - represented as an opaque index
///
/// Types are stored in a thread-local table and accessed by index.
/// This prevents mixing types between different compiler instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ty {
    index: usize,
    thread_local_index: usize,
}

impl Ty {
    /// Create a new Ty (for use by the implementation)
    pub fn new(index: usize, thread_local_index: usize) -> Self {
        Self { index, thread_local_index }
    }

    /// Get the index of this type
    pub fn index(&self) -> usize {
        self.index
    }

    /// Get the kind of this type
    pub fn kind(&self) -> TyKind {
        // This would be implemented by calling into rustc
        TyKind::RigidTy(RigidTy::Never)
    }

    /// Check if this type is a specific rigid type
    pub fn is_unit(&self) -> bool {
        match self.kind() {
            TyKind::RigidTy(RigidTy::Tuple(ref tys)) => tys.is_empty(),
            _ => false,
        }
    }

    /// Check if this type is a specific rigid type
    pub fn is_bool(&self) -> bool {
        matches!(self.kind(), TyKind::RigidTy(RigidTy::Bool))
    }

    /// Check if this type is a specific rigid type
    pub fn is_integral(&self) -> bool {
        matches!(self.kind(), TyKind::RigidTy(RigidTy::Int(_) | RigidTy::Uint(_)))
    }

    /// Check if this type is a reference
    pub fn is_ref(&self) -> bool {
        matches!(self.kind(), TyKind::RigidTy(RigidTy::Ref(_, _, _)))
    }

    /// Check if this type is a raw pointer
    pub fn is_raw_ptr(&self) -> bool {
        matches!(self.kind(), TyKind::RigidTy(RigidTy::RawPtr(_, _)))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ty({})", self.index)
    }
}

/// The kind of a type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TyKind {
    /// A rigid type (fully known, no inference variables)
    RigidTy(RigidTy),

    /// An alias type (opaque type, type projection, etc.)
    Alias(AliasKind, AliasTy),

    /// A type parameter (generic parameter)
    Param(ParamTy),

    /// A bound type (for `for<'a> fn(&'a T)`)
    Bound(usize, BoundTy),
}

/// A rigid type - a fully known type without inference variables
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigidTy {
    /// Boolean type
    Bool,

    /// Character type
    Char,

    /// Signed integer type
    Int(IntTy),

    /// Unsigned integer type
    Uint(UintTy),

    /// Floating point type
    Float(FloatTy),

    /// Algebraic data type (struct, enum, union)
    Adt(AdtDef, GenericArgs),

    /// Foreign type (from FFI)
    Foreign(ForeignDef),

    /// String slice type
    Str,

    /// Array type `[T; N]`
    Array(Ty, TyConst),

    /// Pattern type (for pattern matching)
    Pat(Ty, PatTy),

    /// Slice type `[T]`
    Slice(Ty),

    /// Raw pointer type `*const T` or `*mut T`
    RawPtr(Ty, Mutability),

    /// Reference type `&'a T` or `&'a mut T`
    Ref(Region, Ty, Mutability),

    /// Function definition type
    FnDef(FnDef, GenericArgs),

    /// Function pointer type `fn(...) -> ...`
    FnPtr(PolyFnSig),

    /// Closure type
    Closure(ClosureDef, GenericArgs),

    /// Coroutine type (aka generator)
    Coroutine(CoroutineDef, GenericArgs),

    /// Coroutine closure type (from coroutine-closure feature)
    CoroutineClosure(CoroutineClosureDef, GenericArgs),

    /// Dynamic trait object type `dyn Trait`
    Dynamic(Vec<BoundExistentialPredicate>, Region),

    /// Never type `!`
    Never,

    /// Tuple type `(T1, T2, ...)`
    Tuple(Vec<Ty>),

    /// Coroutine witness type (internal)
    CoroutineWitness(CoroutineWitnessDef, GenericArgs),
}

/// Signed integer types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

/// Unsigned integer types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UintTy {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

/// Floating point types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FloatTy {
    F32,
    F64,
}

/// Mutability of a reference or raw pointer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mutability {
    Mut,
    Not,
}

/// Movability of a value (e.g., for generators)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Movability {
    Static,
    Movable,
}

/// Type parameter
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParamTy {
    pub index: u32,
    pub name: String,
}

/// Bound type (for `for<'a>`)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundTy {
    pub var: BoundVar,
    pub kind: BoundTyKind,
}

/// Bound variable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundVar {
    pub index: usize,
}

/// Bound type kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundTyKind {
    Anon,
    Named(DefId),
}

/// DefId - opaque identifier for a definition
pub type DefId = usize;

/// Type constant (for array sizes, const generics, etc.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TyConst {
    pub inner: String,
}

/// Pattern type (for pattern matching)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatTy {
    pub inner: String,
}

/// Generic arguments - a list of types for instantiating generics
pub type GenericArgs = Vec<Ty>;

/// Region (lifetime)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Region {
    Erased,
}

/// Region kind
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RegionKind {
    ReErased,
    ReStatic,
    ReLateBound(usize, BoundRegion),
    ReEarlyBound(EarlyBoundRegion),
    ReFree(FreeRegion),
    RePlaceholder(Placeholder),
}

/// Bound region
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundRegion {
    pub var: BoundRegionKind,
    pub name: String,
}

/// Bound region kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundRegionKind {
    BrAnon,
    BrNamed(DefId),
    BrEnv,
}

/// Early bound region (for generics)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EarlyBoundRegion {
    pub def_id: DefId,
    pub index: u32,
    pub name: String,
}

/// Free region
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FreeRegion {
    FrRegion,
    FrLateBound(usize, BoundRegionKind),
}

/// Placeholder region
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Placeholder {
    pub universe: usize,
    pub bound: BoundRegionKind,
}

// Opaque indices for various definition types

pub type AdtDef = usize;
pub type ForeignDef = usize;
pub type FnDef = usize;
pub type ClosureDef = usize;
pub type CoroutineDef = usize;
pub type CoroutineClosureDef = usize;
pub type CoroutineWitnessDef = usize;

/// Alias kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AliasKind {
    Projection,
    Opaque,
    Weak,
}

/// Alias type (for trait associated types and opaque types)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasTy {
    pub args: GenericArgs,
    pub def_id: DefId,
}

/// Bound existential predicate (for trait objects)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundExistentialPredicate {
    Trait(ExistentialTraitRef),
    AutoTrait(DefId),
    Projection(ExistentialProjection),
}

pub type ExistentialTraitRef = usize;
pub type ExistentialProjection = usize;

/// Function signature
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSig {
    pub inputs: Vec<Ty>,
    pub output: Ty,
    pub c_variadic: bool,
}

/// Polymorphic function signature (with where clauses)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyFnSig {
    pub sig: FnSig,
}

/// Function ABI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Abi {
    Rust,
    C,
    Cdecl,
    Stdcall,
    Fastcall,
    Vectorcall,
    Aapcs,
    Win64,
    SysV64,
    PtxKernel,
    Msp430Interrupt,
    X86Interrupt,
    AmdGpuKernel,
    EfiApi,
    AvrInterrupt,
    AvrNonBlockingInterrupt,
    CCmseNonSecureCall,
    CCmseNonSecureEntry,
    System,
    RustIntrinsic,
    RustCall,
    PlatformIntrinsic,
    Unadjusted,
    ThisCall,
    RustCold,
    RiscvInterruptM,
    RustColdReclaim,
}

/// Coroutine kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoroutineKind {
    /// A coroutine that comes from `async`/`async move`
    Async(AsyncKind),

    /// A coroutine that comes from `gen`/`gen move`
    Gen,

    /// A coroutine that comes from a `|| { yield ... }` closure
    CoroutineClosure,
}

/// Async coroutine kind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AsyncKind {
    /// `async`/`async move` blocks
    Block,

    /// `async`/`async move` closures
    Closure,

    /// Functions marked with `async fn`
    Fn,
}
