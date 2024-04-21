use crate::{AnalysisDomain, Backward, GenKill, GenKillAnalysis};
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
impl<'tcx> GenKillAnalysis<'tcx> for SafetyLocals {
    type Idx = Local;

    fn domain_size(&self, body: &rustc_middle::mir::Body<'tcx>) -> usize {
        body.local_decls.len()
    }

    fn statement_effect(
        &mut self,
        trans: &mut impl GenKill<Self::Idx>,
        statement: &Statement<'tcx>,
        location: Location,
    ) {
        TransferFunction(trans, StatementSafety::Safe).visit_statement(statement, location)
    }

    fn terminator_effect<'mir>(
        &mut self,
        trans: &mut Self::Domain,
        terminator: &'mir Terminator<'tcx>,
        location: rustc_middle::mir::Location,
    ) -> TerminatorEdges<'mir, 'tcx> {
        terminator.edges()
    }
    fn call_return_effect(
        &mut self,
        trans: &mut Self::Domain,
        block: rustc_middle::mir::BasicBlock,
        return_places: rustc_middle::mir::CallReturnPlaces<'_, 'tcx>,
    ) {
    }

    fn switch_int_edge_effects<G: crate::GenKill<Self::Idx>>(
        &mut self,
        _block: rustc_middle::mir::BasicBlock,
        _discr: &rustc_middle::mir::Operand<'tcx>,
        _edge_effects: &mut impl crate::SwitchIntEdgeEffects<G>,
    ) {
    }
}

#[allow(dead_code)]
struct TransferFunction<'a, T>(pub &'a mut T, StatementSafety);

#[allow(unused_variables)]
impl<'tcx, T> Visitor<'tcx> for TransferFunction<'_, T>
where
    T: GenKill<Local>,
{
    fn visit_statement(&mut self, statement: &Statement<'tcx>, location: Location) {
        let out_safety = self.1;
        self.1 = statement.safety;
        self.super_statement(statement, location);
        self.1 = out_safety;
    }

    fn visit_assign(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>, location: Location) {
        debug!(?rvalue);

        if let StatementSafety::Unsafe = self.1 {
            // if the statement is unsafe
            // lhs becomes unsafe
            self.0.gen(place.local);
        } else {
            // the statement is safe, we
            //
        }
    }
}

#[allow(dead_code)]
struct RvalueVisitor<'a, T> {
    pub gen_kill: &'a mut T,
    pub in_unsafe_context: bool,
}
#[allow(unused_variables)]
impl<'tcx, T> Visitor<'tcx> for RvalueVisitor<'_, T>
where
    T: GenKill<Local>,
{
    fn visit_rvalue(&mut self, rvalue: &Rvalue<'tcx>, location: Location) {
        match rvalue {
            Rvalue::Use(_)
            | Rvalue::Repeat(..)
            | Rvalue::Len(_)
            | Rvalue::NullaryOp(..)
            | Rvalue::Discriminant(_)
            | Rvalue::Aggregate(..)
            | Rvalue::UnaryOp(..) => {
                //safe case
            }
            Rvalue::Ref(_, kind, place) => {}
            Rvalue::ThreadLocalRef(def_id) => {}
            Rvalue::AddressOf(mutability, place) => {}
            Rvalue::Cast(cast_kind, op, ty) => {}
            Rvalue::BinaryOp(op, operands) | Rvalue::CheckedBinaryOp(op, operands) => {
                if let BinOp::Offset = op {}
            }
            Rvalue::ShallowInitBox(op, _) => {}
            Rvalue::CopyForDeref(place) => {}
        }
    }
    fn visit_place(&mut self, place: &Place<'tcx>, context: PlaceContext, location: Location) {}
}
