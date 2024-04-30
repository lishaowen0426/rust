use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
use rustc_mir_dataflow::impls::SafetyLocals;
use rustc_mir_dataflow::Analysis;
pub struct SafetyProp;

impl<'tcx> MirPass<'tcx> for SafetyProp {
    fn is_enabled(&self, _sess: &rustc_session::Session) -> bool {
        true
    }

    #[instrument(level = "debug", skip_all)]
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let mut unsafe_locals =
            SafetyLocals.into_engine(tcx, body).iterate_to_fixpoint().into_results_cursor(body);
        for (bb, _bb_data) in body.basic_blocks.iter_enumerated() {
            unsafe_locals.seek_to_block_start(bb);
            let state = unsafe_locals.get();
            let results: Vec<Local> = state.iter().collect();
            debug!(
                "unsafe locals that will be propagated to the predecessors of {bb:?}: {results:?}"
            );
        }
    }
}
