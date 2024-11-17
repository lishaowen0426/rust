#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::{Idx, IndexVec};
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, BasicBlocks, Body, CallSource, CastKind, Const, ConstOperand,
    CopyNonOverlapping, HasLocalDecls, Local, LocalDecl, Location, Operand, Place, ProjectionElem,
    Rvalue, SourceInfo, Statement, StatementKind, Terminator, TerminatorKind, UnwindAction,
};
use rustc_middle::mir::{MirPass, RETURN_PLACE};
use rustc_middle::ty::{
    self, GenericArg, GenericArgs, GenericParamDefKind, IntTy, Mutability, Ty, TyCtxt,
    TypeVisitableExt,
};
use rustc_session::getopts::Fail;
use rustc_span::source_map::dummy_spanned;
use rustc_span::sym::{args, lifetimes};
use rustc_span::Span;
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

        if body.local_decls[RETURN_PLACE].ty.is_unit() {
            return;
        }
        let body_span = body.span.clone();
        let copy_size_local = body.local_decls.push(LocalDecl::new(tcx.types.usize, body_span));
        let src_ptr_local = body.local_decls.push(LocalDecl::new(
            Ty::new_imm_ptr(tcx, body.local_decls[RETURN_PLACE].ty),
            body_span,
        ));

        let tuple_ret_local = body.local_decls.push(LocalDecl::new(
            Ty::new_mut_ptr(tcx, body.local_decls[RETURN_PLACE].ty),
            body_span,
        ));
        let mut tuple_ret = Place::from(tuple_local);
        let projs = tcx
            .mk_place_elems(&[
                ProjectionElem::Deref,
                ProjectionElem::Field(
                    FieldIdx::from_usize(body.arg_count - 1),
                    Ty::new_mut_ptr(tcx, body.local_decls[RETURN_PLACE].ty),
                ),
            ])
            .to_vec();
        tuple_ret.projection = tcx.mk_place_elems(&projs);
        let tuple_ret_assign_stmt = Statement {
            source_info: SourceInfo::outermost(body_span),
            kind: StatementKind::Assign(Box::new((
                Place::from(tuple_ret_local),
                Rvalue::Use(Operand::Copy(tuple_ret)),
            ))),
        };
        body.basic_blocks.as_mut().raw[0].statements.insert(0, tuple_ret_assign_stmt);

        let mut assign_ret = TupleAssignVisitor {
            tcx,
            tuple: tuple_local,
            idx: FieldIdx::from_usize(body.arg_count - 1),
            ty: Ty::new_mut_ptr(tcx, body.local_decls[RETURN_PLACE].ty),
            span: body_span,
            copy_size_local,
            src_ptr_local,
            return_ty: body.local_decls[RETURN_PLACE].ty,
            tuple_ret_local,
        };
        assign_ret.visit_body(body);
    }
}

// (rest param..., return_value)
pub fn create_tuple_parameter_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &Body<'tcx>,
    skip_first: bool,
) -> Ty<'tcx> {
    let arg_tys = body
        .args_iter()
        .skip(if skip_first { 1 } else { 0 })
        .map(|l| body.local_decls[l].ty)
        .chain(body.local_decls.iter().take(1).map(|l| Ty::new_mut_ptr(tcx, l.ty)));

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

struct TupleAssignVisitor<'tcx> {
    tcx: TyCtxt<'tcx>,
    tuple: Local,
    idx: FieldIdx,
    ty: Ty<'tcx>,
    span: Span,
    copy_size_local: Local,
    src_ptr_local: Local,
    return_ty: Ty<'tcx>,
    tuple_ret_local: Local,
}

impl<'tcx> MutVisitor<'tcx> for TupleAssignVisitor<'tcx> {
    fn tcx(&self) -> TyCtxt<'tcx> {
        self.tcx
    }

    fn visit_assign(
        &mut self,
        place: &mut Place<'tcx>,
        rvalue: &mut Rvalue<'tcx>,
        location: Location,
    ) {
        if place.local == RETURN_PLACE {
            if place.projection.len() > 0 {
                panic!("return place with projections are not implemented");
            }
        }
    }

    /*
    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &mut BasicBlockData<'tcx>) {
        if let Some(term) = data.terminator.as_ref() {
            match term.kind {
                TerminatorKind::Return => {
                    //get copy size
                    let assign_size = Statement {
                        source_info: SourceInfo::outermost(self.span),
                        kind: StatementKind::Assign(Box::new((
                            Place::from(self.copy_size_local),
                            Rvalue::NullaryOp(rustc_middle::mir::NullOp::SizeOf, self.return_ty),
                        ))),
                    };

                    // get src ptr
                    let address_of = Statement {
                        source_info: SourceInfo::outermost(self.span),
                        kind: StatementKind::Assign(Box::new((
                            Place::from(self.src_ptr_local),
                            Rvalue::AddressOf(Mutability::Not, Place::from(RETURN_PLACE)),
                        ))),
                    };

                    let mut tuple_ret = Place::from(self.tuple);
                    let projs = self
                        .tcx
                        .mk_place_elems(&[
                            ProjectionElem::Deref,
                            ProjectionElem::Field(self.idx, self.ty),
                        ])
                        .to_vec();
                    tuple_ret.projection = self.tcx.mk_place_elems(&projs);
                    // memcpy
                    let copy_bytes = Statement {
                        source_info: SourceInfo::outermost(self.span),
                        kind: StatementKind::Intrinsic(Box::new(
                            rustc_middle::mir::NonDivergingIntrinsic::CopyNonOverlapping(
                                CopyNonOverlapping {
                                    src: Operand::Copy(Place::from(self.src_ptr_local)),
                                    dst: Operand::Copy(tuple_ret),
                                    count: Operand::Copy(Place::from(self.copy_size_local)),
                                },
                            ),
                        )),
                    };

                    data.statements.push(assign_size);
                    data.statements.push(address_of);
                    data.statements.push(copy_bytes);
                }
                _ => {}
            }
        }
    }
    */
    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &mut BasicBlockData<'tcx>) {
        let assign_to_return = data.statements.iter_mut().filter(|stmt| match &stmt.kind {
            StatementKind::Assign(s) if s.0.local == RETURN_PLACE => {
                if s.0.projection.len() > 0 {
                    panic!("return place with projections are not implemented");
                } else {
                    true
                }
            }
            _ => false,
        });

        assign_to_return.for_each(|stmt| {
            let mut tuple_ret = Place::from(self.tuple_ret_local);
            let projs = self.tcx.mk_place_elems(&[ProjectionElem::Deref]).to_vec();
            tuple_ret.projection = self.tcx.mk_place_elems(&projs);
            let rval = stmt.kind.as_assign().unwrap().1.clone();
            stmt.kind = StatementKind::Assign(Box::new((tuple_ret, rval)));
        });
    }
}
