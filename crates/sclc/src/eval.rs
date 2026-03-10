use std::collections::{BTreeSet, HashMap};

use thiserror::Error;
use tokio::sync::mpsc;

use crate::{Dict, ExceptionValue, ExternFnValue, FnValue, Record, TrackedValue, Value, ast};

#[derive(Debug)]
pub struct StackFrame<'a> {
    pub module_id: crate::ModuleId,
    pub span: crate::Span,
    pub name: String,
    pub parent: Option<&'a StackFrame<'a>>,
}

impl StackFrame<'_> {
    fn depth(&self) -> u32 {
        let mut depth = 1;
        let mut frame = self.parent;
        while let Some(f) = frame {
            depth += 1;
            frame = f.parent;
        }
        depth
    }

    fn collect_trace(&self) -> Vec<(crate::ModuleId, crate::Span, String)> {
        let mut trace = vec![(self.module_id.clone(), self.span, self.name.clone())];
        let mut frame = self.parent;
        while let Some(f) = frame {
            trace.push((f.module_id.clone(), f.span, f.name.clone()));
            frame = f.parent;
        }
        trace
    }
}

pub struct EvalEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, &'a ast::FileMod)>>,
    locals: HashMap<&'a str, TrackedValue>,
    stack: Option<&'a StackFrame<'a>>,
}

impl<'a> Default for EvalEnv<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> EvalEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            imports: None,
            locals: HashMap::new(),
            stack: None,
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack: self.stack,
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            locals: HashMap::new(),
            stack: self.stack,
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
            stack: self.stack,
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            stack: self.stack,
        }
    }

    pub fn with_local(&self, name: &'a str, value: TrackedValue) -> Self {
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
            stack: self.stack,
        }
    }

    pub fn with_stack_frame(&self, frame: &'a StackFrame<'a>) -> Result<Self, EvalError> {
        if frame.depth() > 50 {
            return Err(self.throw(
                EvalErrorKind::StackOverflow,
                Some((frame.module_id.clone(), frame.span, frame.name.clone())),
            ));
        }

        let mut env = self.inner();
        env.stack = Some(frame);
        Ok(env)
    }

    pub fn stack(&self) -> Option<&'a StackFrame<'a>> {
        self.stack
    }

    pub fn lookup_local(&self, name: &str) -> Option<&TrackedValue> {
        self.locals.get(name)
    }

    pub fn locals(&self) -> impl Iterator<Item = (&str, &TrackedValue)> {
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
        self.module_id
            .cloned()
            .ok_or_else(|| self.throw(EvalErrorKind::ModuleIdMissing, None))
    }

    pub fn throw(
        &self,
        kind: impl Into<EvalErrorKind>,
        final_frame: Option<(crate::ModuleId, crate::Span, String)>,
    ) -> EvalError {
        let mut frames = Vec::new();
        if let Some((module_id, span, name)) = final_frame {
            frames.push((module_id, span, name));
        }
        if let Some(stack) = self.stack {
            frames.extend(stack.collect_trace());
        }
        EvalError {
            kind: kind.into(),
            stack_trace: StackTrace { frames },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnEnv {
    pub module_id: crate::ModuleId,
    pub captures: HashMap<String, TrackedValue>,
    pub parameters: Vec<String>,
}

impl FnEnv {
    pub fn as_eval_env<'a>(
        &'a self,
        args: &[TrackedValue],
        stack: Option<&'a StackFrame<'a>>,
    ) -> EvalEnv<'a> {
        let mut env = EvalEnv::new().with_module_id(&self.module_id);
        env.stack = stack;

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
        dependencies: Vec<crate::ResourceId>,
    },
    UpdateResource {
        id: crate::ResourceId,
        inputs: crate::Record,
        dependencies: Vec<crate::ResourceId>,
    },
    TouchResource {
        id: crate::ResourceId,
        inputs: crate::Record,
        dependencies: Vec<crate::ResourceId>,
    },
}

#[derive(Clone, Debug)]
pub struct EvalCtx {
    effects: mpsc::UnboundedSender<Effect>,
    resources: HashMap<crate::ResourceId, crate::Resource>,
    namespace: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ListItemOutcome {
    Complete,
    Pending(BTreeSet<crate::ResourceId>),
}

impl EvalCtx {
    pub fn emit(&self, effect: Effect) -> Result<(), EvalError> {
        self.effects
            .send(effect)
            .map_err(|send_error| EvalErrorKind::EmitEffect(send_error.0).into())
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

    pub fn namespace(&self) -> &str {
        self.namespace.as_str()
    }

    pub fn resource(
        &self,
        ty: impl Into<String>,
        id: impl Into<String>,
        inputs: &crate::Record,
        dependencies: BTreeSet<crate::ResourceId>,
    ) -> Result<Option<crate::Record>, EvalError> {
        let ty = ty.into();
        let id = id.into();
        let resource_id = crate::ResourceId {
            ty: ty.clone(),
            id: id.clone(),
        };
        let dependencies = dependencies.into_iter().collect::<Vec<_>>();

        let Some(resource) = self.get_resource(ty, id) else {
            self.emit(Effect::CreateResource {
                id: resource_id,
                inputs: inputs.clone(),
                dependencies,
            })?;
            return Ok(None);
        };

        if resource.inputs != *inputs || resource.dependencies != dependencies {
            self.emit(Effect::UpdateResource {
                id: resource_id,
                inputs: inputs.clone(),
                dependencies,
            })?;
            return Ok(None);
        }

        self.emit(Effect::TouchResource {
            id: resource_id,
            inputs: inputs.clone(),
            dependencies,
        })?;

        Ok(Some(resource.outputs.clone()))
    }
}

#[derive(Debug)]
pub struct StackTrace {
    pub frames: Vec<(crate::ModuleId, crate::Span, String)>,
}

impl std::fmt::Display for StackTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (module_id, span, name) in &self.frames {
            write!(f, "\n  at {name} ({module_id} {span})")?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct EvalError {
    pub kind: EvalErrorKind,
    pub stack_trace: StackTrace,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.kind, self.stack_trace)
    }
}

impl std::error::Error for EvalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.kind.source()
    }
}

