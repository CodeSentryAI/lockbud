//! Translate information about items: name, attributes, etc.

extern crate rustc_ast_pretty;
extern crate rustc_attr_parsing;
extern crate rustc_hir;
extern crate rustc_public;
extern crate rustc_public_bridge;
extern crate rustc_span;

use super::translate_crate::TransItemSource;
use super::translate_ctx::{ItemTransCtx, TranslateCtx};
use charon_lib::{ast::*, register_error};
use itertools::Itertools;
use log::trace;
use rustc_public::{CrateDef, mir, rustc_internal, ty};
use rustc_public_bridge::IndexedVal;
use rustc_span::RemapPathScopeComponents;
use std::cmp::Ord;
use std::path::{Component, PathBuf};

// Spans
impl<'tcx, 'ctx> TranslateCtx<'tcx> {
    /// Register a file if it is a "real" file and was not already registered
    /// `span` must be a span from which we obtained that filename.
    fn register_file(&mut self, filename: FileName, span: rustc_span::Span) -> FileId {
        // Lookup the file if it was already registered
        match self.file_to_id.get(&filename) {
            Some(id) => *id,
            None => {
                let source_file = self.tcx.sess.source_map().lookup_source_file(span.lo());
                let crate_name = self.tcx.crate_name(source_file.cnum).to_string();
                let file = File {
                    name: filename.clone(),
                    crate_name,
                    contents: source_file.src.as_deref().cloned(),
                };
                let id = self.translated.files.push(file);
                self.file_to_id.insert(filename, id);
                id
            }
        }
    }

    pub fn translate_filename(&mut self, name: &rustc_span::FileName) -> meta::FileName {
        match name {
            rustc_span::FileName::Real(name) => {
                match name.local_path() {
                    Some(path) => {
                        let path = if let Ok(path) = path.strip_prefix(&self.sysroot) {
                            // The path to files in the standard library may be full paths to somewhere
                            // in the sysroot. This may depend on how the toolchain is installed
                            // (rustup vs nix), so we normalize the paths here to avoid
                            // inconsistencies in the translation.
                            if let Ok(path) = path.strip_prefix("lib/rustlib/src/rust") {
                                let mut rewritten_path: PathBuf = "/rustc".into();
                                rewritten_path.extend(path);
                                rewritten_path
                            } else {
                                // Unclear if this can happen, but just in case.
                                let mut rewritten_path: PathBuf = "/toolchain".into();
                                rewritten_path.extend(path);
                                rewritten_path
                            }
                        } else {
                            // Find the cargo home directory: according to cargo docs and having a
                            // look at the cargo source, it's either the `$CARGO_HOME` var or
                            // `$HOME/.cargo`
                            let cargo_home = std::env::var("CARGO_HOME")
                                .map(PathBuf::from)
                                .ok()
                                .or_else(|| std::env::home_dir().map(|p| p.join(".cargo")));
                            if let Some(cargo_home) = cargo_home
                                && let Ok(path) = path.strip_prefix(cargo_home)
                            {
                                // Avoid some more machine-dependent paths in the llbc output.
                                let mut rewritten_path: PathBuf = "/cargo".into();
                                rewritten_path.extend(path);
                                rewritten_path
                            } else {
                                path.into()
                            }
                        };
                        FileName::Local(path)
                    }
                    None => {
                        // We use the virtual name because it is always available.
                        // That name normally starts with `/rustc/<hash>/`. For our purposes we hide
                        // the hash.
                        let virtual_name = name.path(RemapPathScopeComponents::MACRO);
                        let mut components_iter = virtual_name.components();
                        if let Some(
                            [
                                Component::RootDir,
                                Component::Normal(rustc),
                                Component::Normal(hash),
                            ],
                        ) = components_iter.by_ref().array_chunks().next()
                            && rustc.to_str() == Some("rustc")
                            && hash.len() == 40
                        {
                            let path_without_hash = [Component::RootDir, Component::Normal(rustc)]
                                .into_iter()
                                .chain(components_iter)
                                .collect();
                            FileName::Virtual(path_without_hash)
                        } else {
                            FileName::Virtual(virtual_name.into())
                        }
                    }
                }
            }
            // We use the debug formatter to generate a filename.
            // This is not ideal, but filenames are for debugging anyway.
            _ => FileName::NotReal(format!("{name:?}")),
        }
    }

    pub fn translate_raw_span(&mut self, span: &ty::Span) -> meta::SpanData {
        let span = rustc_public::rustc_internal::internal(self.tcx, *span);
        let span = span.source_callsite();
        let smap: &rustc_span::source_map::SourceMap = self.tcx.sess.psess.source_map();
        let filename = smap.span_to_filename(span);
        let filename = self.translate_filename(&filename);
        let file_id = self.register_file(filename, span);

        let convert_loc = |pos: rustc_span::BytePos| -> Loc {
            let loc = smap.lookup_char_pos(pos);
            Loc {
                line: loc.line,
                col: loc.col_display + 1,
            }
        };
        let beg = convert_loc(span.lo());
        let end = convert_loc(span.hi());

        // Put together
        meta::SpanData { file_id, beg, end }
    }

    pub(crate) fn translate_span_from_smir(&mut self, span: &ty::Span) -> Span {
        Span {
            data: self.translate_raw_span(span),
            generated_from_span: None,
        }
    }
}

