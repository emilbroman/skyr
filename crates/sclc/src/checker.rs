use crate::{DiagList, Diagnosed, Package, Program, Type, ast};
use thiserror::Error;

pub struct TypeEnv;

pub struct TypeChecker;

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("type checking not implemented for expression: {0:?}")]
    UnimplementedExpr(ast::Expr),

    #[error("type checking not implemented for statement: {0:?}")]
    UnimplementedStmt(ast::ModStmt),
}

impl TypeChecker {
    pub fn check_program<S>(&self, program: &Program<S>) -> Result<Diagnosed<()>, TypeCheckError> {
        let env = TypeEnv;
        let mut diags = DiagList::new();

        for (_, package) in program.packages() {
            self.check_package(&env, package)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_package<S>(
        &self,
        env: &TypeEnv,
        package: &Package<S>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let mut diags = DiagList::new();

        for (_, file_mod) in package.modules() {
            self.check_file_mod(env, file_mod)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_file_mod(
        &self,
        env: &TypeEnv,
        file_mod: &ast::FileMod,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let mut diags = DiagList::new();

        for statement in &file_mod.statements {
            self.check_stmt(env, statement)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_stmt(
        &self,
        env: &TypeEnv,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Expr(expr) => {
                let mut diags = DiagList::new();
                self.check_expr(env, expr)?.unpack(&mut diags);
                Ok(Diagnosed::new((), diags))
            }
            stmt => Err(TypeCheckError::UnimplementedStmt(stmt.clone())),
        }
    }

    pub fn check_expr(
        &self,
        _env: &TypeEnv,
        expr: &ast::Expr,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match expr {
            ast::Expr::Int(_) => Ok(Diagnosed::new(Type::Int, DiagList::new())),
            expr => Err(TypeCheckError::UnimplementedExpr(expr.clone())),
        }
    }
}
