use std::collections::HashSet;

use super::{Expr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccessExpr {
    pub expr: Box<Loc<Expr>>,
    pub property: Loc<Var>,
    pub optional: bool,
}

impl PropertyAccessExpr {
    pub fn free_vars(&self) -> HashSet<&str> {
        self.expr.as_ref().free_vars()
    }

    pub fn type_synth(
        &self,
        checker: &crate::checker::TypeChecker<'_>,
        env: &crate::checker::TypeEnv<'_>,
    ) -> Result<crate::TypeSynth, crate::checker::TypeCheckError> {
        use crate::GlobalKey;
        use crate::{DiagList, Diagnosed, RecordType, Type, TypeKind};

        // When the LHS is a variable that resolves to an import alias and the
        // target is a `.scl` module, look up the property directly as a global
        // in the target module.  This handles cross-module recursive groups
        // where the full module-value record has not been assembled yet.
        //
        // For SCLE targets the alias resolves to the module's body value
        // (which has no named globals), so we fall through to ordinary record
        // member access below.
        if let Expr::Var(var) = self.expr.as_ref().as_ref()
            && let Some(raw_id) = env.raw_module_id()
            && let Some(target_raw_id) = env.global_env.resolve_import_alias(&var.name, raw_id)
            && !env.global_env.is_scle_module(target_raw_id)
        {
            let prop_name = self.property.name.as_str();
            let global_key = GlobalKey::Global(target_raw_id.clone(), prop_name.to_string());
            if let Some(ty) = env.global_env.get(&global_key) {
                let ty = ty.clone();
                if let Some((cursor, _)) = &self.property.cursor {
                    cursor.set_type(ty.clone());
                    cursor.set_identifier(crate::CursorIdentifier::Let(prop_name.into()));
                }
                return Ok(crate::TypeSynth::new(Diagnosed::new(ty, DiagList::new())));
            }
        }

        let mut diags = DiagList::new();
        let (raw_lhs_ty, mut props) = checker
            .synth_expr(env, self.expr.as_ref())?
            .unpack_with_props(&mut diags);
        let raw_lhs_ty = raw_lhs_ty.unfold();
        let lhs_ty = env.resolve_var_bound(&raw_lhs_ty).unfold();
        if matches!(lhs_ty.kind, TypeKind::Never) {
            return Ok(crate::TypeSynth::with_props(
                Diagnosed::new(Type::Never(), diags),
                props,
            ));
        }

        if self.optional {
            // Optional chaining: LHS must be optional
            if let TypeKind::Optional(inner) = &lhs_ty.kind {
                let inner_ty = inner.as_ref().clone();
                let prop_name = self.property.name.as_str();

                // Completion candidates
                if let Some((cursor, offset)) = &self.property.cursor {
                    let prefix = &self.property.name[..*offset];
                    if let TypeKind::Record(record_ty) = &inner_ty.unfold().kind {
                        for (name, field_ty) in record_ty.iter() {
                            if name.starts_with(prefix) {
                                cursor.add_completion_candidate(
                                    crate::CompletionCandidate::Member(crate::CompletionMember {
                                        name: name.clone(),
                                        description: record_ty.get_doc(name).map(str::to_owned),
                                        ty: Some(field_ty.clone()),
                                    }),
                                );
                            }
                        }
                    }
                }

                let resolved = inner_ty.unfold();
                let member_ty = match &resolved.kind {
                    TypeKind::Record(record_ty) => record_ty.get(prop_name).cloned(),
                    _ => None,
                };
                if let Some(member_ty) = member_ty {
                    if let Some((cursor, _)) = &self.property.cursor {
                        cursor.set_type(member_ty.clone());
                        cursor.set_identifier(crate::CursorIdentifier::Let(prop_name.into()));
                    }

                    // Determine the inner type for the fresh Optional wrapper.
                    // If the field is itself optional, flatten: reuse the field's
                    // inner TypeId. Otherwise, the inner type IS the field type.
                    let (result_inner, field_is_optional) =
                        if let TypeKind::Optional(field_inner) = &member_ty.kind {
                            (field_inner.as_ref().clone(), true)
                        } else {
                            (member_ty.clone(), false)
                        };

                    // Create fresh Optional wrapper reusing inner TypeId.
                    let result_ty = Type::Optional(Box::new(result_inner.clone()));
                    let result_id = result_ty.id();
                    let source_id = lhs_ty.id();

                    // Source unwrap: if result refines, source is non-nil.
                    let refines_result = crate::Prop::RefinesTo(result_id, result_inner.clone());
                    props.push(
                        refines_result
                            .clone()
                            .implies(crate::Prop::RefinesTo(source_id, inner_ty.clone())),
                    );

                    // Field unwrap: if result refines and field is optional, field is also non-nil.
                    if field_is_optional {
                        let field_id = member_ty.id();
                        props.push(
                            refines_result.implies(crate::Prop::RefinesTo(field_id, result_inner)),
                        );
                    }

                    return Ok(crate::TypeSynth::with_props(
                        Diagnosed::new(result_ty, diags),
                        props,
                    ));
                }

                diags.push(crate::checker::UndefinedMember {
                    module_id: env.module_id()?,
                    name: self.property.name.clone(),
                    ty: inner_ty,
                    property: self.property.clone(),
                });
                return Ok(crate::TypeSynth::with_props(
                    Diagnosed::new(Type::Never(), diags),
                    props,
                ));
            } else {
                // Optional chaining on non-optional type
                diags.push(crate::checker::OptionalChainOnNonOptional {
                    module_id: env.module_id()?,
                    ty: lhs_ty.clone(),
                    span: self.property.span(),
                });
                return Ok(crate::TypeSynth::with_props(
                    Diagnosed::new(Type::Never(), diags),
                    props,
                ));
            }
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
            return Ok(crate::TypeSynth::new(Diagnosed::new(member_var, diags)));
        }

        // Completion candidates for property access
        if let Some((cursor, offset)) = &self.property.cursor {
            let prefix = &self.property.name[..*offset];
            if let TypeKind::Record(record_ty) = &lhs_ty.kind {
                for (name, field_ty) in record_ty.iter() {
                    if name.starts_with(prefix) {
                        cursor.add_completion_candidate(crate::CompletionCandidate::Member(
                            crate::CompletionMember {
                                name: name.clone(),
                                description: record_ty.get_doc(name).map(str::to_owned),
                                ty: Some(field_ty.clone()),
                            },
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
                    if let Some((module, span)) = record_ty.get_origin(prop_name) {
                        cursor.set_declaration(module.clone(), *span);
                    }
                }
            }
            // Track this property access as a reference to the field's
            // declaration (for find-all-references). The cursor buffers by
            // declaration key and flushes into `references` when its own
            // declaration is set to a matching location.
            if let Some(env_cursor) = &env.cursor
                && let TypeKind::Record(record_ty) = &lhs_ty.kind
                && let Some((origin_module, origin_span)) = record_ty.get_origin(prop_name)
                && let Some(ref_module) = env.raw_module_id()
            {
                env_cursor.track_reference(
                    (origin_module.clone(), *origin_span),
                    (ref_module.clone(), self.property.span()),
                );
            }
            return Ok(crate::TypeSynth::with_props(
                Diagnosed::new(member_ty, diags),
                props,
            ));
        }

        diags.push(crate::checker::UndefinedMember {
            module_id: env.module_id()?,
            name: self.property.name.clone(),
            ty: lhs_ty,
            property: self.property.clone(),
        });
        Ok(crate::TypeSynth::with_props(
            Diagnosed::new(Type::Never(), diags),
            props,
        ))
    }

    pub fn eval(
        &self,
        evaluator: &crate::eval::Eval<'_>,
        env: &crate::eval::EvalEnv<'_>,
        expr: &crate::Loc<Expr>,
    ) -> Result<crate::TrackedValue, crate::eval::EvalError> {
        use crate::eval::EvalErrorKind;
        use crate::{GlobalKey, Value};

        // When the LHS is an import alias targeting a `.scl` module, resolve
        // the property directly as a global in the target module (mirrors the
        // type-checking bypass). For SCLE targets, fall through to ordinary
        // property access on the module's body value.
        if let Expr::Var(var) = self.expr.as_ref().as_ref()
            && let Some(raw_id) = env.raw_module_id()
            && let Some(target_raw_id) = env.global_env.resolve_import_alias(&var.name, raw_id)
            && !env.global_env.is_scle_module(target_raw_id)
        {
            let global_key =
                GlobalKey::Global(target_raw_id.clone(), self.property.name.to_string());
            if let Some(val) = env.global_env.get(&global_key) {
                return Ok(val.clone());
            }
        }

        let value = evaluator.eval_expr(env, self.expr.as_ref())?;
        match value.value {
            Value::Pending(_) => Ok(crate::eval::pending_with(value.dependencies)),
            Value::Nil if self.optional => Ok(crate::eval::with_dependencies(
                Value::Nil,
                value.dependencies,
            )),
            Value::Record(record) => Ok(crate::eval::with_dependencies(
                record.get(self.property.name.as_str()).clone(),
                value.dependencies,
            )),
            other => Err(env.throw(
                EvalErrorKind::UnexpectedValue(other),
                Some((
                    env.module_id.cloned().unwrap_or_default(),
                    expr.span(),
                    "property access".to_string(),
                )),
            )),
        }
    }
}