// Names
impl<'tcx, 'ctx> TranslateCtx<'tcx> {
    /// Retrieve the name for this [`mir::DefId`]. Because a given `DefId` may give rise to several
    /// charon items, prefer to use `translate_name` when possible.
    ///
    /// We lookup the path associated to an id, and convert it to a name.
    /// Paths very precisely identify where an item is. There are important
    /// subcases, like the items in an `Impl` block:
    /// ```ignore
    /// impl<T> List<T> {
    ///   fn new() ...
    /// }
    /// ```
    ///
    /// One issue here is that "List" *doesn't appear* in the path, which would
    /// look like the following:
    ///
    ///   `TypeNS("Crate") :: Impl :: ValueNs("new")`
    ///                       ^^^
    ///           This is where "List" should be
    ///
    /// For this reason, whenever we find an `Impl` path element, we actually
    /// lookup the type of the sub-path, from which we can derive a name.
    ///
    /// Besides, as there may be several "impl" blocks for one type, each impl
    /// block is identified by a unique number (rustc calls this a
    /// "disambiguator"), which we grab.
    ///
    /// Example:
    /// ========
    /// For instance, if we write the following code in crate `test` and module
    /// `bla`:
    /// ```ignore
    /// impl<T> Foo<T> {
    ///   fn foo() { ... }
    /// }
    ///
    /// impl<T> Foo<T> {
    ///   fn bar() { ... }
    /// }
    /// ```
    ///
    /// The names we will generate for `foo` and `bar` are:
    /// `[Ident("test"), Ident("bla"), Ident("Foo"), Impl(impl<T> Ty<T>, Disambiguator(0)), Ident("foo")]`
    /// `[Ident("test"), Ident("bla"), Ident("Foo"), Impl(impl<T> Ty<T>, Disambiguator(1)), Ident("bar")]`
    pub fn def_id_to_name(&mut self, def_id: rustc_public::DefId) -> Result<Name, Error> {
        if let Some(name) = self.cached_names.get(&def_id) {
            return Ok(name.clone());
        }
        trace!("Computing name for `{def_id:?}`");

        // let parent_name = if let Some(parent) = &def_id.parent {
        //     self.def_id_to_name(parent)?
        // } else {
        //     Name { name: Vec::new() }
        // };
        let mut name = Name { name: Vec::new() };
        let name_str = def_id.name();
        for elem in name_str.split("::") {
            // We use `Ident` for all names, even if they are not identifiers.
            // This is because we don't have a `PathElem` for these.
            name.name
                .push(PathElem::Ident(elem.to_string(), Disambiguator::ZERO));
        }

        trace!("Computed name for `{def_id:?}`: `{name:?}`");
        self.cached_names.insert(def_id.clone(), name.clone());
        Ok(name)
    }

    /// Retrieve the name for an item.
    pub fn translate_name(&mut self, src: &TransItemSource) -> Result<Name, Error> {
        let mut name = if let Some(def_id) = src.as_def_id() {
            self.def_id_to_name(def_id)?
        } else {
            match src {
                // VTables may not have a def_id if they are for an auto-trait.
                TransItemSource::VTable(_, _) | TransItemSource::VTableInit(_, _) => Name {
                    name: vec![PathElem::Ident("unknown_trait".into(), Disambiguator::ZERO)],
                },
                TransItemSource::Global(id, ..) | TransItemSource::GlobalConstFn(id, ..) => Name {
                    name: vec![PathElem::Ident(
                        format!("anon_const_{}", id.to_index()),
                        Disambiguator::ZERO,
                    )],
                },
                _ => Name {
                    name: vec![PathElem::Ident("todo_name".into(), Disambiguator::ZERO)],
                },
            }
        };

        match src {
            TransItemSource::Closure(..) => name
                .name
                .push(PathElem::Ident("closure".into(), Disambiguator::ZERO)),
            TransItemSource::ClosureAsFn(..) => {
                name.name
                    .push(PathElem::Ident("closure_as_fn".into(), Disambiguator::ZERO));
            }
            TransItemSource::Fun(_) | TransItemSource::Type(..) => 'add_generics: {
                let (gargs, span) = match src {
                    TransItemSource::Fun(instance) => (instance.args(), instance.def.span()),
                    TransItemSource::Type(adt, gargs) => (gargs.clone().into(), adt.span()),
                    _ => unreachable!(),
                };
                if gargs.0.is_empty() {
                    break 'add_generics;
                }
                let span = self.translate_span_from_smir(&span);
                let mut item_ctx = ItemTransCtx::new(None, self);
                let generics = item_ctx.translate_generic_args(span, &gargs)?;
                name.name
                    .push(PathElem::Instantiated(Box::new(Binder::empty(
                        BinderKind::Other,
                        generics,
                    ))));
            }
            TransItemSource::VTable(ty, tref) | TransItemSource::VTableInit(ty, tref) => {
                let mut item_ctx = ItemTransCtx::new(None, self);
                let generics = if let Some((_, args)) = tref {
                    item_ctx.translate_generic_args(Span::dummy(), &args.clone().into())?
                } else {
                    let charon_ty = item_ctx.translate_ty(Span::dummy(), *ty)?;
                    GenericArgs {
                        const_generics: vec![].into(),
                        types: vec![charon_ty].into(),
                        trait_refs: vec![].into(),
                        regions: vec![].into(),
                    }
                };
                name.name
                    .push(PathElem::Instantiated(Box::new(Binder::empty(
                        BinderKind::Other,
                        generics,
                    ))));
                name.name.splice(
                    0..0,
                    [PathElem::Ident("{vtable}".into(), Disambiguator::ZERO)],
                );
            }
            _ => {}
        }

        Ok(name)
    }

    // pub(crate) fn opacity_for_name(&self, name: &Name) -> ItemOpacity {
    //     self.options.opacity_for_name(&self.translated, name)
    // }
}

