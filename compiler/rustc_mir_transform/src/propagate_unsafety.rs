#![allow(unused_variables)]
#![allow(dead_code)]
use rustc_data_structures::fx::FxHashSet;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_middle::mir::visit::NonMutatingUseContext;
use rustc_middle::mir::{traversal, Operand};
use rustc_middle::mir::{
    visit::{MutatingUseContext, PlaceContext, Visitor},
    BasicBlock, BasicBlockData, Body, MirPass, Place, PlaceRef, Rvalue, StatementKind,
};
use rustc_middle::mir::{ClearCrossCrate, Local, Location, Statement, Terminator, TerminatorKind};
use rustc_middle::ty::TyCtxt;
use std::fmt::Debug;
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

    #[instrument(level = "info", skip_all, name = "propagate_unsafety_run_pass")]
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let crate_name = tcx.crate_name(LOCAL_CRATE);
        if white_list_crates().contains(crate_name.as_str()) {
            return;
        }

        info!(crate_name=?crate_name);
        info!(source=?body.source);

        let mut collector = BackwardUnsafeLocalCollector {
            unsafe_loc: FxHashSet::default(),
            tcx,
            unsafe_stack: Vec::new(),
            body: body,
        };

        for (bb, data) in traversal::postorder(body) {
            collector.visit_basic_block_data(bb, data);
        }
        info!("result:\n    {:?}", collector);

        let mut vis = TestVisitor;
        vis.visit_body(body);
    }
}

struct BackwardUnsafeLocalCollector<'tcx, 'a> {
    unsafe_loc: FxHashSet<Local>,
    tcx: TyCtxt<'tcx>,
    unsafe_stack: Vec<bool>, // true == unsafe
    body: &'a Body<'tcx>,
}

impl<'tcx, 'a> Debug for BackwardUnsafeLocalCollector<'tcx, 'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (loc, decl) in self.body.local_decls.iter_enumerated() {
            writeln!(
                f,
                "{} {:?}: {:?}",
                if self.unsafe_loc.contains(&loc) { "UNSAFE" } else { "SAFE" },
                loc,
                decl.ty
            )?;
        }
        Ok(())
    }
}

impl<'tcx, 'a> BackwardUnsafeLocalCollector<'tcx, 'a> {
    fn enter_scope(&mut self, is_unsafe: bool) {
        self.unsafe_stack.push(is_unsafe);
    }

    fn exit_scope(&mut self) {
        if self.unsafe_stack.is_empty() {
            bug!()
        } else {
            self.unsafe_stack.pop();
        }
    }

    fn is_in_unsafe(&self) -> bool {
        if self.unsafe_stack.is_empty() {
            bug!()
        } else {
            self.unsafe_stack[self.unsafe_stack.len() - 1]
        }
    }

    fn add_unsafe(&mut self, lo: Local) {
        self.unsafe_loc.insert(lo);
    }

    fn is_local_unsafe(&self, lo: Local) -> bool {
        self.unsafe_loc.contains(&lo)
    }
}

impl<'tcx, 'a> Visitor<'tcx> for BackwardUnsafeLocalCollector<'tcx, 'a> {
    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &BasicBlockData<'tcx>) {
        //first visit the terminator
        if data.terminator.is_some() {
            self.visit_terminator(
                data.terminator(),
                Location { block, statement_index: data.statements.len() },
            );
        }

        if data.statements.is_empty() {
            return;
        }

        // then statements in reverse order since we traver in postorder
        let last_statement = data.statements.len() - 1; //the index of last statement
        for (idx, statement) in data.statements.iter().rev().enumerate() {
            self.visit_statement(
                statement,
                Location { block, statement_index: last_statement - idx },
            );
        }
    }
    #[instrument(level = "info", skip(self, location), name = "unsafe_collector_visit_statement")]
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        if let ClearCrossCrate::Set(ld) =
            self.body.source_scopes[statement.source_info.scope].local_data.clone().as_ref()
        {
            match &statement.kind {
                StatementKind::Assign(box (place, rvalue)) => {
                    let ty = self.body.local_decls[place.local].ty;

                    if ld.is_unsafe() {
                        self.add_unsafe(place.local);
                    }
                    // if this is either an unsafe statement, or the LHS local has been marked
                    // as unsafe before. For example, this is a safe statement, but the LHS local(i.e., assigned)
                    // is subsequently accessed in an unsafe statement RHS
                    self.enter_scope(ld.is_unsafe() || self.is_local_unsafe(place.local));
                    self.visit_rvalue(rvalue, location);
                    self.exit_scope();
                }
                _ => {}
            }
        } else {
            info!("clear cross crate: {:?}", statement);
            return;
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        if let ClearCrossCrate::Set(ld) =
            self.body.source_scopes[terminator.source_info.scope].local_data.clone().as_ref()
        {
            match &terminator.kind {
                TerminatorKind::Call { func, args, destination, .. } => {
                    if ld.is_unsafe() {
                        self.add_unsafe(destination.local);
                    }
                    self.enter_scope(ld.is_unsafe() || self.is_local_unsafe(destination.local));
                    for a in args.iter() {
                        match &a.node {
                            Operand::Copy(p) => {
                                self.visit_place(
                                    p,
                                    PlaceContext::NonMutatingUse(NonMutatingUseContext::Copy),
                                    location,
                                );
                            }
                            Operand::Move(p) => {
                                self.visit_place(
                                    p,
                                    PlaceContext::NonMutatingUse(NonMutatingUseContext::Move),
                                    location,
                                );
                            }
                            _ => {}
                        }
                    }
                    self.super_terminator(terminator, location);
                    self.exit_scope();
                }
                //should consider yield
                _ => {}
            }
        } else {
            info!("clear cross crate: {:?}", terminator);
            return;
        }
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue<'tcx>, location: Location) {
        self.super_rvalue(rvalue, location);
    }

    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: Location) {
        let is_unsafe = self.is_in_unsafe();
        let place_ty = place.ty(&self.body.local_decls, self.tcx);

        let is_referred = match context {
            PlaceContext::MutatingUse(MutatingUseContext::AddressOf)
            | PlaceContext::MutatingUse(MutatingUseContext::Borrow)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::AddressOf)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::SharedBorrow) => true,
            _ => false,
        };

        let is_copied = match context {
            PlaceContext::NonMutatingUse(NonMutatingUseContext::Copy)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::Move) => true,
            _ => false,
        };

        // 1. if this is an indirect reference, this local itself won't be marked unsafe
        // 2. if this is a copy, this local will be marked unsafe if it contains a reference
        //    for example,  unsafe a: * = b: *;   copy pointer
        if is_unsafe && is_referred && !place.is_indirect() {
            self.add_unsafe(place.local);
        } else if is_unsafe && is_copied && place_ty.ty.is_any_ptr() {
            self.add_unsafe(place.local);
        }
    }
}

struct TestVisitor;
impl<'tcx> Visitor<'tcx> for TestVisitor {
    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_projection")]
    fn visit_projection(
        &mut self,
        place_ref: PlaceRef<'tcx>,
        context: PlaceContext,
        location: Location,
    ) {
        self.super_projection(place_ref, context, location);
    }

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_place")]
    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: Location) {
        self.super_place(place, context, location);
    }

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_statement")]
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        debug!("kind = {:?}", statement.kind);
        self.super_statement(statement, location);
    }

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_local")]
    fn visit_local(&mut self, _local: Local, _context: PlaceContext, _location: Location) {}

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_terminator")]
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        self.super_terminator(terminator, location);
    }
}
