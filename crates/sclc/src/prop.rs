use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::Type;

/// A unique type origin identifier used for propositional refinement.
pub type TypeId = usize;

/// A proposition about types, used for type refinement through control flow.
#[derive(Clone, Debug)]
pub enum Prop {
    /// The boolean value with this TypeId is true.
    IsTrue(TypeId),
    /// The type with this TypeId can be replaced with the given type.
    RefinesTo(TypeId, Type),
    /// Logical negation of a proposition.
    Not(Box<Prop>),
    /// If the first proposition is proven, the second is also proven.
    Implies(Box<Prop>, Box<Prop>),
}

impl Prop {
    pub fn negated(self) -> Self {
        Prop::Not(Box::new(self))
    }

    pub fn implies(self, consequent: Prop) -> Self {
        Prop::Implies(Box::new(self), Box::new(consequent))
    }
}

// Custom PartialEq/Eq/Hash: RefinesTo compares by both the source TypeId
// and the target type's TypeId (not structural type equality).
impl PartialEq for Prop {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Prop::IsTrue(a), Prop::IsTrue(b)) => a == b,
            (Prop::RefinesTo(a_id, a_ty), Prop::RefinesTo(b_id, b_ty)) => {
                a_id == b_id && a_ty.id() == b_ty.id()
            }
            (Prop::Not(a), Prop::Not(b)) => a == b,
            (Prop::Implies(a1, a2), Prop::Implies(b1, b2)) => a1 == b1 && a2 == b2,
            _ => false,
        }
    }
}

impl Eq for Prop {}

impl Hash for Prop {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Prop::IsTrue(id) => id.hash(state),
            Prop::RefinesTo(id, ty) => {
                id.hash(state);
                ty.id().hash(state);
            }
            Prop::Not(inner) => inner.hash(state),
            Prop::Implies(ante, cons) => {
                ante.hash(state);
                cons.hash(state);
            }
        }
    }
}

/// The result of running the derivation engine: a set of proven propositions
/// and a map of proven type refinements.
#[derive(Clone, Debug, Default)]
pub struct ProvenSet {
    /// All proven atomic propositions (IsTrue, RefinesTo, Not(...)).
    proven: Vec<Prop>,
    /// Implications indexed by antecedent for forward-chaining.
    implications: HashMap<Prop, Vec<Prop>>,
    /// Derived refinement map: TypeId → replacement Type.
    refines_to: HashMap<TypeId, Type>,
}

