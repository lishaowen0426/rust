#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use crate::MirPass;
use rustc_ast::Mutability;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_index::IndexVec;
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, Body, HasLocalDecls, Local, LocalDecl, Location, Operand, Place,
    ProjectionElem, Rvalue, SourceInfo, Statement, StatementKind, Terminator, TerminatorKind,
};
use rustc_middle::ty::{Ty, TyCtxt};
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

const TUPLE_ARG: Local = Local::from_u32(1);
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

        // create a new local for the new tuple parameter
        let new_tuple_type = create_tuple_paramter_ty(tcx, body);
        let mut new_tuple_decl = LocalDecl::new(new_tuple_type, body.span);
        //tuple needs to be mutable so we can assign to it
        new_tuple_decl.mutability = Mutability::Mut;
        let new_tuple_local = body.local_decls.push(new_tuple_decl);

        //map parameter local to tuple field
        let mut remap: FxHashMap<Local, (Ty<'tcx>, FieldIdx)> = FxHashMap::default();
        body.args_iter().enumerate().for_each(|(i, l)| {
            remap.insert(l, (body.local_decls[l].ty, FieldIdx::from_usize(i)));
        });

        // create the tuple
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
        let body_span = body.span;
        body.basic_blocks_mut().raw.insert(
            0,
            BasicBlockData {
                statements: assign_tuple_stats,
                terminator: Some(Terminator {
                    source_info: SourceInfo::outermost(body_span),
                    kind: TerminatorKind::Goto { target: BasicBlock::from_u32(1u32) },
                }),
                is_cleanup: false,
            },
        );

        //change arg ref to tuple ref
        let mut arg_transform =
            TransformArgsVisitor { tcx, tuple_local: new_tuple_local, remap: &remap };
        arg_transform.visit_body(body);

        // assign tuple back to arg if it is a mutable reference
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
        let mut assign_back = AssignBackVisitor { tcx, stats: assign_arg_stats };
        assign_back.visit_body(body);
    }
}

fn create_tuple_paramter_ty<'tcx>(tcx: TyCtxt<'tcx>, body: &Body<'tcx>) -> Ty<'tcx> {
    let args_type = body.args_iter().map(|l| body.local_decls[l].ty);
    let new_tuple_ty = Ty::new_tup_from_iter(tcx, args_type);
    new_tuple_ty
}

struct TransformArgsVisitor<'tcx, 'a> {
    tcx: TyCtxt<'tcx>,
    tuple_local: Local,
    remap: &'a FxHashMap<Local, (Ty<'tcx>, FieldIdx)>,
}

impl<'tcx, 'a> MutVisitor<'tcx> for TransformArgsVisitor<'tcx, 'a> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_place(
        &mut self,
        place: &mut Place<'tcx>,
        _context: PlaceContext,
        _location: Location,
    ) {
        if let Some(&(ty, idx)) = self.remap.get(&place.local) {
            replace_base_local(
                place,
                make_tuple_field_place(self.tcx, self.tuple_local, idx, ty),
                self.tcx,
            );
        }
    }
}

struct AssignBackVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    stats: Vec<Statement<'tcx>>,
}

impl<'tcx> MutVisitor<'tcx> for AssignBackVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &mut BasicBlockData<'tcx>) {
        match data.terminator().kind {
            TerminatorKind::Return => {
                data.statements.extend(self.stats.iter().cloned());
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
