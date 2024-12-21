#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(rustc::potential_query_instability)]
use rustc_data_structures::fx::FxHashSet;
use rustc_hir::def_id::LOCAL_CRATE;
use rustc_index::IndexVec;
use rustc_middle::mir::visit::NonMutatingUseContext;
use rustc_middle::mir::{traversal, LocalDecl, Operand, SourceInfo};
use rustc_middle::mir::{
    visit::{MutVisitor, MutatingUseContext, PlaceContext, Visitor},
    BasicBlock, BasicBlockData, Body, CallSource, MirPass, Place, PlaceRef, Rvalue, StatementKind,
    UnwindAction,
};
use rustc_middle::mir::{ClearCrossCrate, Local, Location, Statement, Terminator, TerminatorKind};
use rustc_middle::ty::Ty;
use rustc_middle::ty::TyCtxt;
use rustc_span::DUMMY_SP;
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
        sess.opts.unstable_opts.unsafe_heap
    }

    #[instrument(level = "info", skip_all, name = "propagate_unsafety_run_pass")]
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let crate_name = tcx.crate_name(LOCAL_CRATE);
        /*
        if white_list_crates().contains(crate_name.as_str()) {
            return;
        }
        */

        info!(crate_name=?crate_name);
        info!(source=?body.source);

        let mut backward_collector = BackwardUnsafeLocalCollector {
            unsafe_loc: FxHashSet::default(),
            tcx,
            unsafe_stack: Vec::new(),
            body: body,
        };

        for (bb, data) in traversal::postorder(body) {
            backward_collector.visit_basic_block_data(bb, data);
        }
        let mut forward_collector = ForwardUnsafeLocalCollector {
            unsafe_loc: backward_collector.unsafe_loc.clone(),
            tcx,
            rvalue_unsafe: None,
            body: body,
        };

        for (bb, data) in traversal::reverse_postorder(body) {
            forward_collector.visit_basic_block_data(bb, data);
        }

        let mut result = FxHashSet::default();
        for loc in backward_collector.unsafe_loc.union(&forward_collector.unsafe_loc) {
            result.insert(loc);
        }

        let mut unsafe_count = 0usize;
        let mut safe_count = 0usize;
        for (loc, decl) in body.local_decls.iter_enumerated() {
            info!(
                "{} {:?}: {:?}",
                if result.contains(&loc) {
                    unsafe_count += 1;
                    "UNSAFE"
                } else {
                    safe_count += 1;
                    "SAFE"
                },
                loc,
                decl.ty
            );
        }

        tcx.sess.code_stats.record_unsafe_locals(unsafe_count);
        tcx.sess.code_stats.record_safe_locals(safe_count);

        let mut new_locals: IndexVec<Local, LocalDecl<'tcx>> = IndexVec::new();
        let cur_loc = body.local_decls.len();
        let mut new_blocks = Vec::new();
        let cur_blk = body.basic_blocks.len();

        for block in body.basic_blocks_mut() {
            match block.terminator_mut().kind {
                TerminatorKind::Call {
                    target: Some(ref mut bb), ref destination, unwind, ..
                } if result.contains(&destination.local) => {
                    let set_dest = Local::from_usize(
                        cur_loc
                            + new_locals
                                .push(LocalDecl::new(Ty::new_unit(tcx), DUMMY_SP))
                                .as_usize(),
                    );
                    let clear_dest = Local::from_usize(
                        cur_loc
                            + new_locals
                                .push(LocalDecl::new(Ty::new_unit(tcx), DUMMY_SP))
                                .as_usize(),
                    );

                    let original_call_idx = cur_blk + new_blocks.len();
                    let original_call_target = *bb;
                    let clear_call_idx = original_call_idx + 1;
                    *bb = BasicBlock::from_usize(clear_call_idx);

                    let original_terminator = block.terminator().clone();
                    let original_call_blk = BasicBlockData {
                        statements: vec![],
                        is_cleanup: block.is_cleanup,
                        terminator: Some(original_terminator),
                    };

                    let (set_term, clear_term) = create_mimalloc_call_terminators(
                        tcx,
                        set_dest,
                        clear_dest,
                        BasicBlock::from_usize(original_call_idx),
                        original_call_target,
                        unwind,
                    );

                    *block.terminator_mut() = set_term;
                    let clear_blk = BasicBlockData {
                        statements: vec![],
                        is_cleanup: block.is_cleanup,
                        terminator: Some(clear_term),
                    };

                    new_blocks.push(original_call_blk);
                    new_blocks.push(clear_blk);
                }
                _ => {}
            }
        }

        body.basic_blocks_mut().extend(new_blocks);
        body.local_decls.extend(new_locals);

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
    #[instrument(level = "debug", skip(self, location), name = "unsafe_collector_visit_statement")]
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        if let ClearCrossCrate::Set(ld) =
            self.body.source_scopes[statement.source_info.scope].local_data.clone().as_ref()
        {
            match &statement.kind {
                StatementKind::Assign(box (place, rvalue)) => {
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

struct ForwardUnsafeLocalCollector<'tcx, 'a> {
    unsafe_loc: FxHashSet<Local>,
    tcx: TyCtxt<'tcx>,
    rvalue_unsafe: Option<bool>,
    body: &'a Body<'tcx>,
}

impl<'tcx, 'a> ForwardUnsafeLocalCollector<'tcx, 'a> {
    fn mark_rvalue_unsafe(&mut self) {
        self.rvalue_unsafe = Some(true);
    }

    fn clear_rvalue_unsafe(&mut self) {
        self.rvalue_unsafe = None;
    }

    fn is_rvalue_unsafe(&self) -> bool {
        self.rvalue_unsafe.is_some_and(|s| s)
    }

    fn add_unsafe(&mut self, lo: Local) {
        self.unsafe_loc.insert(lo);
    }

    fn is_local_unsafe(&self, lo: Local) -> bool {
        self.unsafe_loc.contains(&lo)
    }
}
impl<'tcx, 'a> Debug for ForwardUnsafeLocalCollector<'tcx, 'a> {
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

impl<'tcx, 'a> Visitor<'tcx> for ForwardUnsafeLocalCollector<'tcx, 'a> {
    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &BasicBlockData<'tcx>) {
        self.super_basic_block_data(block, data);
    }
    #[instrument(level = "info", skip(self))]
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        if let ClearCrossCrate::Set(ld) =
            self.body.source_scopes[statement.source_info.scope].local_data.clone().as_ref()
        {
            if ld.is_unsafe() {
                //unsafe statement should be processed in backward
                return;
            } else {
                match &statement.kind {
                    StatementKind::Assign(box (place, rvalue)) => {
                        self.visit_rvalue(rvalue, location);
                        info!("is place indirect:{}", place.is_indirect());
                        if self.is_rvalue_unsafe() && !place.is_indirect() {
                            self.add_unsafe(place.local);
                        }
                        self.clear_rvalue_unsafe();
                    }
                    _ => {}
                }
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
                        return;
                    }
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
                        if self.is_rvalue_unsafe() && !destination.is_indirect() {
                            self.add_unsafe(destination.local);
                        }
                        self.clear_rvalue_unsafe();
                    }
                }
                //should consider yield
                _ => {}
            }
        } else {
            info!("clear cross crate: {:?}", terminator);
            return;
        }
    }

    #[instrument(level = "info", skip(self))]
    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: Location) {
        //this should be the place in rvalue and call arguments
        let is_unsafe = match context {
            PlaceContext::NonMutatingUse(NonMutatingUseContext::Copy)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::Move)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::AddressOf)
            | PlaceContext::NonMutatingUse(NonMutatingUseContext::SharedBorrow)
            | PlaceContext::MutatingUse(MutatingUseContext::AddressOf)
            | PlaceContext::MutatingUse(MutatingUseContext::Borrow) => {
                self.is_local_unsafe(place.local)
            }
            _ => false,
        };

        if is_unsafe {
            self.mark_rvalue_unsafe();
        }
    }
}

