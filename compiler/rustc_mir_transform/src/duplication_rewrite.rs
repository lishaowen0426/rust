#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use rustc_middle::mir::MirPass;
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, BasicBlocks, Body, CallSource, Const, ConstOperand, HasLocalDecls,
    Local, LocalDecl, Location, Operand, Place, ProjectionElem, Rvalue, SourceInfo, Statement,
    StatementKind, Terminator, TerminatorKind, UnwindAction,
};
use rustc_middle::ty::{self, IntTy, Ty, TyCtxt, TypeVisitableExt};
pub struct DuplicationRewrite;

impl<'tcx> MirPass<'tcx> for DuplicationRewrite {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        sess.opts.unstable_opts.isolate.is_some_and(|isolate| isolate)
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let (_, set) = tcx.duplicate_map(());

        if let Some(def_id) = body.source.def_id().as_local() {
            //only rewrite local item
            if set.contains(&def_id) {
                self.run(tcx, body);
            }
        } else {
            return;
        }
    }
}

impl<'tcx> DuplicationRewrite {
    #[instrument(level = "debug", skip_all)]
    fn run(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        debug!("duplicate rewrite:{:?}", body);
    }
}
