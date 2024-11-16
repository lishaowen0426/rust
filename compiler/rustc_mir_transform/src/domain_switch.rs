#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
use crate::duplication_rewrite::create_tuple_parameter_ty;
use crate::MirPass;
use rustc_ast::Mutability;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use rustc_hir::def_id::{DefId, LocalDefId};
use rustc_index::IndexVec;
use rustc_middle::mir::visit::{MutVisitor, PlaceContext};
use rustc_middle::mir::CastKind;
use rustc_middle::mir::{
    AggregateKind, BasicBlock, BasicBlockData, BasicBlocks, Body, BorrowKind, CallSource, Const,
    ConstOperand, HasLocalDecls, Local, LocalDecl, Location, MutBorrowKind, Operand, Place,
    ProjectionElem, Rvalue, SourceInfo, Statement, StatementKind, Terminator, TerminatorKind,
    UnwindAction,
};
use rustc_middle::ty::GenericArgs;
use rustc_middle::ty::{
    self, adjustment::PointerCoercion, GenericParamDefKind, Ty, TyCtxt, TypeVisitableExt, UintTy,
};
use rustc_span::source_map::dummy_spanned;
use rustc_target::abi::FieldIdx;

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
        let def_id = body.source.def_id().expect_local();

        if !tcx.reachable_set(()).contains(&def_id) {
            //skip items inaccessible to other crates
            return;
        }

        let (duplicate_map, _) = tcx.duplicate_map(());
        if let Some(duplicate_to_def_id) = duplicate_map.get(&def_id) {
            debug!("{:?} was duplicated to {:?}", def_id, duplicate_to_def_id);
            self.replace_body(tcx, body, def_id, *duplicate_to_def_id);
        }
    }
}

fn create_cast_to_ptr<'tcx>(
    tcx: TyCtxt<'tcx>,
    tuple: Local,
    trans_to_ptr: DefId,
    duplicate_to: LocalDefId,
    body: &mut Body<'tcx>,
    target: usize,
) -> (Local, Local, BasicBlockData<'tcx>) {
    let body_span = body.span;

    let (callee_ptr, fn_ptr_cast, callee_ptr_cast) = {
        let callee_sig = tcx.fn_sig(duplicate_to.to_def_id()).instantiate_identity();

        let generic_args =
            GenericArgs::for_item(tcx, duplicate_to.to_def_id(), |param, _| match param.kind {
                GenericParamDefKind::Lifetime => tcx.lifetimes.re_erased.into(),
                _ => tcx.mk_param_from_def(param),
            });
        let func_ty = Ty::new_fn_def(tcx, duplicate_to.to_def_id(), generic_args);
        //create callee fn ptr
        let fn_ptr =
            body.local_decls.push(LocalDecl::new(Ty::new_fn_ptr(tcx, callee_sig), body_span));
        let callee_ptr = body
            .local_decls
            .push(LocalDecl::new(Ty::new_mut_ptr(tcx, Ty::new_uint(tcx, UintTy::U8)), body_span));
        let fn_ptr_cast = Statement {
            source_info: SourceInfo::outermost(body_span),
            kind: StatementKind::Assign(Box::new((
                Place::from(fn_ptr),
                Rvalue::Cast(
                    CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer),
                    Operand::Constant(Box::new(ConstOperand {
                        span: body_span,
                        user_ty: None,
                        const_: Const::zero_sized(func_ty),
                    })),
                    Ty::new_fn_ptr(tcx, callee_sig),
                ),
            ))),
        };

        let callee_ptr_cast = Statement {
            source_info: SourceInfo::outermost(body_span),
            kind: StatementKind::Assign(Box::new((
                Place::from(callee_ptr),
                Rvalue::Cast(
                    CastKind::FnPtrToPtr,
                    Operand::Move(Place::from(fn_ptr)),
                    Ty::new_mut_ptr(tcx, Ty::new_uint(tcx, UintTy::U8)),
                ),
            ))),
        };
        (callee_ptr, fn_ptr_cast, callee_ptr_cast)
    };

    let tuple_ty = body.local_decls[tuple].ty;
    let tuple_fields = body.args_iter().map(|loc| Operand::Copy(Place::from(loc))).collect();

    let tuple_init_stmt = Statement {
        source_info: SourceInfo::outermost(body_span),
        kind: StatementKind::Assign(Box::new((
            Place::from(tuple),
            Rvalue::Aggregate(Box::new(AggregateKind::Tuple), tuple_fields),
        ))),
    };

    let tuple_ref = body.local_decls.push(LocalDecl::new(
        Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, body.local_decls[tuple].ty),
        body_span,
    ));

    let tuple_ptr = body
        .local_decls
        .push(LocalDecl::new(Ty::new_mut_ptr(tcx, Ty::new_uint(tcx, UintTy::U8)), body_span));

    let tuple_cast_stmt = Statement {
        source_info: SourceInfo::outermost(body_span),
        kind: StatementKind::Assign(Box::new((
            Place::from(tuple_ref),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place::from(tuple),
            ),
        ))),
    };

    let inst_args = GenericArgs::for_item(tcx, trans_to_ptr, |param, _| match param.kind {
        GenericParamDefKind::Lifetime => tcx.lifetimes.re_erased.into(),
        GenericParamDefKind::Type { .. } => tuple_ty.into(),
        _ => panic!("transmute_to_ptr has wrong generics"),
    });

    let func = Operand::function_handle(tcx, trans_to_ptr, inst_args, body_span);

    let cast_block = BasicBlockData {
        statements: vec![fn_ptr_cast, callee_ptr_cast, tuple_init_stmt, tuple_cast_stmt],
        terminator: Some(Terminator {
            source_info: SourceInfo::outermost(body_span),
            kind: TerminatorKind::Call {
                func,
                args: vec![dummy_spanned(Operand::Copy(Place::from(tuple_ref)))],
                destination: Place::from(tuple_ptr),
                target: Some(BasicBlock::from_usize(target)),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Normal,
                fn_span: body_span,
            },
        }),
        is_cleanup: false,
    };
    (tuple_ptr, callee_ptr, cast_block)
}

