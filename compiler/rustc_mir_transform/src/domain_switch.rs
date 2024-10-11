#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use crate::MirPass;
use rustc_ast::Mutability;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::DefId;
use rustc_index::IndexVec;
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, Body, CallSource, Const, ConstOperand, HasLocalDecls, Local,
    LocalDecl, Location, Operand, Place, ProjectionElem, Rvalue, SourceInfo, Statement,
    StatementKind, Terminator, TerminatorKind, UnwindAction,
};
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_target::abi::FieldIdx;

struct RenameLocalVisitor<'tcx> {
    from: Local,
    to: Local,
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for RenameLocalVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_local(&mut self, local: &mut Local, _: PlaceContext, _: Location) {
        if *local == self.from {
            *local = self.to;
        }
    }

    fn visit_terminator(&mut self, terminator: &mut Terminator<'tcx>, location: Location) {
        match terminator.kind {
            TerminatorKind::Return => {
                // Do not replace the implicit `_0` access here, as that's not possible. The
                // transform already handles `return` correctly.
            }
            _ => self.super_terminator(terminator, location),
        }
    }
}
#[derive(Debug, Clone, Copy)]
enum LocalRemap<'tcx> {
    Arg((Ty<'tcx>, FieldIdx)),
    Local(Local),
}
struct TransformLocalsVisitor<'tcx, 'a> {
    remap: &'a FxHashMap<Local, LocalRemap<'tcx>>,
    tcx: TyCtxt<'tcx>,
}

impl<'tcx, 'a> MutVisitor<'tcx> for TransformLocalsVisitor<'tcx, 'a> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_local(&mut self, local: &mut Local, _: PlaceContext, _: Location) {}

    fn visit_place(&mut self, place: &mut Place<'tcx>, _: PlaceContext, _: Location) {
        if let Some(map) = self.remap.get(&place.local) {
            match map {
                LocalRemap::Arg((ty, idx)) => {
                    replace_base_local(
                        place,
                        make_tuple_field_place(self.tcx, TUPLE_ARG, *idx, *ty),
                        self.tcx,
                    );
                }
                LocalRemap::Local(loc) => {
                    place.local = *loc;
                }
            }
        }
    }
}

struct DerefTupleArgVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
}

impl<'tcx> MutVisitor<'tcx> for DerefTupleArgVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_place(&mut self, place: &mut Place<'tcx>, _: PlaceContext, _: Location) {
        if place.local == TUPLE_ARG {
            replace_base_local(
                place,
                Place {
                    local: TUPLE_ARG,
                    projection: self.tcx().mk_place_elems(&[ProjectionElem::Deref]),
                },
                self.tcx,
            );
        }
    }
}

/// Allocates a new local and replaces all references of `local` with it. Returns the new local.
///
/// `local` will be changed to a new local decl with type `ty`.
///
/// Note that the new local will be uninitialized. It is the caller's responsibility to assign some
/// valid value to it before its first use.
fn replace_local<'tcx>(
    local: Local,
    ty: Ty<'tcx>,
    body: &mut Body<'tcx>,
    tcx: TyCtxt<'tcx>,
) -> Local {
    let new_decl = LocalDecl::new(ty, body.span);
    let new_local = body.local_decls.push(new_decl);
    body.local_decls.swap(local, new_local);

    RenameLocalVisitor { from: local, to: new_local, tcx }.visit_body(body);

    new_local
}

fn replace_base_local<'tcx>(place: &mut Place<'tcx>, new_base: Place<'tcx>, tcx: TyCtxt<'tcx>) {
    place.local = new_base.local;
    let mut new_projection = new_base.projection.to_vec();
    new_projection.append(&mut place.projection.to_vec());

    place.projection = tcx.mk_place_elems(&new_projection);
}

fn make_tuple_field_place<'tcx>(
    tcx: TyCtxt<'tcx>,
    tuple_local: Local,
    idx: FieldIdx,
    ty: Ty<'tcx>,
) -> Place<'tcx> {
    let tuple_place = Place::from(tuple_local);
    tcx.mk_place_elem(tuple_place, ProjectionElem::Field(idx, ty))
}

