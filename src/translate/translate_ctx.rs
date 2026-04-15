//! The translation contexts.
extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_public;

use super::translate_crate::TransItemSource;
use charon_lib::ast::*;
use charon_lib::formatter::{FmtCtx, IntoFormatter};
use charon_lib::options::TranslateOptions;
use rustc_middle::ty::TyCtxt;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

// Re-export to avoid having to fix imports.
pub(crate) use charon_lib::errors::{DepSource, ErrorCtx, Level};

/// Translation context used while translating the crate data into our representation.
pub struct TranslateCtx<'tcx> {
    /// The Rust compiler type context
    pub tcx: TyCtxt<'tcx>,

    /// The options that control translation.
    pub options: TranslateOptions,
    /// The translated data.
    pub translated: TranslatedCrate,

    /// Path to the toolchain root.
    pub sysroot: PathBuf,

    /// The map from rustc id to translated id.
    pub id_map: HashMap<TransItemSource, ItemId>,
    /// The reverse map of ids.
    pub reverse_id_map: HashMap<ItemId, TransItemSource>,
    /// The reverse filename map.
    pub file_to_id: HashMap<FileName, FileId>,

    /// Cache of StableMir type IDs to our translated types.
    pub type_trans_cache: HashMap<rustc_public::ty::Ty, Ty>,

    /// Context for tracking and reporting errors.
    pub errors: RefCell<ErrorCtx>,
    /// The declarations we came accross and which we haven't translated yet. We keep them sorted
    /// to make the output order a bit more stable.
    pub items_to_translate: BTreeSet<TransItemSource>,
    /// The declaration we've already processed (successfully or not).
    pub processed: HashSet<TransItemSource>,
    /// Cache the names to compute them only once each.
    pub cached_names: HashMap<rustc_public::DefId, Name>,
    /// Cache the `ItemMeta`s to compute them only once each.
    pub cached_item_metas: HashMap<TransItemSource, ItemMeta>,
    /// When building a test binary (`OBOL_BUILDING_TEST`), the set of test function paths
    /// detected via `#[rustc_test_marker]` on generated `TestDescAndFn` consts. Used to
    /// inject a synthetic `"test"` attribute so the OCaml side can find them as entry points.
    // FIXME: in the future, we should look into preserving attributes that exist in the AST.
    // see https://rust-lang.zulipchat.com/#narrow/channel/131828-t-compiler/topic/Retrieving.20.60.23.5Btest.5D.60.20and.20.60.23.5Bshould_panic.5D.60/with/577547122
    pub test_fn_paths: HashSet<String>,
}

/// A translation context for items.
/// Augments the [TranslateCtx] with type-level variables.
pub(crate) struct ItemTransCtx<'tcx, 'ctx> {
    /// The id of the definition we are currently extracting, if there is one.
    pub item_id: Option<ItemId>,
    /// The translation context containing the top-level definitions/ids.
    pub t_ctx: &'ctx mut TranslateCtx<'tcx>,
}

impl<'tcx, 'ctx> TranslateCtx<'tcx> {
    /// Span an error and register the error.
    pub fn span_err(&self, span: Span, msg: &str, level: Level) -> Error {
        self.errors
            .borrow_mut()
            .span_err(&self.translated, span, msg, level)
    }

    pub(crate) fn with_item_id<F, T>(&mut self, item_id: Option<ItemId>, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let mut errors = self.errors.borrow_mut();
        let current_def_id = std::mem::replace(&mut errors.def_id, item_id);
        drop(errors); // important: release the refcell "lock"
        let ret = f(self);
        let mut errors = self.errors.borrow_mut();
        errors.def_id = current_def_id;
        ret
    }
}

impl<'tcx, 'ctx> ItemTransCtx<'tcx, 'ctx> {
    /// Create a new `ExecContext`.
    pub(crate) fn new(item_id: Option<ItemId>, t_ctx: &'ctx mut TranslateCtx<'tcx>) -> Self {
        ItemTransCtx { item_id, t_ctx }
    }

    pub fn span_err(&self, span: Span, msg: &str, level: Level) -> Error {
        self.t_ctx.span_err(span, msg, level)
    }
}

impl<'a> IntoFormatter for &'a TranslateCtx<'_> {
    type C = FmtCtx<'a>;
    fn into_fmt(self) -> Self::C {
        self.translated.into_fmt()
    }
}