// Attributes
impl<'tcx, 'ctx> TranslateCtx<'tcx> {
    // pub(crate) fn translate_inline(&self, def: &mir::FullDef) -> Option<InlineAttr> {
    //     match def.kind() {
    //         mir::DefKind::Fn { inline, .. } | mir::DefKind::AssocFn { inline, .. } => {
    //             match inline {
    //                 mir::InlineAttr::None => None,
    //                 mir::InlineAttr::Hint => Some(InlineAttr::Hint),
    //                 mir::InlineAttr::Never => Some(InlineAttr::Never),
    //                 mir::InlineAttr::Always => Some(InlineAttr::Always),
    //                 mir::InlineAttr::Force { .. } => Some(InlineAttr::Always),
    //             }
    //         }
    //         _ => None,
    //     }
    // }

    fn translate_attribute(&mut self, attrib: &rustc_hir::Attribute) -> Option<Attribute> {
        match attrib {
            rustc_hir::Attribute::Unparsed(attr) => {
                let path = attr.path.segments.iter().map(|i| i.to_string()).join("::");
                let args = match &attr.args {
                    rustc_hir::AttrArgs::Empty => None,
                    rustc_hir::AttrArgs::Eq { expr, .. } => Some(expr.symbol.to_string()),
                    rustc_hir::AttrArgs::Delimited(args) => {
                        Some(rustc_ast_pretty::pprust::tts_to_string(&args.tokens))
                    }
                };
                Some(Attribute::Unknown(RawAttribute { path, args }))
            }
            rustc_hir::Attribute::Parsed(kind) => match kind {
                rustc_hir::attrs::AttributeKind::DocComment { comment, .. } => {
                    Some(Attribute::DocComment(comment.to_string()))
                }
                _ => None,
            },
        }
    }

    pub(crate) fn translate_attr_info(&mut self, span: Span, src: &TransItemSource) -> AttrInfo {
        let Some(def) = src.as_def_id() else {
            return AttrInfo::default();
        };

        // we need to ignore a few cases, as tcx.visibility will panic for them
        let internal = rustc_public::rustc_internal::internal(self.tcx, def);
        if matches!(
            src,
            TransItemSource::Closure(..)
                | TransItemSource::ClosureAsFn(..)
                | TransItemSource::VTable(..)
                | TransItemSource::VTableInit(..)
        ) || (matches!(src, TransItemSource::Fun(..)) && self.tcx.is_closure_like(internal))
        {
            return AttrInfo::default();
        }

        let public = self.tcx.visibility(internal).is_public();
        let attributes = self.tcx.get_all_attrs(internal);

        let mut attributes: Vec<Attribute> = attributes
            .iter()
            .filter_map(|attr| self.translate_attribute(attr))
            .collect();

        // When building a test binary, inject a synthetic `#[test]` attribute on functions
        // detected via `rustc_test_marker`. The original `#[test]` attribute is consumed by
        // macro expansion and is not present in HIR, so we re-create it here so that the OCaml
        // analysis backend can find test entry points via `Attrib "test"`.
        if let TransItemSource::Fun(instance) = src {
            let instance_name = instance.name();
            let is_test = self.test_fn_paths.iter().any(|test_path| {
                instance_name == test_path.as_str()
                    || instance_name.ends_with(&format!("::{test_path}"))
            });
            if is_test {
                attributes.push(Attribute::Unknown(RawAttribute {
                    path: "test".into(),
                    args: None,
                }));
            }
        }

        let rename = {
            let mut renames = attributes.iter().filter_map(|a| a.as_rename()).cloned();
            let rename = renames.next();
            if renames.next().is_some() {
                register_error!(
                    self,
                    span,
                    "There should be at most one `charon::rename(\"...\")` \
                    or `aeneas::rename(\"...\")` attribute per declaration",
                );
            }
            rename
        };

        AttrInfo {
            attributes,
            inline: None,
            public,
            rename,
        }
    }
}