fn make_tuple_argument_indirect<'tcx>(tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
    let tuple_type = body.local_decls[TUPLE_ARG].ty;
    let ref_tuple_type = Ty::new_ref(
        tcx,
        tcx.lifetimes.re_erased,
        ty::TypeAndMut { ty: tuple_type, mutbl: Mutability::Mut },
    );
    body.local_decls[TUPLE_ARG].ty = ref_tuple_type;
    let mut vis = DerefTupleArgVisitor { tcx };
    vis.visit_body(body);
}

const TUPLE_ARG: Local = Local::from_u32(1);
const RETURN_LOCAL: Local = Local::from_u32(0);

pub struct DomainSwitch;

impl<'tcx> MirPass<'tcx> for DomainSwitch {
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        sess.opts.unstable_opts.isolate.is_some_and(|isolate| isolate)
    }

    #[instrument(level = "debug", skip_all)]
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        if body.coroutine.is_some() {
            //disable this pass for coroutine
            return;
        }

        if tcx.lang_items().domain_enter().is_none() || tcx.lang_items().domain_exit().is_none() {
            debug!("domain_enter/exit lang items do not exit");
            return;
        }

        let body_span = body.span;
        //prepare enter_domain and exit_domain call
        let domain_enter_def_id = tcx.lang_items().domain_enter().unwrap();
        let domain_exit_def_id = tcx.lang_items().domain_exit().unwrap();
        let prepare_call_terminator =
            |def_id: DefId, target: usize, local_decls: &mut IndexVec<Local, LocalDecl<'tcx>>| {
                //let def_id = tcx.lang_items().domain_enter().unwrap();
                let fn_sig = tcx.fn_sig(def_id).instantiate_identity();
                let args = fn_sig
                    .inputs()
                    .no_bound_vars()
                    .unwrap()
                    .iter()
                    .map(|a| a.clone())
                    .collect::<Vec<_>>();
                let return_ty = fn_sig.output().no_bound_vars().unwrap();
                let return_local = local_decls.push(LocalDecl::new(return_ty, body_span));
                let func_ty = Ty::new_fn_def(tcx, def_id, args);
                let func = Operand::Constant(Box::new(ConstOperand {
                    span: body_span,
                    user_ty: None,
                    const_: Const::zero_sized(func_ty),
                }));
                let terminator = Terminator {
                    source_info: SourceInfo::outermost(body_span),
                    kind: TerminatorKind::Call {
                        func,
                        args: vec![],
                        destination: Place::from(return_local),
                        target: Some(BasicBlock::from_usize(target)),
                        unwind: UnwindAction::Continue,
                        call_source: CallSource::Normal,
                        fn_span: body_span,
                    },
                };
                terminator
            };

        // create a new local for the new tuple parameter
        let new_tuple_type = create_tuple_paramter_ty(tcx, body);
        let mut new_tuple_decl = LocalDecl::new(new_tuple_type, body.span);
        //tuple needs to be mutable so we can assign to it
        new_tuple_decl.mutability = Mutability::Mut;

        let mut new_local_decls: IndexVec<Local, LocalDecl<'tcx>> = IndexVec::new();
        new_local_decls.push(body.local_decls[RETURN_LOCAL].clone()); // push the return local to 0
        new_local_decls.push(new_tuple_decl); //push the tuple arg to 1
        //push the rest except the return and original args
        let mut local_remap: FxHashMap<Local, LocalRemap<'_>> = FxHashMap::default();
        body.local_decls.iter().enumerate().skip(1).for_each(|(idx, decl)| {
            let old_local = Local::from(idx);
            if idx <= body.arg_count {
                //argument
                local_remap.insert(
                    old_local,
                    LocalRemap::Arg((
                        body.local_decls[old_local].ty,
                        FieldIdx::from_usize(idx - 1),
                    )),
                );
            } else {
                //function local
                let new_local = new_local_decls.push(body.local_decls[old_local].clone());
                local_remap.insert(old_local, LocalRemap::Local(new_local));
            }
        });

        let mut local_transform = TransformLocalsVisitor { remap: &local_remap, tcx };
        local_transform.visit_body(body);

        // create the tuple
        /*
        let mut assign_tuple_stats = vec![];
        body.args_iter().for_each(|loc| {
            let &(ty, idx) = remap.get(&loc).unwrap();
            let remapped_place = make_tuple_field_place(tcx, new_tuple_local, idx, ty);
            assign_tuple_stats.push(Statement {
                source_info: SourceInfo::outermost(body.span),
                kind: StatementKind::Assign(Box::new((
                    remapped_place,
                    Rvalue::Use(Operand::Copy(Place::from(loc))),
                ))),
            });
        });
        let terminator =
            prepare_call_terminator(domain_enter_def_id, 1usize, &mut body.local_decls);
        body.basic_blocks_mut().raw.insert(
            0,
            BasicBlockData {
                statements: assign_tuple_stats,
                terminator: Some(terminator),
                is_cleanup: false,
            },
        );
        */

        // assign tuple back to arg if it is a mutable reference
        /*
        let mut assign_arg_stats: Vec<Statement<'tcx>> = vec![];
        body.args_iter().for_each(|loc| {
            let &(ty, idx) = remap.get(&loc).unwrap();
            if ty.ref_mutability().is_some_and(|m| m.is_mut()) {
                let remapped_place = make_tuple_field_place(tcx, new_tuple_local, idx, ty);
                assign_arg_stats.push(Statement {
                    source_info: SourceInfo::outermost(body.span),
                    kind: StatementKind::Assign(Box::new((
                        Place::from(loc),
                        Rvalue::Use(Operand::Copy(remapped_place)),
                    ))),
                })
            }
        });

        //create a new empty return block
        let return_block = body.basic_blocks_mut().push(BasicBlockData {
            statements: vec![],
            terminator: Some(Terminator {
                source_info: SourceInfo::outermost(body_span),
                kind: TerminatorKind::Return,
            }),
            is_cleanup: false,
        });
        debug!("exit_domain return block: {:?}", return_block);
        let terminator =
            prepare_call_terminator(domain_exit_def_id, return_block.into(), &mut body.local_decls);

        let mut assign_back = AssignBackVisitor {
            tcx,
            stats: assign_arg_stats,
            exit_terminator: terminator,
            skip_block: return_block,
        };
        assign_back.visit_body(body);
        */
        body.arg_count = 1;
        body.local_decls = new_local_decls;
        make_tuple_argument_indirect(tcx, body); // this needs to be after we have set the new local_decls
        debug!("transformed body: {:?}", body);
    }
}

