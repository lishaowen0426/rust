use rustc_index::bit_set::BitSet;
use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
use rustc_mir_dataflow::impls::SafetyLocals;
use rustc_mir_dataflow::Analysis;
pub struct SafetyProp;

impl<'tcx> MirPass<'tcx> for SafetyProp {
    fn is_enabled(&self, _sess: &rustc_session::Session) -> bool {
        true
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        body.initialize_unsafe_locals(body.local_decls.len());
        if !tcx.features().unsafe_alloc {
            return;
        }
        let mut unsafe_locals = SafetyLocals { local_decls: body.local_decls.clone() }
            .into_engine(tcx, body)
            .iterate_to_fixpoint()
            .into_results_cursor(body);
        let mut collected_unsafe_locals = BitSet::<Local>::new_empty(body.local_decls.len());
        for (bb, _bb_data) in body.basic_blocks.iter_enumerated() {
            unsafe_locals.seek_to_block_start(bb);
            let state = unsafe_locals.get();
            let results: Vec<Local> = state
                .iter()
                .map(|loc| {
                    collected_unsafe_locals.insert(loc);
                    loc
                })
                .collect();
            debug!(
                "unsafe locals that will be propagated to the predecessors of {bb:?}: {results:?}"
            );
        }
        collected_unsafe_locals.iter().for_each(|loc| {
            debug!("unsafe: {loc:?}");
            body.set_unsafe_local(loc);
        })
    }
}