// `ItemMeta`
impl<'tcx, 'ctx> TranslateCtx<'tcx> {
    /// Compute the meta information for a Rust item.
    pub(crate) fn translate_item_meta(
        &mut self,
        item_src: &TransItemSource,
        name: Name,
    ) -> ItemMeta {
        if let Some(item_meta) = self.cached_item_metas.get(&item_src) {
            return item_meta.clone();
        }
        let span = match item_src {
            TransItemSource::Closure(def, _) | TransItemSource::ClosureAsFn(def, _) => {
                Some(def.span())
            }
            TransItemSource::ForeignType(def) => Some(def.span()),
            TransItemSource::Fun(instance) => Some(instance.def.span()),
            TransItemSource::Static(stt) | TransItemSource::StaticFn(stt) => Some(stt.span()),
            TransItemSource::Global(id, ..) | TransItemSource::GlobalConstFn(id, ..) => {
                let glob_alloc: mir::alloc::GlobalAlloc = id.clone().into();
                match glob_alloc {
                    mir::alloc::GlobalAlloc::Function(instance) => Some(instance.def.span()),
                    mir::alloc::GlobalAlloc::Static(static_def) => Some(static_def.span()),
                    _ => None,
                }
            }
            TransItemSource::Type(def, _) => Some(def.span()),
            TransItemSource::VTable(_, tdef) | TransItemSource::VTableInit(_, tdef) => {
                tdef.as_ref().map(|t| t.0.span())
            }
        };
        let span = match span {
            Some(s) => self.translate_span_from_smir(&s),
            None => Span::dummy(),
        };
        let attr_info = self.translate_attr_info(span, item_src);
        let is_local = match item_src.as_def_id() {
            Some(def_id) => {
                let internal = rustc_public::rustc_internal::internal(self.tcx, def_id);
                internal.is_local()
            }
            None => false,
        };

        let lang_item = if let Some(def_id) = item_src.as_def_id() {
            let internal_id = rustc_internal::internal(self.tcx, def_id);
            self.tcx
                .as_lang_item(internal_id)
                .map(|l| l.name().to_ident_string())
        } else {
            None
        };

        let name_opacity = self.options.opacity_for_name(&self.translated, &name);
        let opacity = if attr_info.attributes.iter().any(|attr| attr.is_opaque()) {
            // Force opaque in these cases.
            ItemOpacity::Opaque.max(name_opacity)
        } else {
            name_opacity
        };

        let item_meta = ItemMeta {
            name,
            span,
            source_text: None,
            attr_info,
            is_local,
            opacity,
            lang_item,
        };
        self.cached_item_metas
            .insert(item_src.clone(), item_meta.clone());
        item_meta
    }
}

impl<'tcx, 'ctx> ItemTransCtx<'tcx, 'ctx> {
    pub(crate) fn translate_span_from_smir(&mut self, rspan: &ty::Span) -> Span {
        self.t_ctx.translate_span_from_smir(rspan)
    }

    // pub(crate) fn def_span(&mut self, _def_id: stable_mir::DefId) -> Span {
    //     // self.t_ctx.def_span(def_id)
    //     Span::dummy()
    // }
}
