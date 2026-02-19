use std::collections::HashMap;
use std::path::Component;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{AnySource, DiagList, Diagnosed, FnType, Package, Program, RecordType, Type, ast};
use thiserror::Error;

pub struct TypeEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>>,
    locals: HashMap<&'a str, Type>,
}

impl<'a> TypeEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            imports: None,
            locals: HashMap::new(),
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            locals: HashMap::new(),
        }
    }

    pub fn with_imports(
        &self,
        imports: &'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>,
    ) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: Some(imports),
            locals: HashMap::new(),
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
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
            imports: self.imports,
            locals: HashMap::new(),
        }
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Type> {
        self.locals.get(name)
    }

    pub fn lookup_global(&self, name: &str) -> Option<&crate::Loc<ast::Expr>> {
        self.globals.and_then(|globals| globals.get(name).copied())
    }

    pub fn lookup_import(&self, name: &str) -> Option<(crate::ModuleId, Option<&'a ast::FileMod>)> {
        self.imports
            .and_then(|imports| imports.get(name))
            .map(|(module_id, file_mod)| (module_id.clone(), *file_mod))
    }

    pub fn module_id(&self) -> Result<crate::ModuleId, TypeCheckError> {
        self.module_id
            .cloned()
            .ok_or(TypeCheckError::ModuleIdMissing)
    }
}

pub struct TypeChecker<'p, S> {
    program: &'p Program<S>,
}

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
#[error("undefined member: {name} in type {ty}")]
pub struct UndefinedMember {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub ty: Type,
    pub property: crate::Loc<ast::Var>,
}

impl crate::Diag for UndefinedMember {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.property.span())
    }
}

#[derive(Error, Debug)]
#[error("not a function: {ty}")]
pub struct NotAFunction {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for NotAFunction {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("missing arguments: expected {expected}, got {got}")]
pub struct MissingArguments {
    pub module_id: crate::ModuleId,
    pub expected: usize,
    pub got: usize,
    pub span: crate::Span,
}

impl crate::Diag for MissingArguments {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("extraneous argument at index {index}")]
pub struct ExtraneousArgument {
    pub module_id: crate::ModuleId,
    pub index: usize,
    pub span: crate::Span,
}

impl crate::Diag for ExtraneousArgument {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("type mismatch: expected {expected}, got {actual}")]
pub struct TypeMismatch {
    pub module_id: crate::ModuleId,
    pub expected: Type,
    pub actual: Type,
    pub span: crate::Span,
}

impl crate::Diag for TypeMismatch {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("module id missing during type checking")]
    ModuleIdMissing,
}

impl<'p, S: crate::SourceRepo> TypeChecker<'p, S> {
    pub fn new(program: &'p Program<S>) -> Self {
        Self { program }
    }

