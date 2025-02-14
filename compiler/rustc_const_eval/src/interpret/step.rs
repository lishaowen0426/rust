//! This module contains the `InterpCx` methods for executing a single step of the interpreter.
//!
//! The main entry point is the `step` method.

use either::Either;
use rustc_abi::{BackendRepr, FieldIdx, FIRST_VARIANT};
use rustc_data_structures::fx::FxHashSet;
use rustc_index::IndexSlice;
use rustc_middle::mir::interpret::{AllocId, Pointer};
use rustc_middle::mir::Local;
use rustc_middle::ty::layout::{FnAbiOf, HasTyCtxt, LayoutOf, TyAndLayout};
use rustc_middle::ty::{self, Instance, Ty};
use rustc_middle::{bug, mir, span_bug};
use rustc_span::def_id::DefId;
use rustc_span::source_map::Spanned;
use rustc_target::abi;
use rustc_target::callconv::FnAbi;
use tracing::{info, instrument, trace};

use super::operand::Operand;
use super::{
    interp_ok, throw_ub, FnArg, FnVal, ImmTy, Immediate, InterpCx, InterpResult, MPlaceTy, Machine,
    MemPlaceMeta, PlaceTy, Projectable, Scalar,
};
use crate::interpret::OpTy;
use crate::util;

struct EvaluatedCalleeAndArgs<'tcx, M: Machine<'tcx>> {
    callee: FnVal<'tcx, M::ExtraFnVal>,
    args: Vec<FnArg<'tcx, M::Provenance>>,
    fn_sig: ty::FnSig<'tcx>,
    fn_abi: &'tcx FnAbi<'tcx, Ty<'tcx>>,
    /// True if the function is marked as `#[track_caller]` ([`ty::InstanceKind::requires_caller_location`])
    with_caller_location: bool,
}

