#![feature(rustc_private)]
#![feature(iterator_try_collect)]
#![feature(iter_array_chunks)]
#![feature(impl_trait_in_bindings)]
#![feature(if_let_guard)]

extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_interface;
extern crate rustc_middle;
extern crate rustc_public;
extern crate rustc_session;

pub mod args;
pub mod driver;
pub mod lockbud;
pub mod translate;

#[derive(Debug)]
pub enum ObolError {
    ObolError(usize),
    RustcError,
    Panic,
    Serialize,
}

impl std::fmt::Display for ObolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObolError::RustcError => write!(f, "Code failed to compile")?,
            ObolError::ObolError(err_count) => {
                write!(f, "Obol failed to translate this code ({err_count} errors)")?
            }
            ObolError::Panic => write!(f, "Compilation panicked")?,
            ObolError::Serialize => write!(f, "Could not serialize output file")?,
        }
        Ok(())
    }
}

/// The version of the crate, as defined in `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
