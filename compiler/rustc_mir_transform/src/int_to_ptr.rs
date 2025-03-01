use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use rustc_middle::mir::visit::Visitor;
use rustc_middle::mir::*;
use rustc_middle::ty::TyCtxt;
pub(super) struct IntToPtr;

impl<'tcx> crate::MirPass<'tcx> for IntToPtr {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        sess.opts.unstable_opts.int_to_ptr_check
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let mut output = PathBuf::new();
        output
            .push(tcx.sess.io.output_dir.as_ref().unwrap_or(env::current_dir().as_ref().unwrap()));
        output.push("int_to_ptr");
        let mut vis = IntToPtrVisitor { results: Vec::new() };
        vis.visit_body(body);
        if vis.results.is_empty() {
            return;
        }
        eprintln!("Int to ptr found:");
        let mut file = File::create(output).unwrap();
        for location in vis.results {
            let src = body.source_info(location);
            let stmt = body.stmt_at(location);
            writeln!(file, "{:?},{:?}", src, stmt).unwrap();
            eprintln!("{:?},{:?}", src, stmt);
        }
    }
}

struct IntToPtrVisitor {
    results: Vec<Location>,
}

impl<'tcx> Visitor<'tcx> for IntToPtrVisitor {
    fn visit_rvalue(&mut self, rvalue: &Rvalue<'tcx>, location: Location) {
        match rvalue {
            Rvalue::Cast(CastKind::PointerWithExposedProvenance, ..) => {
                self.results.push(location);
            }
            _ => {}
        }
    }
}
