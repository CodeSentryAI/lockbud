use charon_lib::export::CrateData;
use std::fs::File;
use std::path::PathBuf;

fn format_name(name: &charon_lib::ast::Name) -> String {
    name.name.iter().map(|elem| match elem {
        charon_lib::ast::PathElem::Ident(s, _) => s.clone(),
        _ => "?".to_string(),
    }).collect::<Vec<_>>().join("::")
}

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).unwrap());
    let file = File::open(&path).unwrap();
    let crate_data: CrateData = serde_json::from_reader(file).unwrap();
    let krate = &crate_data.translated;

    for (_fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let name = format_name(&fun_decl.item_meta.name);
        if !name.contains("std_mutex") {
            continue;
        }
        println!("Function: {}", name);
        let charon_lib::ast::Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        for (local_id, local) in body.locals.locals.iter_indexed() {
            let ty_str = format_ty(&local.ty);
            println!("  local {:?}: {}", local_id, ty_str);
        }
        for (block_id, block) in body.body.iter_indexed() {
            println!("  block {:?}:", block_id);
            for (stmt_idx, stmt) in block.statements.iter().enumerate() {
                println!("    stmt {}: {:?}", stmt_idx, stmt.kind);
            }
            println!("    term: {:?}", block.terminator.kind);
        }
    }
}

fn format_ty(ty: &charon_lib::ast::Ty) -> String {
    use charon_lib::ast::TyKind;
    match ty.kind() {
        TyKind::Adt(adt_ref) => {
            if let charon_lib::ast::TypeId::Adt(id) = adt_ref.id {
                format!("Adt({})", id.index())
            } else {
                "Adt(?)".to_string()
            }
        }
        TyKind::TypeVar(id) => {
            let var_id = match id {
                charon_lib::ast::DeBruijnVar::Bound(_, id) | charon_lib::ast::DeBruijnVar::Free(id) => id,
            };
            format!("TypeVar({})", var_id.index())
        }
        TyKind::Literal(ty) => format!("{:?}", ty),
        TyKind::Ref(_, ty, _) => format!("&{}", format_ty(ty)),
        TyKind::FnPtr(sig) => {
            let sig = &sig.skip_binder;
            let a: Vec<_> = sig.inputs.iter().map(format_ty).collect();
            format!("fn({}) -> {}", a.join(", "), format_ty(&sig.output))
        }
        TyKind::Never => "!".to_string(),
        _ => format!("{:?}", ty.kind()),
    }
}
