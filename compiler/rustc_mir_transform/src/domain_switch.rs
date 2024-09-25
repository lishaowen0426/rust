use crate::MirPass;
use rustc_middle::mir::Body;
use rustc_middle::ty::TyCtxt;

pub struct DomainSwitch;

impl<'tcx> MirPass<'tcx> for DomainSwitch {
    fn is_enabled(&self, _: &rustc_session::Session) -> bool {
        true
    }

    #[instrument(level = "debug", skip_all)]
    fn run_pass(&self, _tcx: TyCtxt<'tcx>, _body: &mut Body<'tcx>) {}
}
