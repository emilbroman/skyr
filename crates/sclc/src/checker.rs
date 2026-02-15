use std::collections::HashMap;
use std::path::Component;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{DiagList, Diagnosed, Package, Program, RecordType, Type, ast};
use thiserror::Error;

pub struct TypeEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a ast::Expr>>,
    locals: HashMap<&'a str, Type>,
}

impl<'a> TypeEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            locals: HashMap::new(),
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            locals: self.locals.clone(),
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a ast::Expr>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            locals: HashMap::new(),
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            locals: self.locals.clone(),
        }
    }

    pub fn with_local(&self, name: &'a str, ty: Type) -> Self {
        let mut env = self.inner();
        env.locals.insert(name, ty);
        env
    }

    pub fn without_locals(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            locals: HashMap::new(),
        }
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Type> {
        self.locals.get(name)
    }

    pub fn lookup_global(&self, name: &str) -> Option<&ast::Expr> {
        self.globals.and_then(|globals| globals.get(name).copied())
    }

    pub fn module_id(&self) -> Result<crate::ModuleId, TypeCheckError> {
        self.module_id
            .cloned()
            .ok_or(TypeCheckError::ModuleIdMissing)
    }
}

pub struct TypeChecker;

static NEXT_TYPE_ID: AtomicUsize = AtomicUsize::new(0);

fn next_type_id() -> usize {
    NEXT_TYPE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Error, Debug)]
#[error("undefined variable: {name}")]
pub struct UndefinedVariable {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub var: crate::Loc<ast::Var>,
}

impl crate::Diag for UndefinedVariable {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.var.span())
    }
}

#[derive(Error, Debug)]
#[error("undefined member: {name}")]
pub struct UndefinedMember {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub property: crate::Loc<ast::Var>,
}

impl crate::Diag for UndefinedMember {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.property.span())
    }
}

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("type checking not implemented for statement: {0:?}")]
    UnimplementedStmt(ast::ModStmt),

    #[error("module id missing during type checking")]
    ModuleIdMissing,
}

impl TypeChecker {
    pub fn check_program<S: crate::SourceRepo>(
        &self,
        program: &Program<S>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let env = TypeEnv::new();
        let mut diags = DiagList::new();

        for (_, package) in program.packages() {
            self.check_package(&env, package)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_package<S: crate::SourceRepo>(
        &self,
        env: &TypeEnv<'_>,
        package: &Package<S>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let package_id = package.package_id();
        let mut diags = DiagList::new();

        for (path, file_mod) in package.modules() {
            let module_id = module_id_for_path(&package_id, path);
            let env = env.with_module_id(&module_id);
            self.check_file_mod(&env, file_mod)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_file_mod(
        &self,
        env: &TypeEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let globals = file_mod.find_globals();
        let env = env.with_globals(&globals);

        let mut diags = DiagList::new();

        for statement in &file_mod.statements {
            match statement {
                ast::ModStmt::Let(let_bind) => {
                    self.check_global_let_bind(&env, let_bind)?
                        .unpack(&mut diags);
                }
                _ => {
                    self.check_stmt(&env, statement)?.unpack(&mut diags);
                }
            }
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_stmt(
        &self,
        env: &TypeEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Expr(expr) => {
                let mut diags = DiagList::new();
                self.check_expr(env, expr)?.unpack(&mut diags);
                Ok(Diagnosed::new((), diags))
            }
            ast::ModStmt::Let(let_bind) => {
                let mut diags = DiagList::new();
                self.check_global_let_bind(env, let_bind)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new((), diags))
            }
            ast::ModStmt::Print(print_stmt) => {
                let mut diags = DiagList::new();
                self.check_expr(env, &print_stmt.expr)?.unpack(&mut diags);
                Ok(Diagnosed::new((), diags))
            }
            stmt => Err(TypeCheckError::UnimplementedStmt(stmt.clone())),
        }
    }

    pub fn check_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &ast::Expr,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match expr {
            ast::Expr::Int(_) => Ok(Diagnosed::new(Type::Int, DiagList::new())),
            ast::Expr::Let(let_expr) => {
                let mut diags = DiagList::new();
                let bind_ty = self
                    .check_expr(env, &let_expr.bind.expr)?
                    .unpack(&mut diags);
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_ty);
                let body_ty = self
                    .check_expr(&inner_env, &let_expr.expr)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(body_ty, diags))
            }
            ast::Expr::Var(var) => {
                if let Some(local_ty) = env.lookup_local(var.name.as_str()) {
                    return Ok(Diagnosed::new(local_ty.clone(), DiagList::new()));
                }
                if let Some(global_expr) = env.lookup_global(var.name.as_str()) {
                    let mut diags = DiagList::new();
                    let type_id = next_type_id();
                    let global_env = env
                        .without_locals()
                        .with_local(var.name.as_str(), Type::Var(type_id));
                    let resolved_ty = self
                        .check_expr(&global_env, global_expr)?
                        .unpack(&mut diags);
                    return Ok(Diagnosed::new(
                        Type::IsoRec(type_id, Box::new(resolved_ty)),
                        diags,
                    ));
                }
                let mut diags = DiagList::new();
                diags.push(UndefinedVariable {
                    module_id: env.module_id()?,
                    name: var.name.clone(),
                    var: var.clone(),
                });
                Ok(Diagnosed::new(Type::Never, diags))
            }
            ast::Expr::Record(record_expr) => {
                let mut diags = DiagList::new();
                let mut record_ty = RecordType::default();

                for field in &record_expr.fields {
                    let field_ty = self.check_expr(env, &field.expr)?.unpack(&mut diags);
                    record_ty.insert(field.var.name.clone(), field_ty);
                }

                Ok(Diagnosed::new(Type::Record(record_ty), diags))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let mut diags = DiagList::new();
                let lhs_ty = self
                    .check_expr(env, &property_access.expr)?
                    .unpack(&mut diags);
                let member_ty = match lhs_ty {
                    Type::Record(record_ty) => record_ty
                        .get(property_access.property.name.as_str())
                        .cloned(),
                    _ => None,
                };
                if let Some(member_ty) = member_ty {
                    return Ok(Diagnosed::new(member_ty, diags));
                }

                diags.push(UndefinedMember {
                    module_id: env.module_id()?,
                    name: property_access.property.name.clone(),
                    property: property_access.property.clone(),
                });
                Ok(Diagnosed::new(Type::Never, diags))
            }
        }
    }

    fn check_global_let_bind(
        &self,
        env: &TypeEnv<'_>,
        let_bind: &ast::LetBind,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let type_id = next_type_id();
        let env = env.with_local(let_bind.var.name.as_str(), Type::Var(type_id));
        let resolved_ty = self.check_expr(&env, &let_bind.expr)?.unpack(&mut diags);
        Ok(Diagnosed::new(
            Type::IsoRec(type_id, Box::new(resolved_ty)),
            diags,
        ))
    }
}

fn module_id_for_path(package_id: &crate::ModuleId, path: &Path) -> crate::ModuleId {
    let mut segments = package_id.as_slice().to_vec();
    if let Some(parent) = path.parent() {
        for segment in parent.components() {
            if let Component::Normal(part) = segment {
                segments.push(part.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(stem) = path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }

    segments.into_iter().collect()
}
