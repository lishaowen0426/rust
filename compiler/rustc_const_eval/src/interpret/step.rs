//! This module contains the `InterpCx` methods for executing a single step of the interpreter.
//!
//! The main entry point is the `step` method.

use either::Either;

use rustc_index::IndexSlice;
use rustc_middle::mir;
use rustc_middle::ty::layout::LayoutOf;
use rustc_target::abi::{FieldIdx, FIRST_VARIANT};

use super::operand::Operand;
use super::{ImmTy, InterpCx, InterpResult, Machine, OpTy, PlaceTy, Projectable, Scalar};
use crate::util;

impl<'mir, 'tcx: 'mir, M: Machine<'mir, 'tcx>> InterpCx<'mir, 'tcx, M> {
    /// Returns `true` as long as there are more things to do.
    ///
    /// This is used by [priroda](https://github.com/oli-obk/priroda)
    ///
    /// This is marked `#inline(always)` to work around adversarial codegen when `opt-level = 3`
    #[inline(always)]
    #[instrument(level = "info", name = "interp_step", skip_all)]
    pub fn step(&mut self) -> InterpResult<'tcx, bool> {
        if self.stack().is_empty() {
            return Ok(false);
        }

        let Either::Left(loc) = self.frame().loc else {
            // We are unwinding and this fn has no cleanup code.
            // Just go on unwinding.
            trace!("unwinding: skipping frame");
            self.pop_stack_frame(/* unwinding */ true)?;
            return Ok(true);
        };
        let basic_block = &self.body().basic_blocks[loc.block];
        info!("body source: {:?}", self.body().source);

        if let Some(stmt) = basic_block.statements.get(loc.statement_index) {
            let old_frames = self.frame_idx();
            if self.is_crate_unsafe_target() {
                self.statement_debug(stmt)?;
            } else {
                self.statement(stmt)?;
            }
            // Make sure we are not updating `statement_index` of the wrong frame.
            assert_eq!(old_frames, self.frame_idx());
            // Advance the program counter.
            self.frame_mut().loc.as_mut().left().unwrap().statement_index += 1;
            return Ok(true);
        }

        M::before_terminator(self)?;

        let terminator = basic_block.terminator();
        if self.is_crate_unsafe_target() {
            self.terminator_debug(terminator)?;
        } else {
            self.terminator(terminator)?;
        }
        Ok(true)
    }

    pub fn is_statement_unsafe(&self, stmt: &mir::Statement<'tcx>) -> bool {
        self.body().source_scopes[stmt.source_info.scope].miri_is_unsafe
    }

    pub fn is_terminator_unsafe(&self, terminator: &mir::Terminator<'tcx>) -> bool {
        self.body().source_scopes[terminator.source_info.scope].miri_is_unsafe
    }

    #[instrument(level = "info", skip_all)]
    fn miri_debug_stmt(&self, stmt: &mir::Statement<'tcx>) -> InterpResult<'tcx> {
        use rustc_middle::mir::StatementKind::*;
        let is_target_crate = self.is_crate_unsafe_target();
        if !is_target_crate {
            return Ok(());
        }

        info!(stmt=?stmt);
        match &stmt.kind {
            Assign(box (place, _)) => {
                let decl = &self.body().local_decls[place.local];
                self.print_loc_alloc_id_helper(place.local, decl)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn is_crate_unsafe_target(&self) -> bool {
        self.unsafety_tracking_crates.contains(&self.body().source.def_id().krate)
    }

    #[instrument(level = "info", name = "eval_statement_debug", skip(self))]
    pub fn statement_debug(&mut self, stmt: &mir::Statement<'tcx>) -> InterpResult<'tcx> {
        use rustc_middle::mir::StatementKind::*;
        let is_target_crate = self.is_crate_unsafe_target();
        let is_stmt_unsafe = self.is_statement_unsafe(stmt);

        self.miri_debug_stmt(stmt)?;

        match &stmt.kind {
            Assign(box (place, rvalue)) => {
                info!("Assign:");
                self.eval_rvalue_into_place(rvalue, *place, is_stmt_unsafe, is_target_crate)?
            }

            SetDiscriminant { place, variant_index } => {
                //this is used internally by rustc to change the active enum variant
                //so this does not inherently unsafe.
                //e.g.,
                //  SetDiscriminant { place: _1, variant_index: 1 }
                //  (_1.0 = 42)
                //the unsafety will be handled in subsequent statements (here, the Assign)
                let dest = self.eval_place(**place)?;
                self.write_discriminant(*variant_index, &dest)?;
            }

            Deinit(place) => {
                //this is used internally by the compiler
                //so no effects
                let dest = self.eval_place(**place)?;
                self.write_uninit(&dest)?;
            }

            // Mark locals as alive
            StorageLive(local) => {
                if is_stmt_unsafe && is_target_crate {
                    let def_id = self.body().source.def_id();
                    self.mark_unsafe_local(def_id, *local);
                }
                self.storage_live(*local)?;
            }

            // Mark locals as dead
            StorageDead(local) => {
                self.storage_dead(*local)?;
            }

            // No dynamic semantics attached to `FakeRead`; MIR
            // interpreter is solely intended for borrowck'ed code.
            FakeRead(..) => {}

            // Stacked Borrows.
            Retag(kind, place) => {
                let dest = self.eval_place(**place)?;
                M::retag_place_contents(self, *kind, &dest)?;
            }

            Intrinsic(box intrinsic) => self.emulate_nondiverging_intrinsic(intrinsic)?,

            // Evaluate the place expression, without reading from it.
            PlaceMention(box place) => {
                let _ = self.eval_place(*place)?;
            }

            // This exists purely to guide borrowck lifetime inference, and does not have
            // an operational effect.
            AscribeUserType(..) => {}

            // Currently, Miri discards Coverage statements. Coverage statements are only injected
            // via an optional compile time MIR pass and have no side effects. Since Coverage
            // statements don't exist at the source level, it is safe for Miri to ignore them, even
            // for undefined behavior (UB) checks.
            //
            // A coverage counter inside a const expression (for example, a counter injected in a
            // const function) is discarded when the const is evaluated at compile time. Whether
            // this should change, and/or how to implement a const eval counter, is a subject of the
            // following issue:
            //
            // FIXME(#73156): Handle source code coverage in const eval
            Coverage(..) => {}

            ConstEvalCounter => {
                M::increment_const_eval_counter(self)?;
            }

            // Defined to do nothing. These are added by optimization passes, to avoid changing the
            // size of MIR constantly.
            Nop => {}
        }

        Ok(())
    }

    /// Runs the interpretation logic for the given `mir::Statement` at the current frame and
    /// statement counter.
    ///
    /// This does NOT move the statement counter forward, the caller has to do that!
    #[instrument(level = "info", name = "eval_statement", skip(self))]
    pub fn statement(&mut self, stmt: &mir::Statement<'tcx>) -> InterpResult<'tcx> {
        use rustc_middle::mir::StatementKind::*;
        let is_target_crate = self.is_crate_unsafe_target();
        let is_stmt_unsafe = self.is_statement_unsafe(stmt);

        match &stmt.kind {
            Assign(box (place, rvalue)) => {
                self.eval_rvalue_into_place(rvalue, *place, is_stmt_unsafe, is_target_crate)?
            }

            SetDiscriminant { place, variant_index } => {
                //this is used internally by rustc to change the active enum variant
                //so this does not inherently unsafe.
                //e.g.,
                //  SetDiscriminant { place: _1, variant_index: 1 }
                //  (_1.0 = 42)
                //the unsafety will be handled in subsequent statements (here, the Assign)
                let dest = self.eval_place(**place)?;
                self.write_discriminant(*variant_index, &dest)?;
            }

            Deinit(place) => {
                //this is used internally by the compiler
                //so no effects
                let dest = self.eval_place(**place)?;
                self.write_uninit(&dest)?;
            }

            // Mark locals as alive
            StorageLive(local) => {
                if is_stmt_unsafe && is_target_crate {
                    let def_id = self.body().source.def_id();
                    self.mark_unsafe_local(def_id, *local);
                }
                self.storage_live(*local)?;
            }

            // Mark locals as dead
            StorageDead(local) => {
                self.storage_dead(*local)?;
            }

            // No dynamic semantics attached to `FakeRead`; MIR
            // interpreter is solely intended for borrowck'ed code.
            FakeRead(..) => {}

            // Stacked Borrows.
            Retag(kind, place) => {
                let dest = self.eval_place(**place)?;
                M::retag_place_contents(self, *kind, &dest)?;
            }

            Intrinsic(box intrinsic) => self.emulate_nondiverging_intrinsic(intrinsic)?,

            // Evaluate the place expression, without reading from it.
            PlaceMention(box place) => {
                let _ = self.eval_place(*place)?;
            }

            // This exists purely to guide borrowck lifetime inference, and does not have
            // an operational effect.
            AscribeUserType(..) => {}

            // Currently, Miri discards Coverage statements. Coverage statements are only injected
            // via an optional compile time MIR pass and have no side effects. Since Coverage
            // statements don't exist at the source level, it is safe for Miri to ignore them, even
            // for undefined behavior (UB) checks.
            //
            // A coverage counter inside a const expression (for example, a counter injected in a
            // const function) is discarded when the const is evaluated at compile time. Whether
            // this should change, and/or how to implement a const eval counter, is a subject of the
            // following issue:
            //
            // FIXME(#73156): Handle source code coverage in const eval
            Coverage(..) => {}

            ConstEvalCounter => {
                M::increment_const_eval_counter(self)?;
            }

            // Defined to do nothing. These are added by optimization passes, to avoid changing the
            // size of MIR constantly.
            Nop => {}
        }

        Ok(())
    }

    pub fn mark_place_unsafe(&mut self, place: &PlaceTy<'tcx, M::Provenance>) {
        use crate::interpret::place::Place;

        let def_id = self.body().source.def_id();
        match place.place() {
            Place::Ptr(mplace) => {
                if let Ok((alloc_id, _, _)) = self.ptr_get_alloc_id(mplace.ptr) {
                    self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                    self.frame_mut().mark_alloc_id_unsafe(alloc_id);
                }
            }
            Place::Local { local, .. } => {
                if let Ok(op) = self.frame().locals[*local].access() {
                    match op {
                        Operand::Indirect(mplace) => {
                            if let Ok((alloc_id, _, _)) = self.ptr_get_alloc_id(mplace.ptr) {
                                self.frame_mut().mark_alloc_id_unsafe(alloc_id);
                            }
                        }
                        _ => {}
                    }
                }
                self.mark_unsafe_local(def_id, *local);
            }
        }
    }
    pub fn is_rvalue_op_unsafe(&self, op: &OpTy<'tcx, M::Provenance>) -> bool {
        if self.is_crate_unsafe_target() {
            match op.op() {
                Operand::Immediate(_imm) => false,
                Operand::Indirect(mplace) => {
                    if let Ok((alloc_id, _, _)) = self.ptr_get_alloc_id(mplace.ptr) {
                        self.frame().is_alloc_id_unsafe(alloc_id)
                    } else {
                        false
                    }
                }
            }
        } else {
            false
        }
    }
    /// Evaluate an assignment statement.
    ///
    /// There is no separate `eval_rvalue` function. Instead, the code for handling each rvalue
    /// type writes its results directly into the memory specified by the place.
    pub fn eval_rvalue_into_place(
        &mut self,
        rvalue: &mir::Rvalue<'tcx>,
        place: mir::Place<'tcx>,
        is_stmt_unsafe: bool,
        is_target_crate: bool,
    ) -> InterpResult<'tcx> {
        let dest = self.eval_place(place)?;

        // FIXME: ensure some kind of non-aliasing between LHS and RHS?
        // Also see https://github.com/rust-lang/rust/issues/68364.
        use rustc_middle::mir::Rvalue::*;
        let is_rvalue_unsafe = match *rvalue {
            ThreadLocalRef(did) => {
                let ptr = M::thread_local_static_base_pointer(self, did)?;
                self.write_pointer(ptr, &dest)?;
                false
            }

            Use(ref operand) => {
                // Avoid recomputing the layout
                let op = self.eval_operand(operand, Some(dest.layout))?;
                self.copy_op(&op, &dest)?;
                self.is_rvalue_op_unsafe(&op)
            }

            CopyForDeref(place) => {
                let op = self.eval_place_to_op(place, Some(dest.layout))?;
                self.copy_op(&op, &dest)?;
                self.is_rvalue_op_unsafe(&op)
            }

            BinaryOp(bin_op, box (ref left, ref right)) => {
                let layout = util::binop_left_homogeneous(bin_op).then_some(dest.layout);
                let left = self.read_immediate(&self.eval_operand(left, layout)?)?;
                let layout = util::binop_right_homogeneous(bin_op).then_some(left.layout);
                let right = self.read_immediate(&self.eval_operand(right, layout)?)?;
                self.binop_ignore_overflow(bin_op, &left, &right, &dest)?;
                false
            }

            CheckedBinaryOp(bin_op, box (ref left, ref right)) => {
                // Due to the extra boolean in the result, we can never reuse the `dest.layout`.
                let left = self.read_immediate(&self.eval_operand(left, None)?)?;
                let layout = util::binop_right_homogeneous(bin_op).then_some(left.layout);
                let right = self.read_immediate(&self.eval_operand(right, layout)?)?;
                self.binop_with_overflow(bin_op, &left, &right, &dest)?;
                false
            }

            UnaryOp(un_op, ref operand) => {
                // The operand always has the same type as the result.
                let val = self.read_immediate(&self.eval_operand(operand, Some(dest.layout))?)?;
                let val = self.wrapping_unary_op(un_op, &val)?;
                assert_eq!(val.layout, dest.layout, "layout mismatch for result of {un_op:?}");
                self.write_immediate(*val, &dest)?;
                false
            }

            Aggregate(box ref kind, ref operands) => self.write_aggregate(kind, operands, &dest)?,

            Repeat(ref operand, _) => self.write_repeat(operand, &dest)?,

            Len(place) => {
                let src = self.eval_place(place)?;
                let len = src.len(self)?;
                self.write_scalar(Scalar::from_target_usize(len, self), &dest)?;
                false
            }

            Ref(_, borrow_kind, place) => {
                let src = self.eval_place(place)?;
                if self.is_crate_unsafe_target() {
                    info!("Ref: refered place: ");
                    let _ = self.dump_place(&src);
                }
                let place = self.force_allocation(&src)?;
                if self.is_crate_unsafe_target() {
                    info!("After force_allocation: {:?}", place);
                }
                let val = ImmTy::from_immediate(place.to_ref(self), dest.layout);
                // A fresh reference was created, make sure it gets retagged.
                let val = M::retag_ptr_value(
                    self,
                    if borrow_kind.allows_two_phase_borrow() {
                        mir::RetagKind::TwoPhase
                    } else {
                        mir::RetagKind::Default
                    },
                    &val,
                )?;
                self.write_immediate(*val, &dest)?;
                false
            }

            AddressOf(_, place) => {
                // Figure out whether this is an addr_of of an already raw place.
                let place_base_raw = if place.is_indirect_first_projection() {
                    let ty = self.frame().body.local_decls[place.local].ty;
                    ty.is_unsafe_ptr()
                } else {
                    // Not a deref, and thus not raw.
                    false
                };

                let src = self.eval_place(place)?;
                let place = self.force_allocation(&src)?;
                let mut val = ImmTy::from_immediate(place.to_ref(self), dest.layout);
                if !place_base_raw {
                    // If this was not already raw, it needs retagging.
                    val = M::retag_ptr_value(self, mir::RetagKind::Raw, &val)?;
                }
                self.write_immediate(*val, &dest)?;
                false
            }

            NullaryOp(ref null_op, ty) => {
                let ty = self.instantiate_from_current_frame_and_normalize_erasing_regions(ty)?;
                let layout = self.layout_of(ty)?;
                if let mir::NullOp::SizeOf | mir::NullOp::AlignOf = null_op
                    && layout.is_unsized()
                {
                    span_bug!(
                        self.frame().current_span(),
                        "{null_op:?} MIR operator called for unsized type {ty}",
                    );
                }
                let val = match null_op {
                    mir::NullOp::SizeOf => {
                        let val = layout.size.bytes();
                        Scalar::from_target_usize(val, self)
                    }
                    mir::NullOp::AlignOf => {
                        let val = layout.align.abi.bytes();
                        Scalar::from_target_usize(val, self)
                    }
                    mir::NullOp::OffsetOf(fields) => {
                        let val = layout.offset_of_subfield(self, fields.iter()).bytes();
                        Scalar::from_target_usize(val, self)
                    }
                    mir::NullOp::DebugAssertions => {
                        // The checks hidden behind this are always better done by the interpreter
                        // itself, because it knows the runtime state better.
                        Scalar::from_bool(false)
                    }
                };
                self.write_scalar(val, &dest)?;
                false
            }

            ShallowInitBox(ref operand, _) => {
                let src = self.eval_operand(operand, None)?;
                let v = self.read_immediate(&src)?;
                self.write_immediate(*v, &dest)?;
                false
            }

            Cast(cast_kind, ref operand, cast_ty) => {
                let src = self.eval_operand(operand, None)?;
                let cast_ty =
                    self.instantiate_from_current_frame_and_normalize_erasing_regions(cast_ty)?;
                self.cast(&src, cast_kind, cast_ty, &dest)?;
                false
            }

            Discriminant(place) => {
                let op = self.eval_place_to_op(place, None)?;
                let variant = self.read_discriminant(&op)?;
                let discr = self.discriminant_for_variant(op.layout.ty, variant)?;
                self.write_immediate(*discr, &dest)?;
                false
            }
        };

        if (is_stmt_unsafe || is_rvalue_unsafe) && is_target_crate {
            self.mark_place_unsafe(&dest);
        }

        Ok(())
    }

    /// Writes the aggregate to the destination.
    #[instrument(skip(self), level = "trace")]
    fn write_aggregate(
        &mut self,
        kind: &mir::AggregateKind<'tcx>,
        operands: &IndexSlice<FieldIdx, mir::Operand<'tcx>>,
        dest: &PlaceTy<'tcx, M::Provenance>,
    ) -> InterpResult<'tcx, bool> {
        self.write_uninit(dest)?; // make sure all the padding ends up as uninit
        let (variant_index, variant_dest, active_field_index) = match *kind {
            mir::AggregateKind::Adt(_, variant_index, _, _, active_field_index) => {
                let variant_dest = self.project_downcast(dest, variant_index)?;
                (variant_index, variant_dest, active_field_index)
            }
            _ => (FIRST_VARIANT, dest.clone(), None),
        };
        if active_field_index.is_some() {
            assert_eq!(operands.len(), 1);
        }
        let mut is_rvalue_unsafe = false;
        for (field_index, operand) in operands.iter_enumerated() {
            let field_index = active_field_index.unwrap_or(field_index);
            let field_dest = self.project_field(&variant_dest, field_index.as_usize())?;
            let op = self.eval_operand(operand, Some(field_dest.layout))?;
            is_rvalue_unsafe |= self.is_rvalue_op_unsafe(&op);
            self.copy_op(&op, &field_dest)?;
        }
        self.write_discriminant(variant_index, dest)?;
        Ok(is_rvalue_unsafe)
    }

    /// Repeats `operand` into the destination. `dest` must have array type, and that type
    /// determines how often `operand` is repeated.
    fn write_repeat(
        &mut self,
        operand: &mir::Operand<'tcx>,
        dest: &PlaceTy<'tcx, M::Provenance>,
    ) -> InterpResult<'tcx, bool> {
        let src = self.eval_operand(operand, None)?;
        let is_rvalue_unsafe = self.is_rvalue_op_unsafe(&src);
        assert!(src.layout.is_sized());
        let dest = self.force_allocation(&dest)?;
        let length = dest.len(self)?;
        if length == 0 {
            // Nothing to copy... but let's still make sure that `dest` as a place is valid.
            self.get_place_alloc_mut(&dest)?;
        } else {
            // Write the src to the first element.
            let first = self.project_index(&dest, 0)?;
            self.copy_op(&src, &first)?;

            // This is performance-sensitive code for big static/const arrays! So we
            // avoid writing each operand individually and instead just make many copies
            // of the first element.
            let elem_size = first.layout.size;
            let first_ptr = first.ptr();
            let rest_ptr = first_ptr.offset(elem_size, self)?;
            // No alignment requirement since `copy_op` above already checked it.
            self.mem_copy_repeatedly(
                first_ptr,
                rest_ptr,
                elem_size,
                length - 1,
                /*nonoverlapping:*/ true,
            )?;
        }

        Ok(is_rvalue_unsafe)
    }
    #[instrument(level = "info", skip(self))]
    fn terminator_debug(&mut self, terminator: &mir::Terminator<'tcx>) -> InterpResult<'tcx> {
        info!("{:?}", terminator.kind);

        self.eval_terminator(terminator)?;
        if !self.stack().is_empty() {
            if let Either::Left(loc) = self.frame().loc {
                info!("// executing {:?}", loc.block);
            }
        }
        Ok(())
    }
    /// Evaluate the given terminator. Will also adjust the stack frame and statement position accordingly.
    fn terminator(&mut self, terminator: &mir::Terminator<'tcx>) -> InterpResult<'tcx> {
        info!("{:?}", terminator.kind);

        self.eval_terminator(terminator)?;
        if !self.stack().is_empty() {
            if let Either::Left(loc) = self.frame().loc {
                info!("// executing {:?}", loc.block);
            }
        }
        Ok(())
    }
}
