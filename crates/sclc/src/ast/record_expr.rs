use std::collections::{BTreeSet, HashSet};

use super::{Expr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordExpr {
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub doc_comment: Option<String>,
    pub var: Loc<Var>,
    pub expr: Loc<Expr>,
}

impl RecordExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        let mut vars = HashSet::new();
        for field in &self.fields {
            vars.extend(field.expr.as_ref().free_vars());
        }
        vars
    }

    #[inline(never)]
    pub(crate) fn type_synth(
        &self,
        checker: &crate::checker::TypeChecker<'_, impl crate::SourceRepo>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let mut record_ty = crate::RecordType::default();
        for field in &self.fields {
            let field_ty = checker.synth_expr(env, &field.expr)?.unpack(&mut diags);
            if let Some((cursor, _)) = &field.var.cursor {
                cursor.set_identifier(crate::CursorIdentifier::Let(field.var.name.clone()));
                cursor.set_type(field_ty.clone());
                if let Some(doc) = &field.doc_comment {
                    cursor.set_description(doc.clone());
                }
            }
            record_ty.insert_with_doc(field.var.name.clone(), field_ty, field.doc_comment.clone());
        }
        Ok(crate::Diagnosed::new(crate::Type::Record(record_ty), diags))
    }

    #[inline(never)]
    pub(crate) fn type_check(
        &self,
        checker: &crate::checker::TypeChecker<'_, impl crate::SourceRepo>,
        env: &crate::checker::TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &crate::Type,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        if let crate::TypeKind::Record(expected_record) = &expected.kind {
            return self.check_record_against(checker, env, expr, expected_record);
        }
        checker.synth_then_subsume(env, expr, expected)
    }

    #[inline(never)]
    fn check_record_against(
        &self,
        checker: &crate::checker::TypeChecker<'_, impl crate::SourceRepo>,
        env: &crate::checker::TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected_record: &crate::RecordType,
    ) -> Result<crate::Diagnosed<crate::Type>, crate::checker::TypeCheckError> {
        let mut diags = crate::DiagList::new();
        let mut record_ty = crate::RecordType::default();

        for field in &self.fields {
            // Completion candidates for record field names
            if let Some((cursor, offset)) = &field.var.cursor {
                let prefix = &field.var.name[..*offset];
                for (name, field_ty) in expected_record.iter() {
                    if name.starts_with(prefix) {
                        cursor.add_completion_candidate(crate::CompletionCandidate::Member(
                            crate::CompletionMember {
                                name: name.clone(),
                                description: expected_record.get_doc(name).map(str::to_owned),
                                ty: Some(field_ty.clone()),
                            },
                        ));
                    }
                }
            }
            let expected_field_ty = expected_record.get(field.var.name.as_str());

            let field_description = field
                .doc_comment
                .as_ref()
                .map(String::as_str)
                .or_else(|| expected_record.get_doc(field.var.name.as_str()))
                .map(String::from);

            let field_ty = checker
                .check_expr(env, &field.expr, expected_field_ty)?
                .unpack(&mut diags);
            if let Some((cursor, _)) = &field.var.cursor {
                cursor.set_identifier(crate::CursorIdentifier::Let(field.var.name.clone()));
                cursor.set_type(field_ty.clone());
                if let Some(doc) = &field_description {
                    cursor.set_description(doc.clone());
                }
            }
            record_ty.insert_with_doc(field.var.name.clone(), field_ty, field_description);
        }

        let ty = crate::Type::Record(record_ty);

        // Check for missing required fields
        let missing_field = expected_record.iter().any(|(name, field_ty)| {
            matches!(&ty.kind, crate::TypeKind::Record(record) if record.get(name).is_none())
                && !matches!(field_ty.kind, crate::TypeKind::Optional(_))
        });
        if missing_field {
            diags.push(crate::checker::InvalidType {
                module_id: env.module_id()?,
                error: crate::TypeError::new(crate::TypeIssue::Mismatch(
                    crate::Type::Record(expected_record.clone()),
                    ty.clone(),
                )),
                span: expr.span(),
            });
        }

        // Check for unknown fields
        for field in &self.fields {
            if expected_record.get(field.var.name.as_str()).is_none() {
                diags.push(crate::checker::UnknownField {
                    module_id: env.module_id()?,
                    name: field.var.name.clone(),
                    span: field.var.span(),
                });
            }
        }

        Ok(crate::Diagnosed::new(ty, diags))
    }

    pub(crate) fn eval(
        &self,
        evaluator: &crate::eval::Eval,
        env: &crate::eval::EvalEnv<'_>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        let mut record = crate::Record::default();
        let mut dependencies = BTreeSet::new();
        for field in &self.fields {
            let value = evaluator.eval_expr(env, &field.expr)?;
            dependencies.extend(value.dependencies.clone());
            record.insert(field.var.name.clone(), value.value);
        }
        Ok(crate::eval::Eval::with_dependencies(
            crate::Value::Record(record),
            dependencies,
        ))
    }
}