struct SetMimallocUnsafe<'tcx> {
    unsafe_locals: FxHashSet<Local>,
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for SetMimallocUnsafe<'tcx> {
    fn tcx<'a>(&'a self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &mut BasicBlockData<'tcx>) {}
}

fn create_mimalloc_call_terminators<'tcx>(
    tcx: TyCtxt<'tcx>,
    set_dest: Local,
    clear_dest: Local,
    set_target: BasicBlock,
    clear_target: BasicBlock,
    unwind: UnwindAction,
) -> (Terminator<'tcx>, Terminator<'tcx>) {
    let set = Operand::function_handle(
        tcx,
        tcx.lang_items().set_mimalloc_unsafe().unwrap(),
        vec![],
        DUMMY_SP,
    );

    let set_term = Terminator {
        source_info: SourceInfo::outermost(DUMMY_SP),
        kind: TerminatorKind::Call {
            func: set,
            args: vec![],
            destination: Place::from(set_dest),
            target: Some(set_target),
            unwind,
            call_source: CallSource::Normal,
            fn_span: DUMMY_SP,
        },
    };

    let clear = Operand::function_handle(
        tcx,
        tcx.lang_items().clear_mimalloc_unsafe().unwrap(),
        vec![],
        DUMMY_SP,
    );

    let clear_term = Terminator {
        source_info: SourceInfo::outermost(DUMMY_SP),
        kind: TerminatorKind::Call {
            func: clear,
            args: vec![],
            destination: Place::from(clear_dest),
            target: Some(clear_target),
            unwind,
            call_source: CallSource::Normal,
            fn_span: DUMMY_SP,
        },
    };
    (set_term, clear_term)
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

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_local_decl")]
    fn visit_local_decl(&mut self, local: Local, local_decl: &rustc_middle::mir::LocalDecl<'tcx>) {
        debug!(
            "ty: {}, is any ptr:{}, is box:{}",
            local_decl.ty,
            local_decl.ty.is_any_ptr(),
            local_decl.ty.is_box()
        );
    }

    #[instrument(level = "debug", skip(self), name = "test_visitor_visit_terminator")]
    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        self.super_terminator(terminator, location);
    }
}
