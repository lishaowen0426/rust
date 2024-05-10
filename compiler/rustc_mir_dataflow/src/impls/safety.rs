use crate::{Analysis, AnalysisDomain, Backward, GenKill};
use rustc_index::bit_set::BitSet;
use rustc_index::{IndexSlice, IndexVec};
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{
    BorrowKind, Local, LocalDecl, Location, Operand, Place, Rvalue, Statement, StatementSafety,
    Terminator, TerminatorEdges, TerminatorKind,
};
use rustc_middle::ty;

pub struct SafetyLocals<'tcx> {
    pub local_decls: IndexVec<Local, LocalDecl<'tcx>>,
}

impl<'tcx> AnalysisDomain<'tcx> for SafetyLocals<'tcx> {
    type Domain = BitSet<Local>; // 1 == unsafe
    type Direction = Backward;

    const NAME: &'static str = "safety";

    fn bottom_value(&self, body: &rustc_middle::mir::Body<'tcx>) -> Self::Domain {
        //bottom = safe
        BitSet::new_empty(body.local_decls.len())
    }

    fn initialize_start_block(&self, _: &rustc_middle::mir::Body<'tcx>, _: &mut Self::Domain) {
        // all locals are safe initially
    }
}
#[allow(unused_variables)]
impl<'tcx> Analysis<'tcx> for SafetyLocals<'tcx> {
    fn apply_statement_effect(
        &mut self,
        state: &mut Self::Domain,
        statement: &Statement<'tcx>,
        location: Location,
    ) {
        debug!(?statement);
        let local_decls = self.local_decls.as_slice();
        TransferFunction { state, safety: statement.safety, local_decls }
            .visit_statement(statement, location);
    }

    fn apply_terminator_effect<'mir>(
        &mut self,
        state: &mut Self::Domain,
        terminator: &'mir Terminator<'tcx>,
        location: Location,
    ) -> TerminatorEdges<'mir, 'tcx> {
        let local_decls = self.local_decls.as_slice();
        TransferFunction { state, safety: terminator.safety, local_decls }
            .visit_terminator(terminator, location);
        terminator.edges()
    }

    fn apply_call_return_effect(
        &mut self,
        state: &mut Self::Domain,
        block: rustc_middle::mir::BasicBlock,
        return_places: rustc_middle::mir::CallReturnPlaces<'_, 'tcx>,
    ) {
    }
}

#[allow(dead_code)]
struct TransferFunction<'a, 'tcx, T> {
    state: &'a mut T,
    safety: StatementSafety,
    local_decls: &'a IndexSlice<Local, LocalDecl<'tcx>>,
}

#[allow(unused_variables)]
impl<'tcx> Visitor<'tcx> for TransferFunction<'_, '_, BitSet<Local>> {
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        let out_safety = self.safety;
        self.safety = statement.safety;
        self.super_statement(statement, location);
        self.safety = out_safety;
    }

    fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
        match &terminator.kind {
            TerminatorKind::Call {
                func,
                args,
                destination,
                target,
                unwind,
                call_source,
                fn_span,
            } => {
                for op in args {
                    if let Operand::Copy(p) | Operand::Move(p) = &op.node {
                        match self.local_decls[p.local].ty.kind() {
                            ty::Ref(..) | ty::RawPtr(..) => {
                                self.state.gen(p.local);
                            }
                            _ => {
                                return;
                            }
                        }
                    } else {
                        return;
                    }
                }
            }
            _ => {
                return;
            }
        }
    }

    fn visit_assign(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>, location: Location) {
        if let StatementSafety::Unsafe = self.safety {
            // if the statement is unsafe
            // lhs becomes unsafe
            self.state.gen(place.local);

            // then we check if this leads to any rhs place become unsafe
            LTRRvaluePlaceVisitor { gen_kill: self.state, local_decls: self.local_decls }
                .visit_rvalue(rvalue, location);
        } else if self.state.contains(place.local) {
            // or if the lhs has been marked unsafe
            LTRRvaluePlaceVisitor { gen_kill: self.state, local_decls: self.local_decls }
                .visit_rvalue(rvalue, location);
        } else {
            // if the statement is safe,
            // we check if any unsafe rhs leads lhs unsafe
            RTLRvaluePlaceVisitor { gen_kill: self.state, lhs: place.local }
                .visit_rvalue(rvalue, location);
        }
    }
}

#[allow(dead_code)]
struct LTRRvaluePlaceVisitor<'a, 'tcx> {
    pub gen_kill: &'a mut BitSet<Local>,
    pub local_decls: &'a IndexSlice<Local, LocalDecl<'tcx>>,
}
#[allow(unused_variables)]
impl<'tcx> Visitor<'tcx> for LTRRvaluePlaceVisitor<'_, '_> {
    fn visit_rvalue(&mut self, rvalue: &Rvalue<'tcx>, location: Location) {
        match rvalue {
            Rvalue::Repeat(..)
            | Rvalue::Len(_)
            | Rvalue::NullaryOp(..)
            | Rvalue::Discriminant(_)
            | Rvalue::Aggregate(..)
            | Rvalue::Cast(..)
            | Rvalue::BinaryOp(..)
            | Rvalue::CheckedBinaryOp(..)
            | Rvalue::UnaryOp(..) => {
                //safe case, we return without further descending to the place
                return;
            }
            Rvalue::Use(op) => {
                //we check if op is pointer or reference,
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    match self.local_decls[p.local].ty.kind() {
                        ty::Ref(..) | ty::RawPtr(..) => {
                            self.super_rvalue(rvalue, location);
                        }
                        _ => {
                            return;
                        }
                    }
                } else {
                    return;
                }
            }
            Rvalue::Ref(_, kind, place) => match kind {
                BorrowKind::Mut { .. } => {
                    self.super_rvalue(rvalue, location);
                }
                BorrowKind::Shared => {
                    self.super_rvalue(rvalue, location);
                }
                _ => {
                    return;
                }
            },
            Rvalue::ThreadLocalRef(def_id) => self.super_rvalue(rvalue, location),
            Rvalue::AddressOf(mutability, place) => self.super_rvalue(rvalue, location),
            Rvalue::ShallowInitBox(op, _) => self.super_rvalue(rvalue, location),
            Rvalue::CopyForDeref(place) => self.super_rvalue(rvalue, location),
        }
        return;
    }

    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: Location) {
        self.gen_kill.gen(place.local);
        self.super_place(place, context, location);
    }
}
#[allow(dead_code)]
struct RTLRvaluePlaceVisitor<'a> {
    pub gen_kill: &'a mut BitSet<Local>,
    pub lhs: Local,
}
impl<'tcx> Visitor<'tcx> for RTLRvaluePlaceVisitor<'_> {}
