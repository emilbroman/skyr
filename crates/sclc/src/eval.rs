use std::collections::HashMap;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::{Dict, ExternFnValue, FnValue, PendingValue, Record, Value, ast};

pub struct EvalEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, &'a ast::FileMod)>>,
    locals: HashMap<&'a str, Value>,
    stack_depth: u32,
}

impl<'a> EvalEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            imports: None,
            locals: HashMap::new(),
            stack_depth: 0,
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_imports(
        &self,
        imports: &'a HashMap<&'a str, (crate::ModuleId, &'a ast::FileMod)>,
    ) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: Some(imports),
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_local(&self, name: &'a str, value: Value) -> Self {
        let mut env = self.inner();
        env.locals.insert(name, value);
        env
    }

    pub fn without_locals(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: HashMap::new(),
            stack_depth: self.stack_depth,
        }
    }

    pub fn with_stack_frame(&self) -> Result<Self, EvalError> {
        if self.stack_depth >= 50 {
            return Err(EvalError::StackOverflow);
        }

        let mut env = self.inner();
        env.stack_depth += 1;
        Ok(env)
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Value> {
        self.locals.get(name)
    }

    pub fn locals(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.locals.iter().map(|(name, value)| (*name, value))
    }

    pub fn lookup_global(&self, name: &str) -> Option<&crate::Loc<ast::Expr>> {
        self.globals.and_then(|globals| globals.get(name).copied())
    }

    pub fn lookup_import(&self, name: &str) -> Option<(crate::ModuleId, &'a ast::FileMod)> {
        self.imports
            .and_then(|imports| imports.get(name))
            .map(|(module_id, file_mod)| (module_id.clone(), *file_mod))
    }

    pub fn module_id(&self) -> Result<crate::ModuleId, EvalError> {
        self.module_id.cloned().ok_or(EvalError::ModuleIdMissing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnEnv {
    pub module_id: crate::ModuleId,
    pub captures: HashMap<String, Value>,
    pub parameters: Vec<String>,
}

impl FnEnv {
    pub fn as_eval_env<'a>(&'a self, args: &[Value]) -> EvalEnv<'a> {
        let mut env = EvalEnv::new().with_module_id(&self.module_id);

        for (name, value) in &self.captures {
            env = env.with_local(name.as_str(), value.clone());
        }
        for (name, value) in self.parameters.iter().zip(args.iter()) {
            env = env.with_local(name.as_str(), value.clone());
        }

        env
    }
}

pub struct Eval {
    ctx: EvalCtx,
    externs: HashMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    CreateResource {
        id: crate::ResourceId,
        inputs: crate::Record,
    },
    UpdateResource {
        id: crate::ResourceId,
        inputs: crate::Record,
    },
}

#[derive(Clone, Debug)]
pub struct EvalCtx {
    effects: mpsc::UnboundedSender<Effect>,
    resources: HashMap<crate::ResourceId, crate::Resource>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ListItemOutcome {
    Complete,
    Pending,
}

impl EvalCtx {
    pub fn emit(&self, effect: Effect) -> Result<(), EvalError> {
        self.effects
            .send(effect)
            .map_err(|send_error| EvalError::EmitEffect(send_error.0))
    }

    pub fn get_resource(
        &self,
        ty: impl Into<String>,
        id: impl Into<String>,
    ) -> Option<&crate::Resource> {
        let resource_id = crate::ResourceId {
            ty: ty.into(),
            id: id.into(),
        };
        self.resources.get(&resource_id)
    }

    pub fn resource(
        &self,
        ty: impl Into<String>,
        id: impl Into<String>,
        inputs: &crate::Record,
    ) -> Result<Option<crate::Record>, EvalError> {
        let ty = ty.into();
        let id = id.into();
        let resource_id = crate::ResourceId {
            ty: ty.clone(),
            id: id.clone(),
        };

        let Some(resource) = self.get_resource(ty, id) else {
            self.emit(Effect::CreateResource {
                id: resource_id,
                inputs: inputs.clone(),
            })?;
            return Ok(None);
        };

        if resource.inputs != *inputs {
            self.emit(Effect::UpdateResource {
                id: resource_id,
                inputs: inputs.clone(),
            })?;
            return Ok(None);
        }

        Ok(Some(resource.outputs.clone()))
    }
}

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("failed to emit effect: {0:?}")]
    EmitEffect(Effect),

    #[error("stack overflow")]
    StackOverflow,

    #[error("module id missing during evaluation")]
    ModuleIdMissing,

    #[error("extern not found: {0}")]
    MissingExtern(String),

    #[error("unexpected value: {0}")]
    UnexpectedValue(Value),

    #[error("invalid numeric result: {0}")]
    InvalidNumericResult(String),

    #[error("division by zero")]
    DivisionByZero,

    #[error("invalid comparison for {op}: {lhs} and {rhs}")]
    InvalidComparison {
        op: crate::ast::BinaryOp,
        lhs: Value,
        rhs: Value,
    },
}

