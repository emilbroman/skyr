use std::cell::RefCell;
use std::rc::Rc;

use crate::checker::{FreeVarConstraints, UndefinedVariable, next_type_id};
use crate::{DiagList, Diagnosed, SourceRepo, Type, TypeCheckError, TypeChecker, TypeEnv};

pub(crate) fn synth_var<S: SourceRepo>(
    checker: &TypeChecker<'_, S>,
    env: &TypeEnv<'_>,
    expr: &crate::Loc<super::Expr>,
    var: &crate::Loc<super::Var>,
) -> Result<Diagnosed<Type>, TypeCheckError> {
    // Completion candidates
    if let Some((cursor, offset)) = &var.cursor {
        let prefix = &var.name[..*offset];
        for name in env.local_names().chain(env.global_names()) {
            if name.starts_with(prefix) {
                cursor.add_completion_candidate(crate::CompletionCandidate::Var(name.to_owned()));
            }
        }
    }
    let set_cursor = |decl: crate::Span, ty: &Type| {
        if let Some((cursor, _)) = &var.cursor {
            cursor.set_declaration(decl);
            cursor.set_type(ty.clone());
        }
    };
    let track_ref = |decl: crate::Span| {
        if let Some(cursor) = &env.cursor {
            cursor.track_reference(decl, expr.span());
        }
    };

    // Local variable
    if let Some((decl, local_ty)) = env.lookup_local(var.name.as_str()) {
        let decl = *decl;
        let local_ty = local_ty.clone();
        track_ref(decl);
        set_cursor(decl, &local_ty);
        return Ok(Diagnosed::new(local_ty, DiagList::new()));
    }

    // Global variable
    if let Some((decl, global_expr)) = env.lookup_global(var.name.as_str()) {
        return synth_global(checker, env, expr, var, decl, global_expr);
    }

    // Import
    if let Some((target_module_id, maybe_import_file_mod)) = env.lookup_import(var.name.as_str()) {
        let Some(import_file_mod) = maybe_import_file_mod else {
            return Ok(Diagnosed::new(Type::Never, DiagList::new()));
        };
        let cache_key = import_file_mod as *const super::FileMod;
        let imported_ty = if let Some(cached_ty) = checker.import_cache.borrow().get(&cache_key) {
            cached_ty.clone()
        } else {
            let import_env = TypeEnv::new().with_module_id(&target_module_id);
            let imported_ty = checker.check_file_mod(&import_env, import_file_mod)?;
            let imported_ty = imported_ty.into_inner();
            checker
                .import_cache
                .borrow_mut()
                .insert(cache_key, imported_ty.clone());
            imported_ty
        };
        if let Some((cursor, _)) = &var.cursor {
            cursor.set_type(imported_ty.clone());
        }
        return Ok(Diagnosed::new(imported_ty, DiagList::new()));
    }

    // Undefined
    let mut diags = DiagList::new();
    diags.push(UndefinedVariable {
        module_id: env.module_id()?,
        name: var.name.clone(),
        var: var.clone(),
    });
    Ok(Diagnosed::new(Type::Never, diags))
}

pub(crate) fn synth_global<S: SourceRepo>(
    checker: &TypeChecker<'_, S>,
    env: &TypeEnv<'_>,
    expr: &crate::Loc<super::Expr>,
    var: &crate::Loc<super::Var>,
    decl: crate::Span,
    global_expr: &crate::Loc<super::Expr>,
) -> Result<Diagnosed<Type>, TypeCheckError> {
    let set_cursor = |decl: crate::Span, ty: &Type| {
        if let Some((cursor, _)) = &var.cursor {
            cursor.set_declaration(decl);
            cursor.set_type(ty.clone());
        }
    };
    let track_ref = |decl: crate::Span| {
        if let Some(cursor) = &env.cursor {
            cursor.track_reference(decl, expr.span());
        }
    };

    let mut diags = DiagList::new();
    let cache_key = global_expr as *const crate::Loc<super::Expr>;
    let resolved_ty = if let Some(cached_ty) = checker.global_cache.borrow().get(&cache_key) {
        cached_ty.clone()
    } else {
        let type_id = next_type_id();
        let constraints = Rc::new(RefCell::new(FreeVarConstraints::new()));
        let global_env = env.without_locals().with_free_var(
            var.name.as_str(),
            decl,
            type_id,
            constraints.clone(),
        );
        let resolved_ty = checker
            .synth_expr(&global_env, global_expr)?
            .unpack(&mut diags);
        let solved = constraints.borrow().solve(type_id, &resolved_ty);
        let resolved_ty = resolved_ty.substitute(&solved);
        checker
            .global_cache
            .borrow_mut()
            .insert(cache_key, resolved_ty.clone());
        resolved_ty
    };
    let type_id = next_type_id();
    let ty = Type::IsoRec(type_id, Box::new(resolved_ty));
    track_ref(decl);
    set_cursor(decl, &ty);
    Ok(Diagnosed::new(ty, diags))
}