impl From<EvalErrorKind> for EvalError {
    fn from(kind: EvalErrorKind) -> Self {
        Self {
            kind,
            stack_trace: StackTrace { frames: Vec::new() },
        }
    }
}

#[derive(Error, Debug)]
pub enum EvalErrorKind {
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

    #[error("{0}")]
    Custom(String),

    #[error("{0}")]
    Exception(RaisedException),
}

#[derive(Debug)]
pub struct RaisedException {
    pub exception_id: u64,
    pub payload: Value,
    pub name: String,
}

impl std::fmt::Display for RaisedException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.payload {
            Value::Nil => write!(f, "{}", self.name),
            v => write!(f, "{}: {v}", self.name),
        }
    }
}

pub trait ValueAssertions {
    fn assert_int(self) -> Result<i64, EvalError>;
    fn assert_str(self) -> Result<String, EvalError>;
    fn assert_record(self) -> Result<Record, EvalError>;
    fn assert_int_ref(&self) -> Result<&i64, EvalError>;
    fn assert_str_ref(&self) -> Result<&str, EvalError>;
    fn assert_record_ref(&self) -> Result<&Record, EvalError>;
}

impl ValueAssertions for Value {
    fn assert_int(self) -> Result<i64, EvalError> {
        match self {
            Value::Int(value) => Ok(value),
            other => Err(EvalErrorKind::UnexpectedValue(other).into()),
        }
    }

    fn assert_str(self) -> Result<String, EvalError> {
        match self {
            Value::Str(value) => Ok(value),
            other => Err(EvalErrorKind::UnexpectedValue(other).into()),
        }
    }

    fn assert_record(self) -> Result<Record, EvalError> {
        match self {
            Value::Record(value) => Ok(value),
            other => Err(EvalErrorKind::UnexpectedValue(other).into()),
        }
    }

    fn assert_int_ref(&self) -> Result<&i64, EvalError> {
        match self {
            Value::Int(value) => Ok(value),
            other => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
        }
    }

    fn assert_str_ref(&self) -> Result<&str, EvalError> {
        match self {
            Value::Str(value) => Ok(value.as_str()),
            other => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
        }
    }