fn create_dup_call_blk<'tcx>(
    tcx: TyCtxt<'tcx>,
    duplicate_from: LocalDefId,
    duplicate_to: LocalDefId,
    tuple_ptr: Local,
    body: &mut Body<'tcx>,
    target: usize,
) -> (Local, BasicBlockData<'tcx>) {
    let body_span = body.span;
    //since we are inside the EarlyBinder
    let fn_sig = tcx.fn_sig(duplicate_to.to_def_id()).instantiate_identity();
    let output_ty = tcx.instantiate_bound_regions_with_erased(fn_sig.output());
    let output_local = body.local_decls.push(LocalDecl::new(output_ty, body_span));

    let generic_args =
        GenericArgs::for_item(tcx, duplicate_to.to_def_id(), |param, _| match param.kind {
            GenericParamDefKind::Lifetime => tcx.lifetimes.re_erased.into(),
            _ => tcx.mk_param_from_def(param),
        });
    let func = Operand::function_handle(tcx, duplicate_to.to_def_id(), generic_args, body_span);

    let mut args = vec![dummy_spanned(Operand::Copy(Place::from(tuple_ptr)))];
    args.append(
        &mut (body.args_iter().map(|l| dummy_spanned(Operand::Copy(Place::from(l)))).collect()),
    );

    let call_block = BasicBlockData {
        statements: vec![],
        terminator: Some(Terminator {
            source_info: SourceInfo::outermost(body_span),
            kind: TerminatorKind::Call {
                func,
                args,
                destination: Place::from(output_local),
                target: Some(BasicBlock::from_usize(target)),
                unwind: UnwindAction::Continue,
                call_source: CallSource::Normal,
                fn_span: body_span,
            },
        }),
        is_cleanup: false,
    };

    (output_local, call_block)
}

impl DomainSwitch {
    fn replace_body<'tcx>(
        &self,
        tcx: TyCtxt<'tcx>,
        body: &mut Body<'tcx>,
        duplicate_from: LocalDefId,
        duplicate_to: LocalDefId,
    ) {
        let body_span = body.span;
        let mut new_bb: IndexVec<BasicBlock, BasicBlockData<'tcx>> = IndexVec::new();

        let tuple_ty = create_tuple_parameter_ty(tcx, body, false);
        debug!("tuple typ: {:?}", tuple_ty);
        let tuple_loc = body.local_decls.push(LocalDecl::new(tuple_ty, body_span));

        let trans_to_ptr = tcx.lang_items().transmute_to_pointer().unwrap();
        let (tuple_ptr, callee_ptr, cast_block) =
            create_cast_to_ptr(tcx, tuple_loc, trans_to_ptr, duplicate_to, body, 1usize);
        debug!("cast_block:{:?}", cast_block);
        let (dup_ret, call_block) =
            create_dup_call_blk(tcx, duplicate_from, duplicate_to, tuple_ptr, body, 2usize);
        new_bb.push(cast_block);
        new_bb.push(call_block);
        /*
         */
        /*

        let args = body.args_iter().map(|a| dummy_spanned(Operand::Copy(Place::from(a)))).collect();

        let call_block = BasicBlockData {
            statements: vec![],
            terminator: Some(Terminator {
                source_info: SourceInfo::outermost(body_span),
                kind: TerminatorKind::Call {
                    func,
                    args,
                    destination: Place::from(RETURN_LOCAL),
                    target: Some(BasicBlock::from_usize(1usize)),
                    unwind: UnwindAction::Continue,
                    call_source: CallSource::Normal,
                    fn_span: body_span,
                },
            }),
            is_cleanup: false,
        };
        new_bb.push(call_block);

        */

        let return_block = BasicBlockData {
            statements: vec![Statement {
                source_info: SourceInfo::outermost(body_span),
                kind: StatementKind::Assign(Box::new((
                    Place::from(RETURN_LOCAL),
                    Rvalue::Use(Operand::Move(Place::from(dup_ret))),
                ))),
            }],
            terminator: Some(Terminator {
                source_info: SourceInfo::outermost(body_span),
                kind: TerminatorKind::Return,
            }),
            is_cleanup: false,
        };
        new_bb.push(return_block);
        body.basic_blocks = BasicBlocks::new(new_bb);
    }
}
