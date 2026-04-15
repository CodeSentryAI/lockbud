use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;

fn format_name(name: &Name) -> String {
    name.name
        .iter()
        .map(|elem| match elem {
            PathElem::Ident(s, _) => s.clone(),
            PathElem::Impl(_) => "{impl}".to_string(),
            PathElem::Instantiated(_) => "{inst}".to_string(),
        })
        .collect::<Vec<_>>()
        .join("::")
}

pub fn print_all_calls(crate_data: &CrateData) {
    let krate = &crate_data.translated;
    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        let fun_name = format_name(&fun_decl.item_meta.name);
        for (block_id, block) in body.body.iter_indexed() {
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let FnOperand::Regular(fn_ptr) = &call.func {
                    if let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref() {
                        if let Some(decl) = krate.fun_decls.get(*callee_id) {
                            let callee_name = format_name(&decl.item_meta.name);
                            if callee_name.contains("lock") {
                                println!(
                                    "  [CALL in {} block {:?}] -> {}",
                                    fun_name, block_id, callee_name
                                );
                                // Print dest local type
                                if let Some(gl) = call.dest.local_id() {
                                    if let Some(local) = body.locals.locals.get(gl) {
                                        println!("       dest local {:?} ty: {:?}", gl, local.ty);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