impl ProvenSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the refinement target for a TypeId, if any.
    pub fn get_refinement(&self, id: TypeId) -> Option<&Type> {
        self.refines_to.get(&id)
    }

    /// Apply a type's refinements recursively, walking the type tree.
    pub fn refine_type(&self, ty: &Type) -> Type {
        if self.refines_to.is_empty() {
            return ty.clone();
        }
        self.refine_type_inner(ty)
    }

    fn refine_type_inner(&self, ty: &Type) -> Type {
        // Check if this type itself is refined.
        if let Some(replacement) = self.refines_to.get(&ty.id()) {
            // Continue refining into the replacement (fixed-point).
            return self.refine_type_inner(replacement);
        }

        // Otherwise, walk into children.
        use crate::TypeKind;
        match &ty.kind {
            TypeKind::Optional(inner) => {
                let refined_inner = self.refine_type_inner(inner);
                Type::Optional(Box::new(refined_inner)).with_id(ty.id())
            }
            TypeKind::List(inner) => {
                let refined_inner = self.refine_type_inner(inner);
                Type::List(Box::new(refined_inner)).with_id(ty.id())
            }
            TypeKind::Record(record) => {
                let refined_record = record.map_types(|field_ty| self.refine_type_inner(field_ty));
                Type::Record(refined_record).with_id(ty.id())
            }
            TypeKind::Dict(dict) => {
                let refined_dict = crate::DictType {
                    key: Box::new(self.refine_type_inner(&dict.key)),
                    value: Box::new(self.refine_type_inner(&dict.value)),
                };
                Type::Dict(refined_dict).with_id(ty.id())
            }
            TypeKind::Fn(fn_ty) => {
                let refined_fn = crate::FnType {
                    type_params: fn_ty
                        .type_params
                        .iter()
                        .map(|(id, bound)| (*id, self.refine_type_inner(bound)))
                        .collect(),
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| self.refine_type_inner(p))
                        .collect(),
                    ret: Box::new(self.refine_type_inner(&fn_ty.ret)),
                };
                Type::Fn(refined_fn).with_id(ty.id())
            }
            TypeKind::IsoRec(id, body) => {
                let refined_body = self.refine_type_inner(body);
                Type::IsoRec(*id, Box::new(refined_body)).with_id(ty.id())
            }
            // Leaf types: no children to refine.
            TypeKind::Any
            | TypeKind::Int
            | TypeKind::Float
            | TypeKind::Bool
            | TypeKind::Str
            | TypeKind::Path
            | TypeKind::Never
            | TypeKind::Var(_)
            | TypeKind::Exception(_) => ty.clone(),
        }
    }

    /// Derive all consequences from a set of new propositions added to the
    /// existing proven set. Returns a new ProvenSet with all derivations.
    pub fn with_propositions(&self, new_props: &[Prop]) -> Self {
        let mut result = self.clone();

        for prop in new_props {
            result.add_proposition(prop.clone());
        }

        result
    }

    fn add_proposition(&mut self, prop: Prop) {
        match prop {
            Prop::Implies(ref ante, ref cons) => {
                // Check if the antecedent is already proven.
                if self.is_proven(ante) {
                    // Directly prove the consequent.
                    self.prove(cons.as_ref().clone());
                } else {
                    // Index the implication by its antecedent.
                    self.implications
                        .entry(ante.as_ref().clone())
                        .or_default()
                        .push(cons.as_ref().clone());
                }
            }
            _ => {
                self.prove(prop);
            }
        }
    }

    fn prove(&mut self, prop: Prop) {
        if self.is_proven(&prop) {
            return;
        }

        // Record the refinement if applicable.
        if let Prop::RefinesTo(id, ref ty) = prop {
            self.refines_to.insert(id, ty.clone());
        }

        self.proven.push(prop.clone());

        // Forward-chain: check if this newly proven proposition triggers any
        // implications.
        if let Some(consequents) = self.implications.remove(&prop) {
            for cons in consequents {
                self.prove(cons);
            }
        }
    }

    fn is_proven(&self, prop: &Prop) -> bool {
        self.proven.contains(prop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_modus_ponens() {
        let mut ps = ProvenSet::new();

        // Add implication: IsTrue(1) => RefinesTo(2, Int)
        let inner_type = Type::Int();
        let ante = Prop::IsTrue(1);
        let cons = Prop::RefinesTo(2, inner_type.clone());
        ps.add_proposition(ante.clone().implies(cons));

        // Prove IsTrue(1)
        ps.add_proposition(ante);

        // RefinesTo(2, Int) should be derived
        assert!(ps.get_refinement(2).is_some());
        assert_eq!(ps.get_refinement(2).unwrap().kind, crate::TypeKind::Int);
    }

    #[test]
    fn transitive_chain() {
        let mut ps = ProvenSet::new();

        let int_ty = Type::Int();
        let record_ty = Type::Record(crate::RecordType::default());

        // IsTrue(1) => RefinesTo(2, Int)
        ps.add_proposition(Prop::IsTrue(1).implies(Prop::RefinesTo(2, int_ty.clone())));

        // RefinesTo(2, Int) => RefinesTo(3, Record)
        ps.add_proposition(
            Prop::RefinesTo(2, int_ty.clone()).implies(Prop::RefinesTo(3, record_ty.clone())),
        );

        // Prove IsTrue(1), should chain through
        ps.add_proposition(Prop::IsTrue(1));

        assert!(ps.get_refinement(2).is_some());
        assert!(ps.get_refinement(3).is_some());
    }

    #[test]
    fn not_is_true_as_antecedent() {
        let mut ps = ProvenSet::new();

        let int_ty = Type::Int();

        // Not(IsTrue(1)) => RefinesTo(2, Int)
        ps.add_proposition(
            Prop::IsTrue(1)
                .negated()
                .implies(Prop::RefinesTo(2, int_ty.clone())),
        );

        // Prove Not(IsTrue(1))
        ps.add_proposition(Prop::IsTrue(1).negated());

        assert!(ps.get_refinement(2).is_some());
    }

    #[test]
    fn prop_equality() {
        let t1 = Type::Int();
        let t2 = Type::Int(); // different id
        let id = t1.id();

        let p1 = Prop::RefinesTo(10, t1.clone());
        let p2 = Prop::RefinesTo(10, t1.clone()); // same type id
        let p3 = Prop::RefinesTo(10, t2); // different type id

        assert_eq!(p1, p2);
        assert_ne!(p1, p3);

        // RefinesTo with same target type id should be equal
        let t1_copy = t1.with_id(id);
        let p4 = Prop::RefinesTo(10, t1_copy);
        assert_eq!(p1, p4);
    }

    #[test]
    fn refine_type_recursive() {
        let mut ps = ProvenSet::new();

        // Create a record { f: Int?(3) } and prove RefinesTo(3, Int(inner_id))
        let inner_int = Type::Int();
        let inner_id = inner_int.id();
        let optional_int = Type::Optional(Box::new(inner_int.clone()));
        let opt_id = optional_int.id();

        let mut record = crate::RecordType::default();
        record.insert("f".to_string(), optional_int);
        let record_ty = Type::Record(record);

        // Prove that the optional field refines to its inner type
        ps.add_proposition(Prop::RefinesTo(opt_id, inner_int.clone()));

        // Refine the record type
        let refined = ps.refine_type(&record_ty);

        // The field should now be Int, not Int?
        if let crate::TypeKind::Record(r) = &refined.kind {
            let field = r.get("f").unwrap();
            assert_eq!(field.kind, crate::TypeKind::Int);
            assert_eq!(field.id(), inner_id);
        } else {
            panic!("expected record type");
        }
    }
}