impl<'tcx, M: Machine<'tcx>> InterpCx<'tcx, M> {
    /// Returns `true` as long as there are more things to do.
    ///
    /// This is used by [priroda](https://github.com/oli-obk/priroda)
    ///
    /// This is marked `#inline(always)` to work around adversarial codegen when `opt-level = 3`
    #[inline(always)]
    pub fn step(&mut self) -> InterpResult<'tcx, bool> {
        if self.stack().is_empty() {
            return interp_ok(false);
        }

        let Either::Left(loc) = self.frame().loc else {
            // We are unwinding and this fn has no cleanup code.
            // Just go on unwinding.
            trace!("unwinding: skipping frame");
            self.return_from_current_stack_frame(/* unwinding */ true)?;
            return interp_ok(true);
        };
        let basic_block = &self.body().basic_blocks[loc.block];

        if let Some(stmt) = basic_block.statements.get(loc.statement_index) {
            let old_frames = self.frame_idx();
            self.current_stmt_for_debug = Some(stmt.clone());
            self.eval_statement(stmt)?;
            self.current_stmt_for_debug = None;
            // Make sure we are not updating `statement_index` of the wrong frame.
            assert_eq!(old_frames, self.frame_idx());
            // Advance the program counter.
            self.frame_mut().loc.as_mut().left().unwrap().statement_index += 1;
            return interp_ok(true);
        }

        M::before_terminator(self)?;

        let terminator = basic_block.terminator();
        self.eval_terminator(terminator)?;
        if !self.stack().is_empty() {
            if let Either::Left(loc) = self.frame().loc {
                info!("// executing {:?}", loc.block);
            }
        }
        interp_ok(true)
    }

    pub fn if_tracking_unsafety(&self) -> bool {
        self.unsafety_tracking_crates.contains(&self.body().source.def_id().krate)
    }

    pub fn is_statement_unsafe(&self, stmt: &mir::Statement<'tcx>) -> bool {
        self.body().source_scopes[stmt.source_info.scope].is_unsafe
    }

    /// Runs the interpretation logic for the given `mir::Statement` at the current frame and
    /// statement counter.
    ///
    /// This does NOT move the statement counter forward, the caller has to do that!
    pub fn eval_statement(&mut self, stmt: &mir::Statement<'tcx>) -> InterpResult<'tcx> {
        info!("{:?}", stmt);

        use rustc_middle::mir::StatementKind::*;
        let is_stmt_unsafe = self.is_statement_unsafe(stmt);
        let is_unsafety_tracking_enabled = self.if_tracking_unsafety();

        if is_unsafety_tracking_enabled && is_stmt_unsafe {
            //   println!("unsafe stmt in target: {:?}", stmt);
        }

        match &stmt.kind {
            Assign(box (place, rvalue)) => self.eval_rvalue_into_place(
                rvalue,
                *place,
                is_stmt_unsafe,
                is_unsafety_tracking_enabled,
            )?,

            SetDiscriminant { place, variant_index } => {
                let dest = self.eval_place(**place)?;
                self.write_discriminant(*variant_index, &dest)?;
            }

            Deinit(place) => {
                let dest = self.eval_place(**place)?;
                self.write_uninit(&dest)?;
            }

            // Mark locals as alive
            StorageLive(local) => {
                if is_stmt_unsafe && is_unsafety_tracking_enabled {
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

            Intrinsic(box intrinsic) => self.eval_nondiverging_intrinsic(intrinsic)?,

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

            // Only used for temporary lifetime lints
            BackwardIncompatibleDropHint { .. } => {}
        }

        interp_ok(())
    }

    pub fn is_rvalue_op_unsafe(&self, op: &OpTy<'tcx, M::Provenance>) -> bool {
        match op.op() {
            Operand::Immediate(_imm) => false,
            Operand::Indirect(mplace) => self
                .ptr_get_alloc_id(mplace.ptr, 0)
                .discard_err()
                .is_some_and(|(alloc_id, _, _)| self.frame().is_alloc_id_unsafe(alloc_id)),
        }
    }

    pub fn mark_unsafe_local(&mut self, id: DefId, local: Local) {
        println!(
            "local {:?} in defid {:?} is unsafe in stmt {:?}",
            local, id, self.current_stmt_for_debug
        );
        self.def_id_to_unsafe_local.entry(id).or_insert_with(FxHashSet::default).insert(local);
    }
    pub fn mark_alloc_id_local_unsafe(&mut self, id: DefId, alloc_id: AllocId) {
        let copied = self
            .frame()
            .get_locals_from_alloc_id(alloc_id)
            .collect::<Vec<rustc_middle::mir::Local>>();
        info!("alloc_id :{:?}, locals: {:?}", alloc_id, copied);
        for loc in copied {
            self.mark_unsafe_local(id, loc);
        }
    }

    pub fn mark_mplace_unsafe(&mut self, mplace: &MPlaceTy<'tcx, M::Provenance>) {
        let def_id = self.body().source.def_id();
        if let Some((alloc_id, _, _)) = self.ptr_get_alloc_id(mplace.mplace().ptr, 0).discard_err()
        {
            self.mark_alloc_id_local_unsafe(def_id, alloc_id);
            self.frame_mut().mark_alloc_id_unsafe(alloc_id);
        }
    }
    pub fn mark_place_unsafe(&mut self, place: &PlaceTy<'tcx, M::Provenance>) {
        use crate::interpret::place::Place;

        let def_id = self.body().source.def_id();
        match place.place() {
            Place::Ptr(mplace) => {
                if let Some((alloc_id, _, _)) = self.ptr_get_alloc_id(mplace.ptr, 0).discard_err() {
                    self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                    self.frame_mut().mark_alloc_id_unsafe(alloc_id);
                }
            }
            Place::Local { local, .. } => {
                if let Some(op) = self.frame().locals[*local].access().discard_err() {
                    match op {
                        Operand::Indirect(mplace) => {
                            if let Some((alloc_id, _, _)) =
                                self.ptr_get_alloc_id(mplace.ptr, 0).discard_err()
                            {
                                self.mark_alloc_id_local_unsafe(def_id, alloc_id);
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

    /// Evaluate an assignment statement.
    ///
    /// There is no separate `eval_rvalue` function. Instead, the code for handling each rvalue
    /// type writes its results directly into the memory specified by the place.
    pub fn eval_rvalue_into_place(
        &mut self,
        rvalue: &mir::Rvalue<'tcx>,
        place: mir::Place<'tcx>,
        is_stmt_unsafe: bool,
        is_unsafety_tracking_enabled: bool,
    ) -> InterpResult<'tcx> {
        let dest = self.eval_place(place)?;
        // FIXME: ensure some kind of non-aliasing between LHS and RHS?
        // Also see https://github.com/rust-lang/rust/issues/68364.

        use rustc_middle::mir::Rvalue::*;
        let is_rvalue_unsafe = match *rvalue {
            ThreadLocalRef(did) => {
                let ptr = M::thread_local_static_pointer(self, did)?;
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
                let result = self.binary_op(bin_op, &left, &right)?;
                assert_eq!(result.layout, dest.layout, "layout mismatch for result of {bin_op:?}");
                self.write_immediate(*result, &dest)?;

                false
            }

            UnaryOp(un_op, ref operand) => {
                // The operand always has the same type as the result.
                let val = self.read_immediate(&self.eval_operand(operand, Some(dest.layout))?)?;
                let result = self.unary_op(un_op, &val)?;
                assert_eq!(result.layout, dest.layout, "layout mismatch for result of {un_op:?}");
                self.write_immediate(*result, &dest)?;
                false
            }

            NullaryOp(null_op, ty) => {
                let ty = self.instantiate_from_current_frame_and_normalize_erasing_regions(ty)?;
                let val = self.nullary_op(null_op, ty)?;
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
                let place = self.force_allocation(&src)?;
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

            RawPtr(_, place) => {
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

        trace!("{:?}", self.dump_place(&dest));
        if is_unsafety_tracking_enabled && (is_stmt_unsafe || is_rvalue_unsafe) {
            self.mark_place_unsafe(&dest);
        }

        interp_ok(())
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
            mir::AggregateKind::RawPtr(..) => {
                // Pointers don't have "fields" in the normal sense, so the
                // projection-based code below would either fail in projection
                // or in type mismatches. Instead, build an `Immediate` from
                // the parts and write that to the destination.
                let [data, meta] = &operands.raw else {
                    bug!("{kind:?} should have 2 operands, had {operands:?}");
                };
                let data = self.eval_operand(data, None)?;
                let is_data_unsafe = self.is_rvalue_op_unsafe(&data);
                let data = self.read_pointer(&data)?;
                let meta = self.eval_operand(meta, None)?;
                let is_meta_unsafe = self.is_rvalue_op_unsafe(&meta);
                let meta = if meta.layout.is_zst() {
                    MemPlaceMeta::None
                } else {
                    MemPlaceMeta::Meta(self.read_scalar(&meta)?)
                };
                let ptr_imm = Immediate::new_pointer_with_meta(data, meta, self);
                let ptr = ImmTy::from_immediate(ptr_imm, dest.layout);
                self.copy_op(&ptr, dest)?;
                return interp_ok(is_data_unsafe || is_meta_unsafe);
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
        let _ = self.write_discriminant(variant_index, dest)?;
        interp_ok(is_rvalue_unsafe)
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
            let rest_ptr = first_ptr.wrapping_offset(elem_size, self);
            // No alignment requirement since `copy_op` above already checked it.
            self.mem_copy_repeatedly(
                first_ptr,
                rest_ptr,
                elem_size,
                length - 1,
                /*nonoverlapping:*/ true,
            )?;
        }

        interp_ok(is_rvalue_unsafe)
    }

    fn mark_adt_ptr_field_pointee_unsafe(
        &mut self,
        def_id: DefId,
        ptr: Pointer<Option<M::Provenance>>,
        adt_ty: Ty<'tcx>,
    ) {
        if adt_ty.is_adt() {
            let adt = self.layout_of(adt_ty).expect("cannot find layout of adt ty");
            if let &ty::Adt(adt_def, ga) = adt.ty.kind() {
                if adt_def.is_struct() {
                    match &adt.layout.fields {
                        abi::FieldsShape::Arbitrary { offsets, .. } => {
                            //just safety check
                            if adt_def.all_fields().count() != offsets.len() {
                                bug!(
                                    "layout offsets len: {:?}, adt_def.all_fields.count: {:?}",
                                    offsets.len(),
                                    adt_def.all_fields().count()
                                );
                            }

                            for (idx, (size, field_def)) in
                                offsets.iter().zip(adt_def.all_fields()).enumerate()
                            {
                                let field_ty = field_def.ty(self.tcx(), ga);
                                info!("{:?}, ty: {:?}", idx, field_ty);
                                if field_ty.is_mutable_ptr() {
                                    let field_ptr = ptr.wrapping_offset(*size, self);
                                    // field_ptr points to the field, we need to further
                                    // read the pointer it points to, not field_ptr itself.
                                    let field_layout =
                                        self.layout_of(field_ty).expect("cannot read field layout");

                                    match self
                                        .read_immediate(
                                            &self.ptr_to_mplace(field_ptr, field_layout),
                                        )
                                        .discard_err()
                                    {
                                        Some(imm) => {
                                            match *imm {
                                                Immediate::Scalar(s) => {
                                                    self.mark_scalar_unsafe_allocation(
                                                        s,
                                                        field_layout,
                                                        def_id,
                                                    );
                                                }
                                                Immediate::ScalarPair(s1, s2) => {
                                                    self.mark_scalar_pair_unsafe_allocation(
                                                        s1,
                                                        s2,
                                                        field_layout,
                                                        def_id,
                                                    );
                                                }
                                                Immediate::Uninit => {
                                                    bug!("encounter unint scalae");
                                                }
                                            };
                                        }
                                        None => {
                                            trace!(
                                                "cannot read immediate from a field. adt ptr: {:?}, field_ptr:{:?}, adt_def:{:?}, field_def:{:?}, field_layout:{:?}",
                                                ptr,
                                                field_ptr,
                                                adt_def,
                                                field_def,
                                                field_layout,
                                            );
                                        }
                                    };
                                } else if field_ty.is_adt() {
                                    let field_ptr = ptr.wrapping_offset(*size, self);
                                    self.mark_adt_ptr_field_pointee_unsafe(
                                        def_id, field_ptr, field_ty,
                                    );
                                }
                            }
                        }
                        abi::FieldsShape::Union(_) => {}
                        _ => {
                            bug!(
                                "adt_ty: {:?}, adt_layout.fields: {:?}",
                                adt_ty,
                                adt.layout.fields
                            );
                        }
                    }
                } else {
                    info!("only implemented for struct, not union/enum");
                }
            } else {
                bug!("adt_ty.kind: {:?}, adt_layout.ty.kind: {:?}", adt_ty.kind(), adt.ty.kind());
            }
        } else {
            bug!("type should be adt, passed: {:?}", adt_ty);
        }
    }

    fn mark_scalar_unsafe_allocation(
        &mut self,
        s: Scalar<M::Provenance>,
        layout: TyAndLayout<'tcx>,
        def_id: DefId,
    ) {
        match s {
            Scalar::Ptr(ptr, _) => {
                if !layout.ty.is_any_ptr() {
                    trace!("scalar is ptr but layout.ty is not any ptr:{:?}", layout.ty,);
                }
                if layout.ty.is_mutable_ptr() {
                    if let Some((alloc_id, _, _)) =
                        self.ptr_get_alloc_id((ptr).into(), 0).discard_err()
                    {
                        self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                    }

                    if let Some(type_and_mut) = layout.ty.builtin_deref(true) {
                        info!("pointee type and mut:{:?}", type_and_mut);
                        if type_and_mut.is_adt() {
                            self.mark_adt_ptr_field_pointee_unsafe(
                                def_id,
                                ptr.into(),
                                type_and_mut,
                            );
                        }
                    }
                }
            }
            Scalar::Int(s) if layout.ty.is_mutable_ptr() => {
                // type is ptr, but the value is stored as int in runtime
                // try to inteprete the value as a pointer..
                let addr = s.to_target_usize(self.tcx());
                let p =
                    M::ptr_from_addr_cast(self, addr).expect("addr cannot be casted into pointer");
                if let Ok((alloc_id, _, _)) = self.ptr_try_get_alloc_id(p, 0) {
                    self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                }

                if let Some(type_and_mut) = layout.ty.builtin_deref(true) {
                    info!("pointee type and mut:{:?}", type_and_mut);
                    if type_and_mut.is_adt() {
                        self.mark_adt_ptr_field_pointee_unsafe(def_id, p, type_and_mut);
                    }
                }
            }
            _ => {}
        }
    }

    fn mark_scalar_pair_unsafe_allocation(
        &mut self,
        s1: Scalar<M::Provenance>,
        s2: Scalar<M::Provenance>,
        layout: TyAndLayout<'tcx>,
        def_id: DefId,
    ) {
        let (ty1, ty2) = match &layout.fields {
            abi::FieldsShape::Arbitrary { offsets, .. } => {
                if offsets.len() != 2 {
                    bug!("scalar pair layout FieldsShape::Arbitrary.offsets.len={}", offsets.len());
                }
                (layout.field(&*self, 0), layout.field(&*self, 1))
            }
            _ => {
                bug!(
                    "scalar pair layout fieldshape is not FieldsShape::Arbitrary, it is {:?}",
                    &layout.fields
                );
            }
        };

        info!("s1:{:?}, ty1: {:?}, s2:{:?}, ty2:{:?}, layout:{:?}", s1, ty1, s2, ty2, layout);
        let mut mark_scalar =
            |abi: abi::Scalar, s: Scalar<M::Provenance>, ty: TyAndLayout<'tcx>| match abi {
                abi::Scalar::Initialized { value, .. } => {
                    if let abi::Primitive::Pointer(_) = value {
                        match s {
                            Scalar::Ptr(ptr, _) => {
                                if let Some((alloc_id, _, _)) =
                                    self.ptr_get_alloc_id((ptr).into(), 0).discard_err()
                                {
                                    self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                                }
                                if let Some(type_and_mut) = ty.ty.builtin_deref(true) {
                                    if type_and_mut.is_adt() {
                                        self.mark_adt_ptr_field_pointee_unsafe(
                                            def_id,
                                            ptr.into(),
                                            type_and_mut,
                                        );
                                    }
                                }
                            }
                            _ => {
                                bug!("abi is a pointer but scalar is not");
                            }
                        }
                    }
                }
                abi::Scalar::Union { .. } => match s {
                    Scalar::Ptr(ptr, _) => {
                        if let Some((alloc_id, _, _)) =
                            self.ptr_get_alloc_id((ptr).into(), 0).discard_err()
                        {
                            self.mark_alloc_id_local_unsafe(def_id, alloc_id);
                        }
                        if let Some(type_and_mut) = ty.ty.builtin_deref(true) {
                            if type_and_mut.is_adt() {
                                self.mark_adt_ptr_field_pointee_unsafe(
                                    def_id,
                                    ptr.into(),
                                    type_and_mut,
                                );
                            }
                        }
                    }
                    _ => {}
                },
            };

        match layout.layout.backend_repr {
            BackendRepr::ScalarPair(a, b) => {
                //check a and b, the abi tells whether they are pointers
                mark_scalar(a, s1, ty1);
                mark_scalar(b, s2, ty2);
            }
            _ => {
                bug!(
                    "scalar is pair but layout backend repr is not: {:?}",
                    layout.layout.backend_repr
                );
            }
        }
    }

    fn eval_fn_call_place_unsafey(&mut self, place: &PlaceTy<'tcx, M::Provenance>) {
        use super::{Immediate, Place};
        let def_id = self.body().source.def_id();
        let layout = place.layout;

        match place.place() {
            Place::Ptr(mplace) => {
                info!("mplace: {:?}", mplace);
                if layout.ty.is_any_ptr() {
                    bug!("pointer operand are passed as Place::Ptr not Place::Local");
                }
                if layout.ty.is_adt() {
                    self.mark_adt_ptr_field_pointee_unsafe(def_id, mplace.ptr, layout.ty);
                }
            }
            Place::Local { local, .. } => {
                // &mut T or *mut T are passed as Operand::Immediate
                // T (if "very big") is passed as Operand::Indirect
                if let Some(op) = self.frame().locals[*local].access().discard_err().cloned() {
                    info!("local:{:?}, op: {:?}, layout: {:?}", local, op, layout,);
                    if layout.ty.is_any_ptr() {
                        info!("deref ty: {:?}", layout.ty.builtin_deref(true));
                    }
                    match op {
                        Operand::Immediate(Immediate::Scalar(s)) => {
                            self.mark_scalar_unsafe_allocation(s, layout, def_id);
                        }
                        Operand::Immediate(Immediate::ScalarPair(s1, s2)) => {
                            self.mark_scalar_pair_unsafe_allocation(s1, s2, layout, def_id);
                        }
                        Operand::Immediate(Immediate::Uninit) => {}
                        Operand::Indirect(mplace) => {
                            if layout.ty.is_adt() {
                                self.mark_adt_ptr_field_pointee_unsafe(
                                    def_id, mplace.ptr, layout.ty,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn eval_fn_call_operand_unsafey(&mut self, op: &OpTy<'tcx, M::Provenance>) {
        let def_id = self.body().source.def_id();
        let layout = op.layout;
        match op.as_mplace_or_imm() {
            either::Either::Left(mplace) => {
                if layout.ty.is_adt() {
                    self.mark_adt_ptr_field_pointee_unsafe(def_id, mplace.mplace().ptr, layout.ty);
                }
            }
            either::Either::Right(imm) => {
                match *imm {
                    Immediate::Scalar(s) => {
                        self.mark_scalar_unsafe_allocation(s, imm.layout, def_id);
                    }
                    Immediate::ScalarPair(s1, s2) => {
                        self.mark_scalar_pair_unsafe_allocation(s1, s2, imm.layout, def_id);
                    }
                    Immediate::Uninit => {
                        //it is possible that a copied operand is uninit
                    }
                };
            }
        }
    }

    /// Evaluate the arguments of a function call
    fn eval_fn_call_argument(
        &mut self,
        op: &mir::Operand<'tcx>,
        is_safety_tracking_enabled: bool,
        is_fn_call_unsafe: bool,
    ) -> InterpResult<'tcx, FnArg<'tcx, M::Provenance>> {
        interp_ok(match op {
            mir::Operand::Copy(_) | mir::Operand::Constant(_) => {
                // Make a regular copy.
                let op = self.eval_operand(op, None)?;
                if is_safety_tracking_enabled && is_fn_call_unsafe {
                    self.eval_fn_call_operand_unsafey(&op);
                }
                FnArg::Copy(op)
            }
            mir::Operand::Move(place) => {
                // If this place lives in memory, preserve its location.
                // We call `place_to_op` which will be an `MPlaceTy` whenever there exists
                // an mplace for this place. (This is in contrast to `PlaceTy::as_mplace_or_local`
                // which can return a local even if that has an mplace.)
                let place = self.eval_place(*place)?;
                let op = self.place_to_op(&place)?;

                match op.as_mplace_or_imm() {
                    Either::Left(mplace) => {
                        if is_safety_tracking_enabled && is_fn_call_unsafe {
                            self.eval_fn_call_place_unsafey(&place);
                        }
                        FnArg::InPlace(mplace)
                    }
                    Either::Right(_imm) => {
                        // This argument doesn't live in memory, so there's no place
                        // to make inaccessible during the call.
                        // We rely on there not being any stray `PlaceTy` that would let the
                        // caller directly access this local!
                        // This is also crucial for tail calls, where we want the `FnArg` to
                        // stay valid when the old stack frame gets popped.
                        if is_safety_tracking_enabled && is_fn_call_unsafe {
                            self.eval_fn_call_operand_unsafey(&op);
                        }
                        FnArg::Copy(op)
                    }
                }
            }
        })
    }

    /// Shared part of `Call` and `TailCall` implementation — finding and evaluating all the
    /// necessary information about callee and arguments to make a call.
    fn eval_callee_and_args(
        &mut self,
        terminator: &mir::Terminator<'tcx>,
        func: &mir::Operand<'tcx>,
        args: &[Spanned<mir::Operand<'tcx>>],
        is_safety_tracking_enabled: bool,
        is_terminator_unsafe: bool,
    ) -> InterpResult<'tcx, EvaluatedCalleeAndArgs<'tcx, M>> {
        let func = self.eval_operand(func, None)?;
        let fn_sig_binder = func.layout.ty.fn_sig(*self.tcx);
        let fn_sig = self.tcx.normalize_erasing_late_bound_regions(self.typing_env, fn_sig_binder);
        let args = args
            .iter()
            .map(|arg| {
                self.eval_fn_call_argument(
                    &arg.node,
                    is_safety_tracking_enabled,
                    is_terminator_unsafe,
                )
            })
            .collect::<InterpResult<'tcx, Vec<_>>>()?;

        let extra_args = &args[fn_sig.inputs().len()..];
        let extra_args =
            self.tcx.mk_type_list_from_iter(extra_args.iter().map(|arg| arg.layout().ty));

        let (callee, fn_abi, with_caller_location) = match *func.layout.ty.kind() {
            ty::FnPtr(..) => {
                let fn_ptr = self.read_pointer(&func)?;
                let fn_val = self.get_ptr_fn(fn_ptr)?;
                (fn_val, self.fn_abi_of_fn_ptr(fn_sig_binder, extra_args)?, false)
            }
            ty::FnDef(def_id, args) => {
                let instance = self.resolve(def_id, args)?;
                (
                    FnVal::Instance(instance),
                    self.fn_abi_of_instance(instance, extra_args)?,
                    instance.def.requires_caller_location(*self.tcx),
                )
            }
            _ => {
                span_bug!(terminator.source_info.span, "invalid callee of type {}", func.layout.ty)
            }
        };

        interp_ok(EvaluatedCalleeAndArgs { callee, args, fn_sig, fn_abi, with_caller_location })
    }

    pub fn is_terminator_unsafe(&self, terminator: &mir::Terminator<'tcx>) -> bool {
        self.body().source_scopes[terminator.source_info.scope].is_unsafe
    }

    fn eval_terminator(&mut self, terminator: &mir::Terminator<'tcx>) -> InterpResult<'tcx> {
        info!("{:?}", terminator.kind);

        let is_safety_tracking_enabled = self.if_tracking_unsafety();
        let is_terminator_unsafe = self.is_terminator_unsafe(terminator);
        use rustc_middle::mir::TerminatorKind::*;
        match terminator.kind {
            Return => {
                self.return_from_current_stack_frame(/* unwinding */ false)?
            }

            Goto { target } => self.go_to_block(target),

            SwitchInt { ref discr, ref targets } => {
                let discr = self.read_immediate(&self.eval_operand(discr, None)?)?;
                trace!("SwitchInt({:?})", *discr);

                // Branch to the `otherwise` case by default, if no match is found.
                let mut target_block = targets.otherwise();

                for (const_int, target) in targets.iter() {
                    // Compare using MIR BinOp::Eq, to also support pointer values.
                    // (Avoiding `self.binary_op` as that does some redundant layout computation.)
                    let res = self.binary_op(
                        mir::BinOp::Eq,
                        &discr,
                        &ImmTy::from_uint(const_int, discr.layout),
                    )?;
                    if res.to_scalar().to_bool()? {
                        target_block = target;
                        break;
                    }
                }

                self.go_to_block(target_block);
            }

            Call {
                ref func,
                ref args,
                destination,
                target,
                unwind,
                call_source: _,
                fn_span: _,
            } => {
                let old_stack = self.frame_idx();
                let old_loc = self.frame().loc;

                let EvaluatedCalleeAndArgs { callee, args, fn_sig, fn_abi, with_caller_location } =
                    self.eval_callee_and_args(
                        terminator,
                        func,
                        args,
                        is_safety_tracking_enabled,
                        is_terminator_unsafe,
                    )?;

                let destination = self.force_allocation(&self.eval_place(destination)?)?;

                if is_safety_tracking_enabled && is_terminator_unsafe {
                    self.mark_mplace_unsafe(&destination);
                }

                self.init_fn_call(
                    callee,
                    (fn_sig.abi, fn_abi),
                    &args,
                    with_caller_location,
                    &destination,
                    target,
                    if fn_abi.can_unwind { unwind } else { mir::UnwindAction::Unreachable },
                )?;
                // Sanity-check that `eval_fn_call` either pushed a new frame or
                // did a jump to another block.
                if self.frame_idx() == old_stack && self.frame().loc == old_loc {
                    span_bug!(terminator.source_info.span, "evaluating this call made no progress");
                }
            }

            TailCall { ref func, ref args, fn_span: _ } => {
                let old_frame_idx = self.frame_idx();

                let EvaluatedCalleeAndArgs { callee, args, fn_sig, fn_abi, with_caller_location } =
                    self.eval_callee_and_args(
                        terminator,
                        func,
                        args,
                        is_safety_tracking_enabled,
                        is_terminator_unsafe,
                    )?;

                self.init_fn_tail_call(callee, (fn_sig.abi, fn_abi), &args, with_caller_location)?;

                if self.frame_idx() != old_frame_idx {
                    span_bug!(
                        terminator.source_info.span,
                        "evaluating this tail call pushed a new stack frame"
                    );
                }
            }

            Drop { place, target, unwind, replace: _ } => {
                let place = self.eval_place(place)?;
                let instance = Instance::resolve_drop_in_place(*self.tcx, place.layout.ty);
                if let ty::InstanceKind::DropGlue(_, None) = instance.def {
                    // This is the branch we enter if and only if the dropped type has no drop glue
                    // whatsoever. This can happen as a result of monomorphizing a drop of a
                    // generic. In order to make sure that generic and non-generic code behaves
                    // roughly the same (and in keeping with Mir semantics) we do nothing here.
                    self.go_to_block(target);
                    return interp_ok(());
                }
                trace!("TerminatorKind::drop: {:?}, type {}", place, place.layout.ty);
                self.init_drop_in_place_call(&place, instance, target, unwind)?;
            }

            Assert { ref cond, expected, ref msg, target, unwind } => {
                let ignored =
                    M::ignore_optional_overflow_checks(self) && msg.is_optional_overflow_check();
                let cond_val = self.read_scalar(&self.eval_operand(cond, None)?)?.to_bool()?;
                if ignored || expected == cond_val {
                    self.go_to_block(target);
                } else {
                    M::assert_panic(self, msg, unwind)?;
                }
            }

            UnwindTerminate(reason) => {
                M::unwind_terminate(self, reason)?;
            }

            // When we encounter Resume, we've finished unwinding
            // cleanup for the current stack frame. We pop it in order
            // to continue unwinding the next frame
            UnwindResume => {
                trace!("unwinding: resuming from cleanup");
                // By definition, a Resume terminator means
                // that we're unwinding
                self.return_from_current_stack_frame(/* unwinding */ true)?;
                return interp_ok(());
            }

            // It is UB to ever encounter this.
            Unreachable => throw_ub!(Unreachable),

            // These should never occur for MIR we actually run.
            FalseEdge { .. } | FalseUnwind { .. } | Yield { .. } | CoroutineDrop => span_bug!(
                terminator.source_info.span,
                "{:#?} should have been eliminated by MIR pass",
                terminator.kind
            ),

            InlineAsm { template, ref operands, options, ref targets, .. } => {
                M::eval_inline_asm(self, template, operands, options, targets)?;
            }
        }

        interp_ok(())
    }
}
