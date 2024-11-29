#![allow(unused_variables)]
#![allow(dead_code)]
use rustc_data_structures::fx::FxHashSet;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_middle::mir::{visit::MutVisitor, Body, MirPass};
use rustc_middle::mir::{Local, Location, Safety, Statement};
use rustc_middle::ty::TyCtxt;
use std::sync::OnceLock;

static WHITE_LIST: [&'static str; 87] = [
    "rustc_smir",
    "rustc_type_ir",
    "rustc_codegen_cranelift",
    "rustc_hir",
    "rustc_privacy",
    "rustc_data_structures",
    "rustc_feature",
    "rustc_error_codes",
    "rustc_next_trait_solver",
    "rustc_llvm",
    "rustc_codegen_llvm",
    "rustc_abi",
    "rustc_fs_util",
    "rustc_pattern_analysis",
    "rustc_arena",
    "rustc_passes",
    "rustc_const_eval",
    "rustc_interface",
    "rustc_ast",
    "rustc_mir_build",
    "rustc_lint",
    "rustc_driver_impl",
    "rustc_fluent_macro",
    "rustc_mir_transform",
    "rustc_ast_passes",
    "rustc_transmute",
    "rustc_middle",
    "rustc_query_system",
    "rustc_baked_icu_data",
    "rustc_symbol_mangling",
    "rustc_lint_defs",
    "stable_mir",
    "rustc_target",
    "rustc",
    "rustc_codegen_ssa",
    "rustc_attr",
    "rustc_driver",
    "rustc_errors",
    "rustc_traits",
    "rustc_parse",
    "rustc_query_impl",
    "rustc_log",
    "rustc_hir_analysis",
    "rustc_ast_pretty",
    "rustc_hir_typeck",
    "rustc_macros",
    "rustc_infer",
    "rustc_serialize",
    "rustc_parse_format",
    "rustc_span",
    "rustc_metadata",
    "rustc_index",
    "rustc_session",
    "rustc_graphviz",
    "rustc_resolve",
    "rustc_codegen_gcc",
    "rustc_ty_utils",
    "rustc_lexer",
    "rustc_incremental",
    "rustc_ast_ir",
    "rustc_index_macros",
    "rustc_trait_selection",
    "rustc_builtin_macros",
    "rustc_hir_pretty",
    "rustc_mir_dataflow",
    "rustc_ast_lowering",
    "rustc_expand",
    "rustc_error_messages",
    "rustc_borrowck",
    "rustc_monomorphize",
    "unwind",
    "std",
    "sysroot",
    "test",
    "rtstartup",
    "profiler_builtins",
    "stdarch",
    "core",
    "rustc-std-workspace-std",
    "rustc-std-workspace-core",
    "backtrace",
    "portable-simd",
    "rustc-std-workspace-alloc",
    "proc_macro",
    "panic_abort",
    "panic_unwind",
    "alloc",
];

pub fn white_list_crates() -> &'static FxHashSet<&'static str> {
    static WHITE_LIST_CRATES: OnceLock<FxHashSet<&str>> = OnceLock::new();
    WHITE_LIST_CRATES.get_or_init(|| {
        let mut s = FxHashSet::default();
        for &c in WHITE_LIST.iter() {
            s.insert(c);
        }
        s
    })
}

pub struct PropagateUnsafety;

impl<'tcx> MirPass<'tcx> for PropagateUnsafety {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        true
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let crate_name = tcx.crate_name(LOCAL_CRATE);
        if white_list_crates().contains(crate_name.as_str()) {
            return;
        }
    }
}

struct UnsafeLocalCollector<'tcx, 'a> {
    unsafe_loc: FxHashSet<Local>,
    tcx: TyCtxt<'tcx>,
    body: &'a Body<'tcx>,
}

impl<'tcx, 'a> MutVisitor<'tcx> for UnsafeLocalCollector<'tcx, 'a> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_statement(&mut self, statement: &mut Statement<'tcx>, location: Location) {
        let safety = self.body.source_scopes[statement.source_info.scope]
            .local_data
            .as_ref()
            .assert_crate_local()
            .safety;

        if let Safety::Safe = safety {
            return;
        }
    }
}