pub trait ValueAssertions {
    fn assert_int(self) -> Result<i64, EvalError>;
    fn assert_str(self) -> Result<String, EvalError>;
}

impl ValueAssertions for Value {
    fn assert_int(self) -> Result<i64, EvalError> {
        match self {
            Value::Int(value) => Ok(value),
            other => Err(EvalError::UnexpectedValue(other)),
        }
    }

    fn assert_str(self) -> Result<String, EvalError> {
        match self {
            Value::Str(value) => Ok(value),
            other => Err(EvalError::UnexpectedValue(other)),
        }
    }
}

impl ValueAssertions for Option<Value> {
    fn assert_int(self) -> Result<i64, EvalError> {
        self.unwrap_or(Value::Nil).assert_int()
    }

    fn assert_str(self) -> Result<String, EvalError> {
        self.unwrap_or(Value::Nil).assert_str()
    }
}

impl Eval {
    pub fn new<S: crate::SourceRepo>(effects: mpsc::UnboundedSender<Effect>) -> Self {
        let mut eval = Self {
            ctx: EvalCtx {
                effects,
                resources: HashMap::new(),
            },
            externs: HashMap::new(),
        };
        <crate::AnySource<S> as crate::SourceRepo>::register_extern(&mut eval);
        eval
    }

    pub fn add_extern(&mut self, name: impl Into<String>, value: Value) {
        self.externs.insert(name.into(), value);
    }

    pub fn add_resource(&mut self, id: crate::ResourceId, resource: crate::Resource) {
        self.ctx.resources.insert(id, resource);
    }

