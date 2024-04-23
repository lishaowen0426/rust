//! Lowers intrinsic calls

use rustc_middle::mir::*;
use rustc_middle::ty::{self, TyCtxt};
use rustc_span::symbol::sym;

pub struct LowerIntrinsics;

impl<'tcx> MirPass<'tcx> for LowerIntrinsics {
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        let local_decls = &body.local_decls;
        for block in body.basic_blocks.as_mut() {
            let terminator = block.terminator.as_mut().unwrap();
            if let TerminatorKind::Call { func, args, destination, target, .. } =
                &mut terminator.kind
                && let ty::FnDef(def_id, generic_args) = *func.ty(local_decls, tcx).kind()
                && let Some(intrinsic_name) = tcx.intrinsic(def_id)
            {
                match intrinsic_name {
                    sym::unreachable => {
                        terminator.kind = TerminatorKind::Unreachable;
                    }
                    sym::debug_assertions => {
                        let target = target.unwrap();
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                *destination,
                                Rvalue::NullaryOp(NullOp::DebugAssertions, tcx.types.bool),
                            ))),
                            safety: terminator.safety,
                        });
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::forget => {
                        if let Some(target) = *target {
                            block.statements.push(Statement {
                                source_info: terminator.source_info,
                                kind: StatementKind::Assign(Box::new((
                                    *destination,
                                    Rvalue::Use(Operand::Constant(Box::new(ConstOperand {
                                        span: terminator.source_info.span,
                                        user_ty: None,
                                        const_: Const::zero_sized(tcx.types.unit),
                                    }))),
                                ))),
                                safety: terminator.safety,
                            });
                            terminator.kind = TerminatorKind::Goto { target };
                        }
                    }
                    sym::copy_nonoverlapping => {
                        let target = target.unwrap();
                        let mut args = args.drain(..);
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Intrinsic(Box::new(
                                NonDivergingIntrinsic::CopyNonOverlapping(
                                    rustc_middle::mir::CopyNonOverlapping {
                                        src: args.next().unwrap().node,
                                        dst: args.next().unwrap().node,
                                        count: args.next().unwrap().node,
                                    },
                                ),
                            )),
                            safety: terminator.safety,
                        });
                        assert_eq!(
                            args.next(),
                            None,
                            "Extra argument for copy_non_overlapping intrinsic"
                        );
                        drop(args);
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::assume => {
                        let target = target.unwrap();
                        let mut args = args.drain(..);
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Intrinsic(Box::new(
                                NonDivergingIntrinsic::Assume(args.next().unwrap().node),
                            )),
                            safety: terminator.safety,
                        });
                        assert_eq!(
                            args.next(),
                            None,
                            "Extra argument for copy_non_overlapping intrinsic"
                        );
                        drop(args);
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::wrapping_add
                    | sym::wrapping_sub
                    | sym::wrapping_mul
                    | sym::unchecked_add
                    | sym::unchecked_sub
                    | sym::unchecked_mul
                    | sym::unchecked_div
                    | sym::unchecked_rem
                    | sym::unchecked_shl
                    | sym::unchecked_shr => {
                        let target = target.unwrap();
                        let lhs;
                        let rhs;
                        {
                            let mut args = args.drain(..);
                            lhs = args.next().unwrap();
                            rhs = args.next().unwrap();
                        }
                        let bin_op = match intrinsic_name {
                            sym::wrapping_add => BinOp::Add,
                            sym::wrapping_sub => BinOp::Sub,
                            sym::wrapping_mul => BinOp::Mul,
                            sym::unchecked_add => BinOp::AddUnchecked,
                            sym::unchecked_sub => BinOp::SubUnchecked,
                            sym::unchecked_mul => BinOp::MulUnchecked,
                            sym::unchecked_div => BinOp::Div,
                            sym::unchecked_rem => BinOp::Rem,
                            sym::unchecked_shl => BinOp::ShlUnchecked,
                            sym::unchecked_shr => BinOp::ShrUnchecked,
                            _ => bug!("unexpected intrinsic"),
                        };
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                *destination,
                                Rvalue::BinaryOp(bin_op, Box::new((lhs.node, rhs.node))),
                            ))),
                            safety: terminator.safety,
                        });
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::add_with_overflow | sym::sub_with_overflow | sym::mul_with_overflow => {
                        if let Some(target) = *target {
                            let lhs;
                            let rhs;
                            {
                                let mut args = args.drain(..);
                                lhs = args.next().unwrap();
                                rhs = args.next().unwrap();
                            }
                            let bin_op = match intrinsic_name {
                                sym::add_with_overflow => BinOp::Add,
                                sym::sub_with_overflow => BinOp::Sub,
                                sym::mul_with_overflow => BinOp::Mul,
                                _ => bug!("unexpected intrinsic"),
                            };
                            block.statements.push(Statement {
                                source_info: terminator.source_info,
                                kind: StatementKind::Assign(Box::new((
                                    *destination,
                                    Rvalue::CheckedBinaryOp(bin_op, Box::new((lhs.node, rhs.node))),
                                ))),
                                safety: terminator.safety,
                            });
                            terminator.kind = TerminatorKind::Goto { target };
                        }
                    }
                    sym::size_of | sym::min_align_of => {
                        if let Some(target) = *target {
                            let tp_ty = generic_args.type_at(0);
                            let null_op = match intrinsic_name {
                                sym::size_of => NullOp::SizeOf,
                                sym::min_align_of => NullOp::AlignOf,
                                _ => bug!("unexpected intrinsic"),
                            };
                            block.statements.push(Statement {
                                source_info: terminator.source_info,
                                kind: StatementKind::Assign(Box::new((
                                    *destination,
                                    Rvalue::NullaryOp(null_op, tp_ty),
                                ))),
                                safety: terminator.safety,
                            });
                            terminator.kind = TerminatorKind::Goto { target };
                        }
                    }
                    sym::read_via_copy => {
                        let [arg] = args.as_slice() else {
                            span_bug!(terminator.source_info.span, "Wrong number of arguments");
                        };
                        let derefed_place = if let Some(place) = arg.node.place()
                            && let Some(local) = place.as_local()
                        {
                            tcx.mk_place_deref(local.into())
                        } else {
                            span_bug!(
                                terminator.source_info.span,
                                "Only passing a local is supported"
                            );
                        };
                        // Add new statement at the end of the block that does the read, and patch
                        // up the terminator.
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                *destination,
                                Rvalue::Use(Operand::Copy(derefed_place)),
                            ))),
                            safety: terminator.safety,
                        });
                        terminator.kind = match *target {
                            None => {
                                // No target means this read something uninhabited,
                                // so it must be unreachable.
                                TerminatorKind::Unreachable
                            }
                            Some(target) => TerminatorKind::Goto { target },
                        }
                    }
                    sym::write_via_move => {
                        let target = target.unwrap();
                        let Ok([ptr, val]) = <[_; 2]>::try_from(std::mem::take(args)) else {
                            span_bug!(
                                terminator.source_info.span,
                                "Wrong number of arguments for write_via_move intrinsic",
                            );
                        };
                        let derefed_place = if let Some(place) = ptr.node.place()
                            && let Some(local) = place.as_local()
                        {
                            tcx.mk_place_deref(local.into())
                        } else {
                            span_bug!(
                                terminator.source_info.span,
                                "Only passing a local is supported"
                            );
                        };
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                derefed_place,
                                Rvalue::Use(val.node),
                            ))),
                            safety: terminator.safety,
                        });
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::discriminant_value => {
                        if let (Some(target), Some(arg)) = (*target, args[0].node.place()) {
                            let arg = tcx.mk_place_deref(arg);
                            block.statements.push(Statement {
                                source_info: terminator.source_info,
                                kind: StatementKind::Assign(Box::new((
                                    *destination,
                                    Rvalue::Discriminant(arg),
                                ))),
                                safety: terminator.safety,
                            });
                            terminator.kind = TerminatorKind::Goto { target };
                        }
                    }
                    sym::offset => {
                        let target = target.unwrap();
                        let Ok([ptr, delta]) = <[_; 2]>::try_from(std::mem::take(args)) else {
                            span_bug!(
                                terminator.source_info.span,
                                "Wrong number of arguments for offset intrinsic",
                            );
                        };
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                *destination,
                                Rvalue::BinaryOp(BinOp::Offset, Box::new((ptr.node, delta.node))),
                            ))),
                            safety: terminator.safety,
                        });
                        terminator.kind = TerminatorKind::Goto { target };
                    }
                    sym::transmute | sym::transmute_unchecked => {
                        let dst_ty = destination.ty(local_decls, tcx).ty;
                        let Ok([arg]) = <[_; 1]>::try_from(std::mem::take(args)) else {
                            span_bug!(
                                terminator.source_info.span,
                                "Wrong number of arguments for transmute intrinsic",
                            );
                        };

                        // Always emit the cast, even if we transmute to an uninhabited type,
                        // because that lets CTFE and codegen generate better error messages
                        // when such a transmute actually ends up reachable.
                        block.statements.push(Statement {
                            source_info: terminator.source_info,
                            kind: StatementKind::Assign(Box::new((
                                *destination,
                                Rvalue::Cast(CastKind::Transmute, arg.node, dst_ty),
                            ))),
                            safety: terminator.safety,
                        });

                        if let Some(target) = *target {
                            terminator.kind = TerminatorKind::Goto { target };
                        } else {
                            terminator.kind = TerminatorKind::Unreachable;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
