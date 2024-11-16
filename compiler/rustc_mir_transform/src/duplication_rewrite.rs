#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::{Idx, IndexVec};
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::MirPass;
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, BasicBlocks, Body, CallSource, CastKind, Const, ConstOperand,
    HasLocalDecls, Local, LocalDecl, Location, Operand, Place, ProjectionElem, Rvalue, SourceInfo,
    Statement, StatementKind, Terminator, TerminatorKind, UnwindAction,
};
use rustc_middle::ty::{
    self, GenericArg, GenericArgs, GenericParamDefKind, IntTy, Mutability, Ty, TyCtxt,
    TypeVisitableExt,
};
use rustc_span::source_map::dummy_spanned;
use rustc_span::sym::{args, lifetimes};
use rustc_target::abi::FieldIdx;
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
        for b in body.basic_blocks.iter() {
            for s in b.statements.iter() {
                debug!("{:?}, kind: {:?}", s, s.kind);
            }
        }

        let tuple_ty = create_tuple_parameter_ty(tcx, body, true);
        let trans_type = tcx.lang_items().transmute_to_ref().unwrap();
        let tuple_local = create_cast_to_tuple(tcx, tuple_ty, trans_type, body);
        debug!("ref local: {:?}", tuple_local);

        //collect original arg info

        let mut remap: FxHashMap<Local, (FieldIdx, Ty<'tcx>)> = FxHashMap::default();
        body.args_iter().skip(1 /* the injected */).enumerate().for_each(|(idx, loc)| {
            debug!("idx:{:?}, local:{:?}", idx, loc);
            remap.insert(loc, (FieldIdx::from(idx), body.local_decls[loc].ty));
        });

        let mut vis = ArgReplaceVisitor { tcx, tuple: tuple_local, idx_map: &remap };
        vis.visit_body(body);

        let mut check = ArgCheckVisitor { tcx, idx_map: &remap };
        check.visit_body(body);
    }
}

/// The first arg is a pointer we have injected
/// it points to a tuple which consists of the rest param

pub fn create_tuple_parameter_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    skip_first: bool,
) -> Ty<'tcx> {
    let arg_tys = body
        .args_iter()
        .skip(if skip_first { 1 } else { 0 } /* the injecte */)
        .map(|l| body.local_decls[l].ty);
    Ty::new_tup_from_iter(tcx, arg_tys)
}

/// Return the local used to access the tuple
fn create_cast_to_tuple<'tcx>(
    tcx: TyCtxt<'tcx>,
    cast_to: Ty<'tcx>,
    trans_type_fn: DefId,
    body: &mut Body<'tcx>,
) -> Local {
    let body_span = body.span;

    let inst_args = GenericArgs::for_item(tcx, trans_type_fn, |param, _| match param.kind {
        GenericParamDefKind::Lifetime => tcx.lifetimes.re_erased.into(),
        GenericParamDefKind::Type { .. } => cast_to.into(),
        _ => panic!("transmute_type has wrong generics"),
    });
    let fn_sig = tcx.fn_sig(trans_type_fn).instantiate(tcx, &inst_args);
    let output_ty = fn_sig.output().no_bound_vars().unwrap();
    debug!("output ty: {:?}", output_ty);

    let mut loc_decl = LocalDecl::new(output_ty, body.span);
    let loc = body.local_decls.push(loc_decl);

    let func = Operand::function_handle(tcx, trans_type_fn, inst_args, body_span);
    let cast_block = BasicBlockData {
        statements: vec![],
        terminator: Some(Terminator {
            source_info: SourceInfo::outermost(body_span),
            kind: TerminatorKind::Call {
                func,
                args: vec![dummy_spanned(Operand::Copy(Place::from(Local::from_usize(1))))],
                destination: Place::from(loc),
                target: Some(BasicBlock::from_usize(1usize)),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Normal,
                fn_span: body_span,
            },
        }),
        is_cleanup: false,
    };
    body.basic_blocks_mut().raw.insert(0, cast_block);

    let blocks = body.basic_blocks_mut().iter_mut().skip(1);

    for target in blocks.flat_map(|b| b.terminator_mut().successors_mut()) {
        *target = BasicBlock::new(target.index() + 1);
    }

    loc
}

struct ArgReplaceVisitor<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    tuple: Local,
    idx_map: &'a FxHashMap<Local, (FieldIdx, Ty<'tcx>)>,
}

impl<'tcx, 'a> ArgReplaceVisitor<'tcx, 'a> {
    fn change_local(&self, old_place: &mut Place<'tcx>, idx: FieldIdx, ty: Ty<'tcx>) {
        let mut projs = self
            .tcx
            .mk_place_elems(&[ProjectionElem::Deref, ProjectionElem::Field(idx, ty)])
            .to_vec();
        projs.append(&mut old_place.projection.to_vec());

        old_place.local = self.tuple;
        old_place.projection = self.tcx.mk_place_elems(&projs);
    }
    fn replace(&self, target: &mut Place<'tcx>) {
        if let Some(info) = self.idx_map.get(&target.local) {
            self.change_local(target, info.0, info.1);
        }
    }
}

impl<'tcx, 'a> MutVisitor<'tcx> for ArgReplaceVisitor<'tcx, 'a> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_place(&mut self, place: &mut Place<'tcx>, context: PlaceContext, location: Location) {
        self.replace(place);
    }
}

struct ArgCheckVisitor<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    idx_map: &'a FxHashMap<Local, (FieldIdx, Ty<'tcx>)>,
}

impl<'tcx, 'a> MutVisitor<'tcx> for ArgCheckVisitor<'tcx, 'a> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_local(&mut self, local: &mut Local, _context: PlaceContext, _location: Location) {
        if self.idx_map.get(local).is_some() {
            panic!("{:?} has not been replaced!", local);
        }
    }
}