    pub fn add_extern_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<Value>, &EvalCtx) -> Result<Value, EvalError> + Clone + Send + Sync + 'static,
    ) {
        self.add_extern(name, Value::ExternFn(ExternFnValue::new(Box::new(f))));
    }

    pub fn eval_expr(
        &self,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<Value, EvalError> {
        match expr.as_ref() {
            ast::Expr::Int(int) => Ok(Value::Int(int.value)),
            ast::Expr::Float(float) => Ok(Value::Float(float.value)),
            ast::Expr::Bool(bool) => Ok(Value::Bool(bool.value)),
            ast::Expr::Nil => Ok(Value::Nil),
            ast::Expr::Str(str) => Ok(Value::Str(str.value.clone())),
            ast::Expr::Extern(extern_expr) => self
                .externs
                .get(extern_expr.name.as_str())
                .cloned()
                .ok_or_else(|| EvalError::MissingExtern(extern_expr.name.clone())),
            ast::Expr::If(if_expr) => {
                let condition = self.eval_expr(env, if_expr.condition.as_ref())?;
                match condition {
                    Value::Pending(_) => Ok(Value::Pending(PendingValue)),
                    Value::Bool(true) => self.eval_expr(env, if_expr.then_expr.as_ref()),
                    Value::Bool(false) => {
                        if let Some(else_expr) = &if_expr.else_expr {
                            self.eval_expr(env, else_expr.as_ref())
                        } else {
                            Ok(Value::Nil)
                        }
                    }
                    other => Err(EvalError::UnexpectedValue(other)),
                }
            }
            ast::Expr::Let(let_expr) => {
                let bind_value = self.eval_expr(env, let_expr.bind.expr.as_ref())?;
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_value);
                self.eval_expr(&inner_env, let_expr.expr.as_ref())
            }
            ast::Expr::Fn(fn_expr) => {
                let mut captures = HashMap::new();
                for name in expr.as_ref().free_vars() {
                    captures.insert(name.to_owned(), self.eval_var_name(env, name)?);
                }
                Ok(Value::Fn(FnValue {
                    env: FnEnv {
                        module_id: env.module_id()?,
                        captures,
                        parameters: fn_expr
                            .params
                            .iter()
                            .map(|param| param.var.name.clone())
                            .collect(),
                    },
                    body: *fn_expr.body.clone(),
                }))
            }
            ast::Expr::Call(call_expr) => {
                let args = call_expr
                    .args
                    .iter()
                    .map(|arg| self.eval_expr(env, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let callee = self.eval_expr(env, call_expr.callee.as_ref())?;
                if matches!(callee, Value::Pending(_)) {
                    return Ok(Value::Pending(PendingValue));
                }

                match callee {
                    Value::Fn(function) => {
                        let call_env = function.env.as_eval_env(&args);
                        self.eval_expr(&call_env, &function.body)
                    }
                    Value::ExternFn(function) => {
                        if args.iter().any(|arg| matches!(arg, Value::Pending(_))) {
                            return Ok(Value::Pending(PendingValue));
                        }
                        function.call(args, &self.ctx)
                    }
                    _ => Ok(Value::Nil),
                }
            }
            ast::Expr::Unary(unary_expr) => {
                let value = self.eval_expr(env, unary_expr.expr.as_ref())?;

                if matches!(value, Value::Pending(_)) {
                    return Ok(Value::Pending(PendingValue));
                }

                match unary_expr.op {
                    ast::UnaryOp::Negate => match value {
                        Value::Int(value) => Ok(Value::Int(-value)),
                        Value::Float(value) => Ok(Value::Float(
                            ordered_float::NotNan::new(-value.into_inner()).map_err(|_| {
                                EvalError::InvalidNumericResult("unary - produced NaN".into())
                            })?,
                        )),
                        other => Err(EvalError::UnexpectedValue(other)),
                    },
                }
            }
            ast::Expr::Binary(binary_expr) => {
                let lhs = self.eval_expr(env, binary_expr.lhs.as_ref())?;

                if matches!(lhs, Value::Pending(_)) {
                    return Ok(Value::Pending(PendingValue));
                }

                match binary_expr.op {
                    ast::BinaryOp::And => match lhs {
                        Value::Bool(false) => Ok(Value::Bool(false)),
                        Value::Bool(true) => {
                            let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                            if matches!(rhs, Value::Pending(_)) {
                                return Ok(Value::Pending(PendingValue));
                            }
                            match rhs {
                                Value::Bool(value) => Ok(Value::Bool(value)),
                                other => Err(EvalError::UnexpectedValue(other)),
                            }
                        }
                        other => Err(EvalError::UnexpectedValue(other)),
                    },
                    ast::BinaryOp::Or => match lhs {
                        Value::Bool(true) => Ok(Value::Bool(true)),
                        Value::Bool(false) => {
                            let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                            if matches!(rhs, Value::Pending(_)) {
                                return Ok(Value::Pending(PendingValue));
                            }
                            match rhs {
                                Value::Bool(value) => Ok(Value::Bool(value)),
                                other => Err(EvalError::UnexpectedValue(other)),
                            }
                        }
                        other => Err(EvalError::UnexpectedValue(other)),
                    },
                    _ => {
                        let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                        if matches!(rhs, Value::Pending(_)) {
                            return Ok(Value::Pending(PendingValue));
                        }
                        match binary_expr.op {
                            ast::BinaryOp::Add => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs + rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Float(lhs + rhs))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs as f64 + rhs.into_inner())
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "int + float produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs.into_inner() + rhs as f64)
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "float + int produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (Value::Str(mut lhs), Value::Str(rhs)) => {
                                    lhs.push_str(&rhs);
                                    Ok(Value::Str(lhs))
                                }
                                (lhs, _) => Err(EvalError::UnexpectedValue(lhs)),
                            },
                            ast::BinaryOp::Sub => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs - rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Float(lhs - rhs))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs as f64 - rhs.into_inner())
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "int - float produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs.into_inner() - rhs as f64)
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "float - int produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (lhs, _) => Err(EvalError::UnexpectedValue(lhs)),
                            },
                            ast::BinaryOp::Mul => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs * rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Float(lhs * rhs))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs as f64 * rhs.into_inner())
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "int * float produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                                    ordered_float::NotNan::new(lhs.into_inner() * rhs as f64)
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "float * int produced NaN".into(),
                                            )
                                        })?,
                                )),
                                (lhs, _) => Err(EvalError::UnexpectedValue(lhs)),
                            },
                            ast::BinaryOp::Div => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => {
                                    if rhs == 0 {
                                        return Err(EvalError::DivisionByZero);
                                    }
                                    Ok(Value::Int(lhs / rhs))
                                }
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    if rhs.into_inner() == 0.0 {
                                        return Err(EvalError::DivisionByZero);
                                    }
                                    Ok(Value::Float(
                                        ordered_float::NotNan::new(
                                            lhs.into_inner() / rhs.into_inner(),
                                        )
                                        .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "float / float produced NaN".into(),
                                            )
                                        })?,
                                    ))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => {
                                    if rhs.into_inner() == 0.0 {
                                        return Err(EvalError::DivisionByZero);
                                    }
                                    Ok(Value::Float(
                                        ordered_float::NotNan::new(lhs as f64 / rhs.into_inner())
                                            .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "int / float produced NaN".into(),
                                            )
                                        })?,
                                    ))
                                }
                                (Value::Float(lhs), Value::Int(rhs)) => {
                                    if rhs == 0 {
                                        return Err(EvalError::DivisionByZero);
                                    }
                                    Ok(Value::Float(
                                        ordered_float::NotNan::new(lhs.into_inner() / rhs as f64)
                                            .map_err(|_| {
                                            EvalError::InvalidNumericResult(
                                                "float / int produced NaN".into(),
                                            )
                                        })?,
                                    ))
                                }
                                (lhs, _) => Err(EvalError::UnexpectedValue(lhs)),
                            },
                            ast::BinaryOp::Eq => Ok(Value::Bool(lhs == rhs)),
                            ast::BinaryOp::Neq => Ok(Value::Bool(lhs != rhs)),
                            ast::BinaryOp::Lt => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs < rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() < rhs.into_inner()))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool((lhs as f64) < rhs.into_inner()))
                                }
                                (Value::Float(lhs), Value::Int(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() < rhs as f64))
                                }
                                (lhs, rhs) => Err(EvalError::InvalidComparison {
                                    op: binary_expr.op,
                                    lhs,
                                    rhs,
                                }),
                            },
                            ast::BinaryOp::Lte => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs <= rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() <= rhs.into_inner()))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool((lhs as f64) <= rhs.into_inner()))
                                }
                                (Value::Float(lhs), Value::Int(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() <= rhs as f64))
                                }
                                (lhs, rhs) => Err(EvalError::InvalidComparison {
                                    op: binary_expr.op,
                                    lhs,
                                    rhs,
                                }),
                            },
                            ast::BinaryOp::Gt => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs > rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() > rhs.into_inner()))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool((lhs as f64) > rhs.into_inner()))
                                }
                                (Value::Float(lhs), Value::Int(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() > rhs as f64))
                                }
                                (lhs, rhs) => Err(EvalError::InvalidComparison {
                                    op: binary_expr.op,
                                    lhs,
                                    rhs,
                                }),
                            },
                            ast::BinaryOp::Gte => match (lhs, rhs) {
                                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Bool(lhs >= rhs)),
                                (Value::Float(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() >= rhs.into_inner()))
                                }
                                (Value::Int(lhs), Value::Float(rhs)) => {
                                    Ok(Value::Bool((lhs as f64) >= rhs.into_inner()))
                                }
                                (Value::Float(lhs), Value::Int(rhs)) => {
                                    Ok(Value::Bool(lhs.into_inner() >= rhs as f64))
                                }
                                (lhs, rhs) => Err(EvalError::InvalidComparison {
                                    op: binary_expr.op,
                                    lhs,
                                    rhs,
                                }),
                            },
                            _ => unreachable!("handled above"),
                        }
                    }
                }
            }
            ast::Expr::Var(var) => self.eval_var_name(env, var.name.as_str()),
            ast::Expr::Record(record_expr) => {
                let mut record = Record::default();
                for field in &record_expr.fields {
                    let value = self.eval_expr(env, &field.expr)?;
                    record.insert(field.var.name.clone(), value);
                }
                Ok(Value::Record(record))
            }
            ast::Expr::Dict(dict_expr) => {
                let mut dict = Dict::default();
                for entry in &dict_expr.entries {
                    let key = self.eval_expr(env, &entry.key)?;
                    let value = self.eval_expr(env, &entry.value)?;
                    if matches!(key, Value::Pending(_)) || matches!(value, Value::Pending(_)) {
                        return Ok(Value::Pending(PendingValue));
                    }
                    dict.insert(key, value);
                }
                Ok(Value::Dict(dict))
            }
            ast::Expr::List(list_expr) => {
                let mut values = Vec::new();
                for item in &list_expr.items {
                    if matches!(
                        self.eval_list_item(env, item, &mut values)?,
                        ListItemOutcome::Pending
                    ) {
                        return Ok(Value::Pending(PendingValue));
                    }
                }
                Ok(Value::List(values))
            }
            ast::Expr::Interp(interp_expr) => {
                let mut out = String::new();
                for part in &interp_expr.parts {
                    let value = self.eval_expr(env, part)?;
                    if matches!(value, Value::Pending(_)) {
                        return Ok(Value::Pending(PendingValue));
                    }
                    out.push_str(&value.to_string());
                }
                Ok(Value::Str(out))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let value = self.eval_expr(env, property_access.expr.as_ref())?;
                match value {
                    Value::Pending(_) => Ok(Value::Pending(PendingValue)),
                    Value::Record(record) => Ok(record
                        .get(property_access.property.name.as_str())
                        .cloned()
                        .unwrap_or(Value::Nil)),
                    _ => Ok(Value::Nil),
                }
            }
        }
    }

    fn eval_list_item(
        &self,
        env: &EvalEnv<'_>,
        item: &ast::ListItem,
        out: &mut Vec<Value>,
    ) -> Result<ListItemOutcome, EvalError> {
        match item {
            ast::ListItem::Expr(expr) => {
                out.push(self.eval_expr(env, expr)?);
                Ok(ListItemOutcome::Complete)
            }
            ast::ListItem::If(if_item) => {
                let condition = self.eval_expr(env, if_item.condition.as_ref())?;
                match condition {
                    Value::Bool(true) => self.eval_list_item(env, if_item.then_item.as_ref(), out),
                    Value::Bool(false) => Ok(ListItemOutcome::Complete),
                    Value::Pending(_) => Ok(ListItemOutcome::Pending),
                    other => Err(EvalError::UnexpectedValue(other)),
                }
            }
            ast::ListItem::For(for_item) => {
                let iterable = self.eval_expr(env, for_item.iterable.as_ref())?;
                match iterable {
                    Value::List(values) => {
                        for value in values {
                            let inner_env = env.with_local(for_item.var.name.as_str(), value);
                            if matches!(
                                self.eval_list_item(&inner_env, for_item.emit_item.as_ref(), out)?,
                                ListItemOutcome::Pending
                            ) {
                                return Ok(ListItemOutcome::Pending);
                            }
                        }
                        Ok(ListItemOutcome::Complete)
                    }
                    Value::Pending(_) => Ok(ListItemOutcome::Pending),
                    other => Err(EvalError::UnexpectedValue(other)),
                }
            }
        }
    }

    fn eval_var_name(&self, env: &EvalEnv<'_>, name: &str) -> Result<Value, EvalError> {
        if let Some(local_value) = env.lookup_local(name) {
            return Ok(local_value.clone());
        }
        if let Some(global_expr) = env.lookup_global(name) {
            let global_env = env.without_locals().with_stack_frame()?;
            return self.eval_expr(&global_env, global_expr);
        }
        if let Some((target_module_id, import_file_mod)) = env.lookup_import(name) {
            let import_env = EvalEnv::new().with_module_id(&target_module_id);
            return self.eval_file_mod(&import_env, import_file_mod);
        }
        Ok(Value::Nil)
    }

    pub fn eval_stmt(
        &self,
        env: &EvalEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Option<(String, Value)>, EvalError> {
        match stmt {
            ast::ModStmt::Let(_) | ast::ModStmt::Import(_) => Ok(None),
            ast::ModStmt::Export(let_bind) => {
                let value = self.eval_expr(env, let_bind.expr.as_ref())?;
                Ok(Some((let_bind.var.name.clone(), value)))
            }
            ast::ModStmt::Expr(expr) => {
                let _ = self.eval_expr(env, expr)?;
                Ok(None)
            }
        }
    }

    pub fn eval_file_mod(
        &self,
        env: &EvalEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Value, EvalError> {
        let globals = file_mod.find_globals();
        let env = env.with_globals(&globals);
        let mut exports = Record::default();

        for statement in &file_mod.statements {
            if let Some((name, value)) = self.eval_stmt(&env, statement)? {
                exports.insert(name, value);
            }
        }

        Ok(Value::Record(exports))
    }
}