fn create_tuple_paramter_ty<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> Ty<'tcx> {
    let args_type = body.args_iter().map(|l| body.local_decls[l].ty);
    let new_tuple_ty = Ty::new_tup_from_iter(tcx, args_type);
    new_tuple_ty
}

struct AssignBackVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    stats: Vec<Statement<'tcx>>,
    exit_terminator: Terminator<'tcx>,
    skip_block: BasicBlock,
}

impl<'tcx> MutVisitor<'tcx> for AssignBackVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &mut BasicBlockData<'tcx>) {
        if block == self.skip_block {
            return;
        }
        match data.terminator().kind {
            TerminatorKind::Return => {
                data.statements.extend(self.stats.iter().cloned());
                data.terminator_mut().kind = self.exit_terminator.kind.clone();
            }
            _ => {}
        }
    }
}

cfg_if::cfg_if! {
    if #[cfg(bootstrap)]{

        fn insert_domain_switch<'tcx>(_tcx: TyCtxt<'tcx>, _body: &mut Body<'tcx>) -> () {}

    }else{
        fn insert_domain_switch<'tcx>(tcx: TyCtxt<'tcx>, _body: &mut Body<'tcx>) -> () {
            let _domain_enter = tcx.lang_items().domain_enter().unwrap();

        }

    }

}
