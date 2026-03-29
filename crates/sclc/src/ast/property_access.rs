use std::collections::HashSet;

use super::{Expr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccessExpr {
    pub expr: Box<Loc<Expr>>,
    pub property: Loc<Var>,
}

impl PropertyAccessExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        self.expr.as_ref().free_vars()
    }

    pub fn type_synth<S: crate::SourceRepo>(
        &self,
        checker: &crate::checker::TypeChecker<'_, S>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        use crate::{DiagList, Diagnosed, RecordType, Type, TypeKind};

        let mut diags = DiagList::new();
        let raw_lhs_ty = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack(&mut diags)
            .unfold();
        let lhs_ty = env.resolve_var_bound(&raw_lhs_ty).unfold();
        if matches!(lhs_ty.kind, TypeKind::Never) {
            return Ok(Diagnosed::new(Type::Never, diags));
        }

        // Free variable: constrain to record with accessed member
        if let TypeKind::Var(lhs_var_id) = &raw_lhs_ty.kind
            && env.is_free_var(*lhs_var_id)
        {
            let member_id = crate::checker::next_type_id();
            let member_var = Type::Var(member_id);
            if let Some(fv) = &env.free_vars {
                fv.borrow_mut().register(member_id);
                let mut record = RecordType::default();
                record.insert(self.property.name.clone(), member_var.clone());
                fv.borrow_mut().constrain(*lhs_var_id, Type::Record(record));
            }
            if let Some((cursor, _)) = &self.property.cursor {
                cursor.set_type(member_var.clone());
            }
            return Ok(Diagnosed::new(member_var, diags));
        }

        // Completion candidates for property access
        if let Some((cursor, offset)) = &self.property.cursor {
            let prefix = &self.property.name[..*offset];
            if let TypeKind::Record(record_ty) = &lhs_ty.kind {
                for (name, _) in record_ty.iter() {
                    if name.starts_with(prefix) {
                        cursor.add_completion_candidate(crate::CompletionCandidate::Member(
                            name.clone(),
                        ));
                    }
                }
            }
        }

        let prop_name = self.property.name.as_str();
        let member_ty = match &lhs_ty.kind {
            TypeKind::Record(record_ty) => record_ty.get(prop_name).cloned(),
            _ => None,
        };
        if let Some(member_ty) = member_ty {
            if let Some((cursor, _)) = &self.property.cursor {
                cursor.set_type(member_ty.clone());
                cursor.set_identifier(crate::CursorIdentifier::Let(prop_name.into()));
                if let TypeKind::Record(record_ty) = &lhs_ty.kind {
                    if let Some(doc) = record_ty.get_doc(prop_name) {
                        cursor.set_description(doc.to_owned());
                    }
                }
            }
            let member_ty = if let Some(outer_name) = lhs_ty.name() {
                member_ty.with_name(format!("{outer_name}.{prop_name}"))
            } else {
                member_ty
            };
            return Ok(Diagnosed::new(member_ty, diags));
        }

        diags.push(crate::checker::UndefinedMember {
            module_id: env.module_id()?,
            name: self.property.name.clone(),
            ty: lhs_ty,
            property: self.property.clone(),
        });
        Ok(Diagnosed::new(Type::Never, diags))
    }

    pub fn eval(
        &self,
        evaluator: &crate::eval::Eval,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        use crate::Value;
        use crate::eval::Eval;

        let value = evaluator.eval_expr(env, self.expr.as_ref())?;
        match value.value {
            Value::Pending(_) => Ok(Eval::pending_with(value.dependencies)),
            Value::Record(record) => Ok(Eval::with_dependencies(
                record.get(self.property.name.as_str()).clone(),
                value.dependencies,
            )),
            _ => Ok(Eval::tracked(Value::Nil)),
        }
    }
}
