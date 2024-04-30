use crate::{Analysis, AnalysisDomain, Backward, GenKill};
use rustc_index::bit_set::BitSet;
use rustc_middle::mir::visit::{PlaceContext, Visitor};
use rustc_middle::mir::{
    BinOp, Local, Location, Place, Rvalue, Statement, StatementSafety, Terminator, TerminatorEdges,
};

pub struct SafetyLocals;
impl<'tcx> AnalysisDomain<'tcx> for SafetyLocals {
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
impl<'tcx> Analysis<'tcx> for SafetyLocals {
    fn apply_statement_effect(
        &mut self,
        state: &mut Self::Domain,
        statement: &Statement<'tcx>,
        location: Location,
    ) {
        debug!(?statement);
        TransferFunction(state, statement.safety).visit_statement(statement, location);
    }

    fn apply_terminator_effect<'mir>(
        &mut self,
        state: &mut Self::Domain,
        terminator: &'mir Terminator<'tcx>,
        location: Location,
    ) -> TerminatorEdges<'mir, 'tcx> {
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
struct TransferFunction<'a, T>(pub &'a mut T, StatementSafety);

#[allow(unused_variables)]
impl<'tcx> Visitor<'tcx> for TransferFunction<'_, BitSet<Local>> {
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        let out_safety = self.1;
        self.1 = statement.safety;
        self.super_statement(statement, location);
        self.1 = out_safety;
    }

    fn visit_assign(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>, location: Location) {
        if let StatementSafety::Unsafe = self.1 {
            // if the statement is unsafe
            // lhs becomes unsafe
            self.0.gen(place.local);

            // then we check if this leads to any rhs place become unsafe
            LTRRvaluePlaceVisitor { gen_kill: self.0 }.visit_rvalue(rvalue, location);
        } else {
            // if the statement is safe,
            // we check if any unsafe rhs leads lhs unsafe
            RTLRvaluePlaceVisitor { gen_kill: self.0, lhs: place.local }
                .visit_rvalue(rvalue, location);
        }
    }
}

#[allow(dead_code)]
struct LTRRvaluePlaceVisitor<'a> {
    pub gen_kill: &'a mut BitSet<Local>,
}
#[allow(unused_variables)]
impl<'tcx> Visitor<'tcx> for LTRRvaluePlaceVisitor<'_> {
    fn visit_rvalue(&mut self, rvalue: &Rvalue<'tcx>, location: Location) {
        match rvalue {
            Rvalue::Use(_)
            | Rvalue::Repeat(..)
            | Rvalue::Len(_)
            | Rvalue::NullaryOp(..)
            | Rvalue::Discriminant(_)
            | Rvalue::Aggregate(..)
            | Rvalue::UnaryOp(..) => {
                //safe case, we return without further descending to the place
                return;
            }
            Rvalue::Ref(_, kind, place) => self.super_rvalue(rvalue, location),
            Rvalue::ThreadLocalRef(def_id) => self.super_rvalue(rvalue, location),
            Rvalue::AddressOf(mutability, place) => self.super_rvalue(rvalue, location),
            Rvalue::Cast(cast_kind, op, ty) => self.super_rvalue(rvalue, location),
            Rvalue::BinaryOp(op, operands) | Rvalue::CheckedBinaryOp(op, operands) => {
                if let BinOp::Offset = op {
                    self.super_rvalue(rvalue, location)
                }
            }
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