    fn assert_record_ref(&self) -> Result<&Record, EvalError> {
        match self {
            Value::Record(value) => Ok(value),
            other => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
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

    fn assert_record(self) -> Result<Record, EvalError> {
        self.unwrap_or(Value::Nil).assert_record()
    }

    fn assert_int_ref(&self) -> Result<&i64, EvalError> {
        match self {
            Some(Value::Int(value)) => Ok(value),
            Some(other) => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
            None => Err(EvalErrorKind::UnexpectedValue(Value::Nil).into()),
        }
    }

    fn assert_str_ref(&self) -> Result<&str, EvalError> {
        match self {
            Some(Value::Str(value)) => Ok(value.as_str()),
            Some(other) => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
            None => Err(EvalErrorKind::UnexpectedValue(Value::Nil).into()),
        }
    }

    fn assert_record_ref(&self) -> Result<&Record, EvalError> {
        match self {
            Some(Value::Record(value)) => Ok(value),
            Some(other) => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
            None => Err(EvalErrorKind::UnexpectedValue(Value::Nil).into()),
        }
    }
}

impl Eval {
    pub fn new<S: crate::SourceRepo>(
        effects: mpsc::UnboundedSender<Effect>,
        namespace: impl Into<String>,
    ) -> Self {
        let mut eval = Self {
            ctx: EvalCtx {
                effects,
                resources: HashMap::new(),
                namespace: namespace.into(),
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
        f: impl Fn(Vec<TrackedValue>, &EvalCtx) -> Result<TrackedValue, EvalError>
        + Clone
        + Send
        + Sync
        + 'static,
    ) {
        self.add_extern(name, Value::ExternFn(ExternFnValue::new(Box::new(f))));
    }

    fn tracked(value: Value) -> TrackedValue {
        TrackedValue::new(value)
    }

    fn pending_with(dependencies: BTreeSet<crate::ResourceId>) -> TrackedValue {
        TrackedValue::pending().with_dependencies(dependencies)
    }

    fn with_dependencies(value: Value, dependencies: BTreeSet<crate::ResourceId>) -> TrackedValue {
        TrackedValue::new(value).with_dependencies(dependencies)
    }

    pub fn eval_expr(
        &self,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        match expr.as_ref() {
            ast::Expr::Int(int) => Ok(Self::tracked(Value::Int(int.value))),
            ast::Expr::Float(float) => Ok(Self::tracked(Value::Float(float.value))),
            ast::Expr::Bool(bool) => Ok(Self::tracked(Value::Bool(bool.value))),
            ast::Expr::Nil => Ok(Self::tracked(Value::Nil)),
            ast::Expr::Str(str) => Ok(Self::tracked(Value::Str(str.value.clone()))),
            ast::Expr::Extern(extern_expr) => self
                .externs
                .get(extern_expr.name.as_str())
                .cloned()
                .map(Self::tracked)
                .ok_or_else(|| {
                    env.throw(
                        EvalErrorKind::MissingExtern(extern_expr.name.clone()),
                        Some((
                            env.module_id.cloned().unwrap_or_default(),
                            expr.span(),
                            "extern".to_string(),
                        )),
                    )
                }),
            ast::Expr::If(if_expr) => {
                let condition = self.eval_expr(env, if_expr.condition.as_ref())?;
                if matches!(&condition.value, Value::Pending(_)) {
                    return Ok(condition.map(|_| Value::Pending(crate::PendingValue)));
                }

                match condition.value {
                    Value::Bool(true) => self.eval_expr(env, if_expr.then_expr.as_ref()),
                    Value::Bool(false) => {
                        if let Some(else_expr) = &if_expr.else_expr {
                            self.eval_expr(env, else_expr.as_ref())
                        } else {
                            Ok(Self::tracked(Value::Nil))
                        }
                    }
                    other => Err(env.throw(
                        EvalErrorKind::UnexpectedValue(other),
                        Some((
                            env.module_id.cloned().unwrap_or_default(),
                            expr.span(),
                            "if".to_string(),
                        )),
                    )),
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
                Ok(Self::tracked(Value::Fn(FnValue {
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
                })))
            }
            ast::Expr::Call(call_expr) => {
                let args = call_expr
                    .args
                    .iter()
                    .map(|arg| self.eval_expr(env, arg))
                    .collect::<Result<Vec<_>, _>>()?;
                let callee = self.eval_expr(env, call_expr.callee.as_ref())?;
                let callee_dependencies = callee.dependencies.clone();
                if matches!(&callee.value, Value::Pending(_)) {
                    return Ok(TrackedValue::pending().with_dependencies(callee_dependencies));
                }

                let frame_name = match &**call_expr.callee.as_ref() {
                    ast::Expr::Var(var) => var.name.clone(),
                    _ => "[fn]".to_string(),
                };

                match callee.value {
                    Value::Fn(function) => {
                        let call_module_id = env.module_id.cloned().unwrap_or_default();
                        let frame = StackFrame {
                            module_id: call_module_id,
                            span: expr.span(),
                            name: frame_name,
                            parent: env.stack,
                        };
                        let call_env = function.env.as_eval_env(&args, Some(&frame));
                        self.eval_expr(&call_env, &function.body)
                            .map(|value| value.with_dependencies(callee_dependencies))
                    }
                    Value::ExternFn(function) => {
                        if args
                            .iter()
                            .any(|arg| matches!(arg.value, Value::Pending(_)))
                        {
                            return Ok(
                                TrackedValue::pending().with_dependencies(callee_dependencies)
                            );
                        }
                        function
                            .call(args, &self.ctx)
                            .map(|value| value.with_dependencies(callee_dependencies))
                    }
                    _ => Ok(Self::tracked(Value::Nil).with_dependencies(callee_dependencies)),
                }
            }
            ast::Expr::Unary(unary_expr) => {
                let value = self.eval_expr(env, unary_expr.expr.as_ref())?;
                if matches!(value.value, Value::Pending(_)) {
                    return Ok(Self::pending_with(value.dependencies));
                }
                let unary_span = expr.span();
                let unary_module_id = env.module_id.cloned().unwrap_or_default();
                match unary_expr.op {
                    ast::UnaryOp::Negate => value.try_map(|value| match value {
                        Value::Int(value) => Ok(Value::Int(-value)),
                        Value::Float(value_float) => Ok(Value::Float(
                            ordered_float::NotNan::new(-value_float.into_inner()).map_err(
                                |_| {
                                    env.throw(
                                        EvalErrorKind::InvalidNumericResult(
                                            "unary - produced NaN".into(),
                                        ),
                                        Some((
                                            unary_module_id.clone(),
                                            unary_span,
                                            "-".to_string(),
                                        )),
                                    )
                                },
                            )?,
                        )),
                        other => Err(env.throw(
                            EvalErrorKind::UnexpectedValue(other),
                            Some((unary_module_id.clone(), unary_span, "-".to_string())),
                        )),
                    }),
                }
            }
            ast::Expr::Binary(binary_expr) => {
                let lhs = self.eval_expr(env, binary_expr.lhs.as_ref())?;
                if matches!(lhs.value, Value::Pending(_)) {
                    return Ok(Self::pending_with(lhs.dependencies));
                }

                let binary_span = expr.span();
                let binary_module_id = env.module_id.cloned().unwrap_or_default();
                let op_name = format!("{:?}", binary_expr.op).to_lowercase();

                match binary_expr.op {
                    ast::BinaryOp::And => lhs.try_flat_map(|lhs| match lhs {
                        Value::Bool(false) => Ok(TrackedValue::new(Value::Bool(false))),
                        Value::Bool(true) => {
                            let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                            if matches!(&rhs.value, Value::Pending(_)) {
                                return Ok(rhs.map(|_| Value::Pending(crate::PendingValue)));
                            }
                            rhs.try_map(|rhs| match rhs {
                                Value::Bool(value) => Ok(Value::Bool(value)),
                                other => Err(env.throw(
                                    EvalErrorKind::UnexpectedValue(other),
                                    Some((binary_module_id.clone(), binary_span, op_name.clone())),
                                )),
                            })
                        }
                        other => Err(env.throw(
                            EvalErrorKind::UnexpectedValue(other),
                            Some((binary_module_id.clone(), binary_span, op_name.clone())),
                        )),
                    }),
                    ast::BinaryOp::Or => lhs.try_flat_map(|lhs| match lhs {
                        Value::Bool(true) => Ok(TrackedValue::new(Value::Bool(true))),
                        Value::Bool(false) => {
                            let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                            if matches!(&rhs.value, Value::Pending(_)) {
                                return Ok(rhs.map(|_| Value::Pending(crate::PendingValue)));
                            }
                            rhs.try_map(|rhs| match rhs {
                                Value::Bool(value) => Ok(Value::Bool(value)),
                                other => Err(env.throw(
                                    EvalErrorKind::UnexpectedValue(other),
                                    Some((binary_module_id.clone(), binary_span, op_name.clone())),
                                )),
                            })
                        }
                        other => Err(env.throw(
                            EvalErrorKind::UnexpectedValue(other),
                            Some((binary_module_id.clone(), binary_span, op_name.clone())),
                        )),
                    }),
                    _ => {
                        let rhs = self.eval_expr(env, binary_expr.rhs.as_ref())?;
                        if matches!(rhs.value, Value::Pending(_)) {
                            return Ok(Self::pending_with(rhs.dependencies));
                        }

                        lhs.try_flat_map(|lhs| {
                            rhs.try_map(|rhs| {
                                self.eval_binary_values(binary_expr.op, lhs, rhs)
                                    .map_err(|kind| {
                                        env.throw(
                                            kind,
                                            Some((
                                                binary_module_id.clone(),
                                                binary_span,
                                                op_name.clone(),
                                            )),
                                        )
                                    })
                            })
                        })
                    }
                }
            }
            ast::Expr::Var(var) => self.eval_var_name(env, var.name.as_str()),
            ast::Expr::Record(record_expr) => {
                let mut record = Record::default();
                let mut dependencies = BTreeSet::new();
                for field in &record_expr.fields {
                    let value = self.eval_expr(env, &field.expr)?;
                    dependencies.extend(value.dependencies.clone());
                    record.insert(field.var.name.clone(), value.value);
                }
                Ok(Self::with_dependencies(Value::Record(record), dependencies))
            }
            ast::Expr::Dict(dict_expr) => {
                let mut dict = Dict::default();
                let mut dependencies = BTreeSet::new();
                for entry in &dict_expr.entries {
                    let key = self.eval_expr(env, &entry.key)?;
                    let value = self.eval_expr(env, &entry.value)?;
                    dependencies.extend(key.dependencies.clone());
                    dependencies.extend(value.dependencies.clone());
                    if matches!(key.value, Value::Pending(_))
                        || matches!(value.value, Value::Pending(_))
                    {
                        return Ok(Self::pending_with(dependencies));
                    }
                    dict.insert(key.value, value.value);
                }
                Ok(Self::with_dependencies(Value::Dict(dict), dependencies))
            }
            ast::Expr::List(list_expr) => {
                let mut values = Vec::new();
                let mut dependencies = BTreeSet::new();
                for item in &list_expr.items {
                    match self.eval_list_item(env, item, &mut values)? {
                        ListItemOutcome::Complete => {}
                        ListItemOutcome::Pending(pending_dependencies) => {
                            dependencies.extend(pending_dependencies);
                            return Ok(Self::pending_with(dependencies));
                        }
                    }
                }

                for value in &values {
                    dependencies.extend(value.dependencies.clone());
                }

                Ok(Self::with_dependencies(
                    Value::List(values.into_iter().map(|value| value.value).collect()),
                    dependencies,
                ))
            }
            ast::Expr::Interp(interp_expr) => {
                let mut out = String::new();
                let mut dependencies = BTreeSet::new();
                for part in &interp_expr.parts {
                    let value = self.eval_expr(env, part)?;
                    dependencies.extend(value.dependencies.clone());
                    if matches!(value.value, Value::Pending(_)) {
                        return Ok(Self::pending_with(dependencies));
                    }
                    out.push_str(&value.value.to_string());
                }
                Ok(Self::with_dependencies(Value::Str(out), dependencies))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let value = self.eval_expr(env, property_access.expr.as_ref())?;
                match value.value {
                    Value::Pending(_) => Ok(Self::pending_with(value.dependencies)),
                    Value::Record(record) => Ok(Self::with_dependencies(
                        record.get(property_access.property.name.as_str()).clone(),
                        value.dependencies,
                    )),
                    _ => Ok(Self::tracked(Value::Nil)),
                }
            }
            ast::Expr::Exception(exception_expr) => {
                let exception_id = exception_expr.exception_id;
                if exception_expr.ty.is_some() {
                    let exc_fn = Value::ExternFn(ExternFnValue::new(Box::new(
                        move |args: Vec<TrackedValue>, _ctx: &EvalCtx| {
                            let payload = args
                                .into_iter()
                                .next()
                                .map(|a| a.value)
                                .unwrap_or(Value::Nil);
                            Ok(TrackedValue::new(Value::Exception(ExceptionValue {
                                exception_id,
                                payload: Box::new(payload),
                            })))
                        },
                    )));
                    Ok(Self::tracked(exc_fn))
                } else {
                    Ok(Self::tracked(Value::Exception(ExceptionValue {
                        exception_id,
                        payload: Box::new(Value::Nil),
                    })))
                }
            }
            ast::Expr::Raise(raise_expr) => {
                let value = self.eval_expr(env, raise_expr.expr.as_ref())?;
                if matches!(value.value, Value::Pending(_)) {
                    return Ok(Self::pending_with(value.dependencies));
                }
                let raise_frame = Some((
                    env.module_id.cloned().unwrap_or_default(),
                    expr.span(),
                    "raise".to_string(),
                ));
                let exception_name = match raise_expr.expr.as_ref().as_ref() {
                    ast::Expr::Var(var) => var.name.clone(),
                    ast::Expr::Call(call) => match call.callee.as_ref().as_ref() {
                        ast::Expr::Var(var) => var.name.clone(),
                        _ => "exception".to_string(),
                    },
                    _ => "exception".to_string(),
                };
                match value.value {
                    Value::Exception(exc) => Err(env.throw(
                        EvalErrorKind::Exception(RaisedException {
                            exception_id: exc.exception_id,
                            payload: *exc.payload,
                            name: exception_name,
                        }),
                        raise_frame,
                    )),
                    other => Err(env.throw(EvalErrorKind::UnexpectedValue(other), raise_frame)),
                }
            }
            ast::Expr::Try(try_expr) => {
                let try_module_id = env.module_id.cloned().unwrap_or_default();
                match self.eval_expr(env, try_expr.expr.as_ref()) {
                    Ok(value) => Ok(value),
                    Err(EvalError {
                        kind: EvalErrorKind::Exception(raised),
                        stack_trace,
                    }) => {
                        for catch in &try_expr.catches {
                            let catch_span = catch.exception_var.span();
                            let catch_target = self.eval_expr(
                                env,
                                &crate::Loc::new(
                                    ast::Expr::Var(catch.exception_var.clone()),
                                    catch_span,
                                ),
                            )?;

                            match catch_target.value {
                                Value::Exception(exc) => {
                                    if exc.exception_id == raised.exception_id {
                                        return self.eval_expr(env, &catch.body);
                                    }
                                }
                                Value::ExternFn(func) => {
                                    let arg_value = TrackedValue::new(raised.payload.clone());
                                    let call_result = func.call(vec![arg_value], &self.ctx)?;
                                    match call_result.value {
                                        Value::Exception(exc) => {
                                            if exc.exception_id == raised.exception_id {
                                                if let Some(catch_arg) = &catch.catch_arg {
                                                    let inner_env = env.with_local(
                                                        catch_arg.name.as_str(),
                                                        TrackedValue::new(raised.payload.clone()),
                                                    );
                                                    return self.eval_expr(&inner_env, &catch.body);
                                                } else {
                                                    return self.eval_expr(env, &catch.body);
                                                }
                                            }
                                        }
                                        _ => {
                                            return Err(env.throw(
                                                EvalErrorKind::UnexpectedValue(call_result.value),
                                                Some((
                                                    try_module_id.clone(),
                                                    catch_span,
                                                    "catch".to_string(),
                                                )),
                                            ));
                                        }
                                    }
                                }
                                Value::Fn(function) => {
                                    let arg_value = TrackedValue::new(raised.payload.clone());
                                    let frame = StackFrame {
                                        module_id: try_module_id.clone(),
                                        span: catch_span,
                                        name: "[fn]".to_string(),
                                        parent: env.stack,
                                    };
                                    let call_env =
                                        function.env.as_eval_env(&[arg_value], Some(&frame));
                                    let call_result = self.eval_expr(&call_env, &function.body)?;
                                    match call_result.value {
                                        Value::Exception(exc) => {
                                            if exc.exception_id == raised.exception_id {
                                                if let Some(catch_arg) = &catch.catch_arg {
                                                    let inner_env = env.with_local(
                                                        catch_arg.name.as_str(),
                                                        TrackedValue::new(raised.payload.clone()),
                                                    );
                                                    return self.eval_expr(&inner_env, &catch.body);
                                                } else {
                                                    return self.eval_expr(env, &catch.body);
                                                }
                                            }
                                        }
                                        _ => {
                                            return Err(env.throw(
                                                EvalErrorKind::UnexpectedValue(call_result.value),
                                                Some((
                                                    try_module_id.clone(),
                                                    catch_span,
                                                    "catch".to_string(),
                                                )),
                                            ));
                                        }
                                    }
                                }
                                _ => {
                                    return Err(env.throw(
                                        EvalErrorKind::UnexpectedValue(catch_target.value),
                                        Some((
                                            try_module_id.clone(),
                                            catch_span,
                                            "catch".to_string(),
                                        )),
                                    ));
                                }
                            }
                        }
                        // No catch matched, re-raise
                        Err(EvalError {
                            kind: EvalErrorKind::Exception(raised),
                            stack_trace,
                        })
                    }
                    Err(other) => Err(other),
                }
            }
        }
    }

    fn eval_binary_values(
        &self,
        op: ast::BinaryOp,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, EvalErrorKind> {
        match op {
            ast::BinaryOp::Add => match (lhs, rhs) {
                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs + rhs)),
                (Value::Float(lhs), Value::Float(rhs)) => Ok(Value::Float(lhs + rhs)),
                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs as f64 + rhs.into_inner()).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("int + float produced NaN".into())
                    })?,
                )),
                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs.into_inner() + rhs as f64).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("float + int produced NaN".into())
                    })?,
                )),
                (Value::Str(mut lhs), Value::Str(rhs)) => {
                    lhs.push_str(&rhs);
                    Ok(Value::Str(lhs))
                }
                (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
            },
            ast::BinaryOp::Sub => match (lhs, rhs) {
                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs - rhs)),
                (Value::Float(lhs), Value::Float(rhs)) => Ok(Value::Float(lhs - rhs)),
                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs as f64 - rhs.into_inner()).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("int - float produced NaN".into())
                    })?,
                )),
                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs.into_inner() - rhs as f64).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("float - int produced NaN".into())
                    })?,
                )),
                (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
            },
            ast::BinaryOp::Mul => match (lhs, rhs) {
                (Value::Int(lhs), Value::Int(rhs)) => Ok(Value::Int(lhs * rhs)),
                (Value::Float(lhs), Value::Float(rhs)) => Ok(Value::Float(lhs * rhs)),
                (Value::Int(lhs), Value::Float(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs as f64 * rhs.into_inner()).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("int * float produced NaN".into())
                    })?,
                )),
                (Value::Float(lhs), Value::Int(rhs)) => Ok(Value::Float(
                    ordered_float::NotNan::new(lhs.into_inner() * rhs as f64).map_err(|_| {
                        EvalErrorKind::InvalidNumericResult("float * int produced NaN".into())
                    })?,
                )),
                (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
            },
            ast::BinaryOp::Div => match (lhs, rhs) {
                (Value::Int(lhs), Value::Int(rhs)) => {
                    if rhs == 0 {
                        return Err(EvalErrorKind::DivisionByZero);
                    }
                    Ok(Value::Int(lhs / rhs))
                }
                (Value::Float(lhs), Value::Float(rhs)) => {
                    if rhs.into_inner() == 0.0 {
                        return Err(EvalErrorKind::DivisionByZero);
                    }
                    Ok(Value::Float(
                        ordered_float::NotNan::new(lhs.into_inner() / rhs.into_inner()).map_err(
                            |_| {
                                EvalErrorKind::InvalidNumericResult(
                                    "float / float produced NaN".into(),
                                )
                            },
                        )?,
                    ))
                }
                (Value::Int(lhs), Value::Float(rhs)) => {
                    if rhs.into_inner() == 0.0 {
                        return Err(EvalErrorKind::DivisionByZero);
                    }
                    Ok(Value::Float(
                        ordered_float::NotNan::new(lhs as f64 / rhs.into_inner()).map_err(
                            |_| {
                                EvalErrorKind::InvalidNumericResult(
                                    "int / float produced NaN".into(),
                                )
                            },
                        )?,
                    ))
                }
                (Value::Float(lhs), Value::Int(rhs)) => {
                    if rhs == 0 {
                        return Err(EvalErrorKind::DivisionByZero);
                    }
                    Ok(Value::Float(
                        ordered_float::NotNan::new(lhs.into_inner() / rhs as f64).map_err(
                            |_| {
                                EvalErrorKind::InvalidNumericResult(
                                    "float / int produced NaN".into(),
                                )
                            },
                        )?,
                    ))
                }
                (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
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
                (lhs, rhs) => Err(EvalErrorKind::InvalidComparison { op, lhs, rhs }),
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
                (lhs, rhs) => Err(EvalErrorKind::InvalidComparison { op, lhs, rhs }),
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
                (lhs, rhs) => Err(EvalErrorKind::InvalidComparison { op, lhs, rhs }),
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
                (lhs, rhs) => Err(EvalErrorKind::InvalidComparison { op, lhs, rhs }),
            },
            ast::BinaryOp::And | ast::BinaryOp::Or => unreachable!("handled earlier"),
        }
    }

    fn eval_list_item(
        &self,
        env: &EvalEnv<'_>,
        item: &ast::ListItem,
        out: &mut Vec<TrackedValue>,
    ) -> Result<ListItemOutcome, EvalError> {
        match item {
            ast::ListItem::Expr(expr) => {
                out.push(self.eval_expr(env, expr)?);
                Ok(ListItemOutcome::Complete)
            }
            ast::ListItem::If(if_item) => {
                let condition = self.eval_expr(env, if_item.condition.as_ref())?;
                match condition.value {
                    Value::Bool(true) => {
                        let mut outcome =
                            self.eval_list_item(env, if_item.then_item.as_ref(), out)?;
                        if let ListItemOutcome::Pending(ref mut dependencies) = outcome {
                            dependencies.extend(condition.dependencies);
                        }
                        Ok(outcome)
                    }
                    Value::Bool(false) => Ok(ListItemOutcome::Complete),
                    Value::Pending(_) => Ok(ListItemOutcome::Pending(condition.dependencies)),
                    other => Err(env.throw(
                        EvalErrorKind::UnexpectedValue(other),
                        Some((
                            env.module_id.cloned().unwrap_or_default(),
                            if_item.condition.span(),
                            "if".to_string(),
                        )),
                    )),
                }
            }
            ast::ListItem::For(for_item) => {
                let iterable = self.eval_expr(env, for_item.iterable.as_ref())?;
                match iterable.value {
                    Value::List(values) => {
                        for value in values {
                            let local_value = TrackedValue::new(value)
                                .with_dependencies(iterable.dependencies.clone());
                            let inner_env = env.with_local(for_item.var.name.as_str(), local_value);
                            if let ListItemOutcome::Pending(mut dependencies) =
                                self.eval_list_item(&inner_env, for_item.emit_item.as_ref(), out)?
                            {
                                dependencies.extend(iterable.dependencies.clone());
                                return Ok(ListItemOutcome::Pending(dependencies));
                            }
                        }
                        Ok(ListItemOutcome::Complete)
                    }
                    Value::Pending(_) => Ok(ListItemOutcome::Pending(iterable.dependencies)),
                    other => Err(env.throw(
                        EvalErrorKind::UnexpectedValue(other),
                        Some((
                            env.module_id.cloned().unwrap_or_default(),
                            for_item.iterable.span(),
                            "for".to_string(),
                        )),
                    )),
                }
            }
        }
    }

    fn eval_var_name(&self, env: &EvalEnv<'_>, name: &str) -> Result<TrackedValue, EvalError> {
        if let Some(local_value) = env.lookup_local(name) {
            return Ok(local_value.clone());
        }
        if let Some(global_expr) = env.lookup_global(name) {
            let module_id = env.module_id.cloned().unwrap_or_default();
            let frame = StackFrame {
                module_id,
                span: global_expr.span(),
                name: name.to_string(),
                parent: env.stack,
            };
            let global_env = env.without_locals().with_stack_frame(&frame)?;
            return self.eval_expr(&global_env, global_expr);
        }
        if let Some((target_module_id, import_file_mod)) = env.lookup_import(name) {
            let import_env = EvalEnv::new().with_module_id(&target_module_id);
            return self.eval_file_mod(&import_env, import_file_mod);
        }
        Ok(Self::tracked(Value::Nil))
    }

    pub fn eval_stmt(
        &self,
        env: &EvalEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Option<(String, TrackedValue)>, EvalError> {
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
    ) -> Result<TrackedValue, EvalError> {
        let globals = file_mod.find_globals();
        let env = env.with_globals(&globals);
        let mut exports = Record::default();
        let mut dependencies = BTreeSet::new();

        for statement in &file_mod.statements {
            if let Some((name, value)) = self.eval_stmt(&env, statement)? {
                dependencies.extend(value.dependencies.clone());
                exports.insert(name, value.value);
            }
        }

        Ok(Self::with_dependencies(
            Value::Record(exports),
            dependencies,
        ))
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::{Effect, Eval, EvalEnv};
    use crate::{ExternFnValue, ModuleId, Resource, ResourceId, TrackedValue, Value};

    fn parse_expr(source: &str, module_id: &ModuleId) -> crate::Loc<crate::ast::Expr> {
        let diagnosed = crate::parse_repl_line(source, module_id);
        assert!(!diagnosed.diags().has_errors());
        let line = diagnosed.into_inner().expect("repl line should parse");
        let statement = line
            .statement
            .expect("repl line should contain a statement");
        match statement {
            crate::ast::ModStmt::Expr(expr) => expr,
            other => panic!("expected expression statement, got {other:?}"),
        }
    }

    #[test]
    fn eval_expr_propagates_dependencies() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let module_id = ModuleId::default();
        let dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "seed".to_string(),
        };
        let env = EvalEnv::new().with_module_id(&module_id).with_local(
            "x",
            TrackedValue::new(Value::Int(2)).with_dependency(dependency.clone()),
        );
        let expr = parse_expr("x + 1", &module_id);

        let evaluated = eval
            .eval_expr(&env, &expr)
            .expect("evaluation should succeed");
        assert_eq!(evaluated.value, Value::Int(3));
        assert!(evaluated.dependencies.contains(&dependency));
    }

    #[test]
    fn eval_extern_call_can_explicitly_include_argument_dependencies() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "arg".to_string(),
        };
        let env = EvalEnv::new()
            .with_module_id(&module_id)
            .with_local(
                "f",
                TrackedValue::new(Value::ExternFn(ExternFnValue::new(Box::new(
                    |args: Vec<TrackedValue>, _ctx: &super::EvalCtx| {
                        let first = args
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| TrackedValue::new(Value::Nil));
                        first.try_map(|value| match value {
                            Value::Int(value) => Ok(Value::Int(value + 1)),
                            other => Err(super::EvalErrorKind::UnexpectedValue(other).into()),
                        })
                    },
                ))))
                .with_dependency(callee_dependency.clone()),
            )
            .with_local(
                "x",
                TrackedValue::new(Value::Int(2)).with_dependency(arg_dependency.clone()),
            );
        let expr = parse_expr("f(x)", &module_id);

        let evaluated = eval
            .eval_expr(&env, &expr)
            .expect("evaluation should succeed");
        assert_eq!(evaluated.value, Value::Int(3));
        assert!(evaluated.dependencies.contains(&callee_dependency));
        assert!(evaluated.dependencies.contains(&arg_dependency));
    }

    #[test]
    fn eval_extern_call_does_not_implicitly_include_argument_dependencies() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "arg".to_string(),
        };
        let env = EvalEnv::new()
            .with_module_id(&module_id)
            .with_local(
                "f",
                TrackedValue::new(Value::ExternFn(ExternFnValue::new(Box::new(
                    |args: Vec<TrackedValue>, _ctx: &super::EvalCtx| {
                        let value = args
                            .into_iter()
                            .next()
                            .map(|value| value.value)
                            .unwrap_or(Value::Nil);
                        match value {
                            Value::Int(value) => Ok(TrackedValue::new(Value::Int(value + 1))),
                            other => Err(super::EvalErrorKind::UnexpectedValue(other).into()),
                        }
                    },
                ))))
                .with_dependency(callee_dependency.clone()),
            )
            .with_local(
                "x",
                TrackedValue::new(Value::Int(2)).with_dependency(arg_dependency.clone()),
            );
        let expr = parse_expr("f(x)", &module_id);

        let evaluated = eval
            .eval_expr(&env, &expr)
            .expect("evaluation should succeed");
        assert_eq!(evaluated.value, Value::Int(3));
        assert!(evaluated.dependencies.contains(&callee_dependency));
        assert!(!evaluated.dependencies.contains(&arg_dependency));
    }

    #[test]
    fn eval_fn_call_constant_body_does_not_inherit_unused_argument_dependencies() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "arg".to_string(),
        };
        let fn_value = Value::Fn(crate::FnValue {
            env: crate::FnEnv {
                module_id: module_id.clone(),
                captures: std::collections::HashMap::new(),
                parameters: vec!["x".to_string()],
            },
            body: parse_expr("123", &module_id),
        });
        let env = EvalEnv::new()
            .with_module_id(&module_id)
            .with_local(
                "f",
                TrackedValue::new(fn_value).with_dependency(callee_dependency.clone()),
            )
            .with_local(
                "x",
                TrackedValue::new(Value::Int(2)).with_dependency(arg_dependency.clone()),
            );
        let expr = parse_expr("f(x)", &module_id);

        let evaluated = eval
            .eval_expr(&env, &expr)
            .expect("evaluation should succeed");
        assert_eq!(evaluated.value, Value::Int(123));
        assert!(evaluated.dependencies.contains(&callee_dependency));
        assert!(!evaluated.dependencies.contains(&arg_dependency));
    }

    #[test]
    fn resource_effect_updates_when_dependencies_change() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let id = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "x".to_string(),
        };
        let mut inputs = crate::Record::default();
        inputs.insert("min".to_string(), Value::Int(1));
        inputs.insert("max".to_string(), Value::Int(2));
        eval.add_resource(
            id.clone(),
            Resource {
                inputs: inputs.clone(),
                outputs: crate::Record::default(),
                dependencies: vec![],
            },
        );
        let dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "seed".to_string(),
        };
        let mut dependencies = std::collections::BTreeSet::new();
        dependencies.insert(dependency.clone());

        let outputs = eval
            .ctx
            .resource(id.ty.clone(), id.id.clone(), &inputs, dependencies)
            .expect("resource lookup should succeed");
        assert!(outputs.is_none());

        match rx.try_recv() {
            Ok(Effect::UpdateResource {
                id: effect_id,
                dependencies,
                ..
            }) => {
                assert_eq!(effect_id, id);
                assert_eq!(dependencies, vec![dependency]);
            }
            Ok(other) => panic!("expected update effect, got {other:?}"),
            Err(error) => panic!("expected queued effect, got {error}"),
        }
    }

    #[test]
    fn resource_effect_touches_when_unchanged() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut eval = Eval::new::<crate::std::StdSourceRepo>(tx, String::from("test/namespace"));
        let id = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "x".to_string(),
        };
        let mut inputs = crate::Record::default();
        inputs.insert("min".to_string(), Value::Int(1));
        inputs.insert("max".to_string(), Value::Int(2));
        let dependency = ResourceId {
            ty: "Std/Random.Int".to_string(),
            id: "seed".to_string(),
        };
        eval.add_resource(
            id.clone(),
            Resource {
                inputs: inputs.clone(),
                outputs: crate::Record::default(),
                dependencies: vec![dependency.clone()],
            },
        );
        let mut dependencies = std::collections::BTreeSet::new();
        dependencies.insert(dependency.clone());

        let outputs = eval
            .ctx
            .resource(id.ty.clone(), id.id.clone(), &inputs, dependencies)
            .expect("resource lookup should succeed");
        assert_eq!(outputs, Some(crate::Record::default()));

        match rx.try_recv() {
            Ok(Effect::TouchResource {
                id: effect_id,
                dependencies,
                ..
            }) => {
                assert_eq!(effect_id, id);
                assert_eq!(dependencies, vec![dependency]);
            }
            Ok(other) => panic!("expected touch effect, got {other:?}"),
            Err(error) => panic!("expected queued effect, got {error}"),
        }
    }
}