    pub fn check_program(&self) -> Result<Diagnosed<()>, TypeCheckError> {
        let env = TypeEnv::new();
        let mut diags = DiagList::new();

        for (_, package) in self.program.packages() {
            self.check_package(&env, package)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_package(
        &self,
        env: &TypeEnv<'_>,
        package: &Package<AnySource<S>>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let package_id = package.package_id();
        let mut diags = DiagList::new();

        for (path, file_mod) in package.modules() {
            let module_id = module_id_for_path(&package_id, path);
            let env = env.with_module_id(&module_id);
            let _ = self.check_file_mod(&env, file_mod)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_file_mod(
        &self,
        env: &TypeEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let globals = file_mod.find_globals();
        let imports = self.find_imports(file_mod);
        let env = env.with_globals(&globals).with_imports(&imports);

        let mut diags = DiagList::new();
        let mut exports = RecordType::default();

        for statement in &file_mod.statements {
            if let Some((name, ty)) = self.check_stmt(&env, statement)?.unpack(&mut diags) {
                exports.insert(name, ty);
            }
        }

        Ok(Diagnosed::new(Type::Record(exports), diags))
    }

    pub fn check_stmt(
        &self,
        env: &TypeEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<Option<(String, Type)>>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Import(_) => Ok(Diagnosed::new(None, DiagList::new())),
            ast::ModStmt::Expr(expr) => {
                let mut diags = DiagList::new();
                self.check_expr(env, expr)?.unpack(&mut diags);
                Ok(Diagnosed::new(None, diags))
            }
            ast::ModStmt::Let(let_bind) => {
                let mut diags = DiagList::new();
                self.check_global_let_bind(env, let_bind)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(None, diags))
            }
            ast::ModStmt::Export(let_bind) => {
                let mut diags = DiagList::new();
                let ty = self
                    .check_global_let_bind(env, let_bind)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(Some((let_bind.var.name.clone(), ty)), diags))
            }
            ast::ModStmt::Print(print_stmt) => {
                let mut diags = DiagList::new();
                self.check_expr(env, &print_stmt.expr)?.unpack(&mut diags);
                Ok(Diagnosed::new(None, diags))
            }
        }
    }

    pub fn check_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match expr.as_ref() {
            ast::Expr::Int(_) => Ok(Diagnosed::new(Type::Int, DiagList::new())),
            ast::Expr::Str(_) => Ok(Diagnosed::new(Type::Str, DiagList::new())),
            ast::Expr::Extern(extern_expr) => Ok(Diagnosed::new(
                self.resolve_type_expr(&extern_expr.ty),
                DiagList::new(),
            )),
            ast::Expr::Let(let_expr) => {
                let mut diags = DiagList::new();
                let bind_ty = self
                    .check_expr(env, let_expr.bind.expr.as_ref())?
                    .unpack(&mut diags);
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_ty);
                let body_ty = self
                    .check_expr(&inner_env, let_expr.expr.as_ref())?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(body_ty, diags))
            }
            ast::Expr::Fn(fn_expr) => {
                let mut diags = DiagList::new();
                let mut fn_env = env.inner();
                let mut params = Vec::with_capacity(fn_expr.params.len());

                for param in &fn_expr.params {
                    let param_ty = self.resolve_type_expr(&param.ty);
                    fn_env = fn_env.with_local(param.var.name.as_str(), param_ty.clone());
                    params.push(param_ty);
                }

                let ret = self
                    .check_expr(&fn_env, fn_expr.body.as_ref())?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(
                    Type::Fn(FnType {
                        params,
                        ret: Box::new(ret),
                    }),
                    diags,
                ))
            }
            ast::Expr::Call(call_expr) => {
                let mut diags = DiagList::new();
                let callee_ty = self
                    .check_expr(env, call_expr.callee.as_ref())?
                    .unpack(&mut diags)
                    .unfold();
                let Type::Fn(fn_ty) = callee_ty else {
                    diags.push(NotAFunction {
                        module_id: env.module_id()?,
                        ty: callee_ty,
                        span: call_expr.callee.span(),
                    });
                    return Ok(Diagnosed::new(Type::Never, diags));
                };

                if call_expr.args.len() < fn_ty.params.len() {
                    diags.push(MissingArguments {
                        module_id: env.module_id()?,
                        expected: fn_ty.params.len(),
                        got: call_expr.args.len(),
                        span: call_expr.callee.span(),
                    });
                }

                for (index, arg) in call_expr.args.iter().enumerate() {
                    let arg_ty = self.check_expr(env, arg)?.unpack(&mut diags);
                    let Some(param_ty) = fn_ty.params.get(index) else {
                        diags.push(ExtraneousArgument {
                            module_id: env.module_id()?,
                            index,
                            span: arg.span(),
                        });
                        continue;
                    };

                    if &arg_ty != param_ty {
                        diags.push(TypeMismatch {
                            module_id: env.module_id()?,
                            expected: param_ty.clone(),
                            actual: arg_ty,
                            span: arg.span(),
                        });
                    }
                }

                Ok(Diagnosed::new(*fn_ty.ret, diags))
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
                if let Some((target_module_id, maybe_import_file_mod)) =
                    env.lookup_import(var.name.as_str())
                {
                    let Some(import_file_mod) = maybe_import_file_mod else {
                        return Ok(Diagnosed::new(Type::Never, DiagList::new()));
                    };
                    let import_env = TypeEnv::new().with_module_id(&target_module_id);
                    let imported_ty = self.check_file_mod(&import_env, import_file_mod)?;
                    return Ok(Diagnosed::new(imported_ty.into_inner(), DiagList::new()));
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
            ast::Expr::Interp(interp_expr) => {
                let mut diags = DiagList::new();
                for part in &interp_expr.parts {
                    self.check_expr(env, part)?.unpack(&mut diags);
                }
                Ok(Diagnosed::new(Type::Str, diags))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let mut diags = DiagList::new();
                let lhs_ty = self
                    .check_expr(env, property_access.expr.as_ref())?
                    .unpack(&mut diags)
                    .unfold();
                if matches!(lhs_ty, Type::Never) {
                    return Ok(Diagnosed::new(Type::Never, diags));
                }
                let member_ty = match &lhs_ty {
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
                    ty: lhs_ty,
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
        let resolved_ty = self
            .check_expr(&env, let_bind.expr.as_ref())?
            .unpack(&mut diags);
        Ok(Diagnosed::new(
            Type::IsoRec(type_id, Box::new(resolved_ty)),
            diags,
        ))
    }

    fn resolve_type_expr(&self, type_expr: &crate::Loc<ast::TypeExpr>) -> Type {
        match type_expr.as_ref() {
            ast::TypeExpr::Var(var) if var.name == "Int" => Type::Int,
            ast::TypeExpr::Var(var) if var.name == "Str" => Type::Str,
            ast::TypeExpr::Fn(fn_ty) => Type::Fn(FnType {
                params: fn_ty
                    .params
                    .iter()
                    .map(|param| self.resolve_type_expr(param))
                    .collect(),
                ret: Box::new(self.resolve_type_expr(&fn_ty.ret)),
            }),
            ast::TypeExpr::Record(record_ty) => {
                let mut resolved = RecordType::default();
                for field in &record_ty.fields {
                    resolved.insert(field.var.name.clone(), self.resolve_type_expr(&field.ty));
                }
                Type::Record(resolved)
            }
            ast::TypeExpr::Var(_) => Type::Never,
        }
    }

    fn find_imports<'a>(
        &'a self,
        file_mod: &'a ast::FileMod,
    ) -> HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)> {
        file_mod
            .statements
            .iter()
            .filter_map(|statement| {
                if let ast::ModStmt::Import(import_stmt) = statement {
                    let alias = import_stmt
                        .as_ref()
                        .vars
                        .last()
                        .expect("import path contains at least one segment");
                    let import_path = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect::<crate::ModuleId>();
                    let destination = self.resolve_import(import_stmt);
                    return Some((alias.name.as_str(), (import_path, destination)));
                }
                None
            })
            .collect()
    }

    fn resolve_import<'a>(
        &'a self,
        import_stmt: &'a crate::Loc<ast::ImportStmt>,
    ) -> Option<&'a ast::FileMod> {
        let import_path = import_stmt
            .as_ref()
            .vars
            .iter()
            .map(|var| var.name.clone())
            .collect::<crate::ModuleId>();
        let package_name = self.package_name_for_import(&import_path)?;
        let (_, package) = self
            .program
            .packages()
            .find(|(name, _)| *name == &package_name)?;
        let module_segments = import_path.suffix_after(&package_name)?;
        if module_segments.is_empty() {
            return None;
        }
        let module_path = module_segments
            .iter()
            .cloned()
            .collect::<crate::ModuleId>()
            .to_path_buf_with_extension("scl");
        package
            .modules()
            .find_map(|(path, file_mod)| (path == &module_path).then_some(file_mod))
    }

    fn package_name_for_import(&self, import_path: &crate::ModuleId) -> Option<crate::ModuleId> {
        self.program
            .packages()
            .map(|(name, _)| name)
            .filter(|package_name| import_path.starts_with(package_name))
            .max_by_key(|package_name| package_name.len())
            .cloned()
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
