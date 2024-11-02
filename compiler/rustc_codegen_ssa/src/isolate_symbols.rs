use rustc_hir::def_id::CrateNum;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_middle::middle::exported_symbols::SymbolExportKind;
use rustc_middle::ty::TyCtxt;
use rustc_session::config::CrateType;
static ISOLATE_STACK_INIT_FN_PREFIX: &'static str = "__rust_isolate_stack_init";
static ISOLATE_STACK_GLOBAL_PREFIX: &'static str = "__rust_isolate_stack";

pub static ISOLATE_STACK_SIZE: usize = 512 * 1024;
pub static ISOLATE_STACK_ALIGN: usize = 16;

pub fn isolate_stack_global_name(tcx: TyCtxt<'_>, crate_name: Option<CrateNum>) -> String {
    if let Some(cn) = crate_name {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(cn))
    } else {
        format!("{}_{}", ISOLATE_STACK_GLOBAL_PREFIX, tcx.crate_name(LOCAL_CRATE))
    }
}

pub fn isolate_stack_fn_name(tcx: TyCtxt<'_>, crate_name: Option<CrateNum>) -> String {
    if let Some(cn) = crate_name {
        format!("{}_{}", ISOLATE_STACK_INIT_FN_PREFIX, tcx.crate_name(cn))
    } else {
        format!("{}_{}", ISOLATE_STACK_INIT_FN_PREFIX, tcx.crate_name(LOCAL_CRATE))
    }
}

#[instrument(level = "debug", skip_all)]
pub(crate) fn isolate_exported_symbols(tcx: TyCtxt<'_>) -> Vec<(String, SymbolExportKind)> {
    let crate_types = tcx.crate_types();
    let is_executable_crate = crate_types.contains(&CrateType::Executable);
    let mut res = vec![];
    if is_executable_crate {
        tcx.isolate_crates(()).iter().for_each(|cn| {
            res.push((isolate_stack_global_name(tcx, Some(*cn)), SymbolExportKind::Tls));
            res.push((isolate_stack_fn_name(tcx, Some(*cn)), SymbolExportKind::Data));
        });
    }
    res
}
