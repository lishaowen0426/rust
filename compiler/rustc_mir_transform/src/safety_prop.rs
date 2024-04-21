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
        debug!("SafetyProp runs");
        let _unsafe_locals = SafetyLocals.into_engine(tcx, body).iterate_to_fixpoint();
    }
}
