use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::sync::mpsc;

use ids::ResourceId;

use crate::{ExternFnValue, GlobalKey, PathValue, RawModuleId, Record, TrackedValue, Value, ast};

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

    pub(crate) fn collect_trace(&self) -> Vec<(crate::ModuleId, crate::Span, String)> {
        let mut trace = vec![(self.module_id.clone(), self.span, self.name.clone())];
        let mut frame = self.parent;
        while let Some(f) = frame {
            trace.push((f.module_id.clone(), f.span, f.name.clone()));
            frame = f.parent;
        }
        trace
    }
}

type GlobalsMap<'a> = HashMap<&'a str, (crate::Span, &'a crate::Loc<ast::Expr>, Option<&'a str>)>;

// ═══════════════════════════════════════════════════════════════════════════════
// GlobalEvalEnv — accumulated evaluation results across SCC iterations
// ═══════════════════════════════════════════════════════════════════════════════

/// Accumulated global evaluation environment, built up as SCCs are processed
/// in topological order. `EvalEnv` borrows this to resolve globals and imports
/// without copying data into each per-SCC environment.
#[derive(Clone, Debug, Default)]
pub struct GlobalEvalEnv {
    values: HashMap<GlobalKey, TrackedValue>,
    /// Per-module import alias → target RawModuleId.
    import_maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>,
    /// Raw IDs of modules whose body is `.scle`. See
    /// [`crate::checker::GlobalTypeEnv::scle_modules`] for the rationale.
    scle_modules: std::collections::HashSet<RawModuleId>,
}

impl GlobalEvalEnv {
    pub fn new(import_maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>) -> Self {
        Self {
            values: HashMap::new(),
            import_maps,
            scle_modules: std::collections::HashSet::new(),
        }
    }

    pub fn insert(&mut self, key: GlobalKey, value: TrackedValue) {
        self.values.insert(key, value);
    }

    pub fn get(&self, key: &GlobalKey) -> Option<&TrackedValue> {
        self.values.get(key)
    }

    pub fn import_maps(&self) -> &HashMap<RawModuleId, HashMap<String, RawModuleId>> {
        &self.import_maps
    }

    /// Mark a module id as SCLE.
    pub fn mark_scle_module(&mut self, raw_id: RawModuleId) {
        self.scle_modules.insert(raw_id);
    }

    /// Whether the module at `raw_id` is an SCLE module.
    pub fn is_scle_module(&self, raw_id: &[String]) -> bool {
        self.scle_modules.contains(raw_id)
    }

    /// Merge additional import maps into this environment.
    pub fn merge_import_maps(&mut self, maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>) {
        for (raw_id, aliases) in maps {
            self.import_maps.entry(raw_id).or_default().extend(aliases);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&GlobalKey, &TrackedValue)> {
        self.values.iter()
    }

    /// Resolve an import alias to its target RawModuleId.
    pub fn resolve_import_alias(
        &self,
        alias: &str,
        raw_module_id: &[String],
    ) -> Option<&RawModuleId> {
        self.import_maps
            .get(raw_module_id)
            .and_then(|imports| imports.get(alias))
    }

    /// Resolve a value-level variable name in the context of a module.
    /// Checks same-module globals first, then import aliases.
    pub fn resolve_variable(&self, name: &str, raw_module_id: &[String]) -> Option<&TrackedValue> {
        // Same-module global?
        let global_key = GlobalKey::Global(raw_module_id.to_vec(), name.to_string());
        if let Some(val) = self.values.get(&global_key) {
            return Some(val);
        }
        // Import alias?
        if let Some(imports) = self.import_maps.get(raw_module_id)
            && let Some(target_raw_id) = imports.get(name)
        {
            let module_key = GlobalKey::ModuleValue(target_raw_id.clone());
            return self.values.get(&module_key);
        }
        None
    }
}

pub struct EvalEnv<'a> {
    pub(crate) module_id: Option<&'a crate::ModuleId>,
    pub(crate) global_env: &'a GlobalEvalEnv,
    raw_module_id: Option<&'a RawModuleId>,
    globals: Option<&'a GlobalsMap<'a>>,
    locals: HashMap<&'a str, TrackedValue>,
    /// Pre-evaluated globals (e.g., mutually recursive function groups).
    /// Checked before the lazy globals path in eval_var_name.
    precomputed: HashMap<String, TrackedValue>,
    pub(crate) stack: Option<&'a StackFrame<'a>>,
}

impl<'a> EvalEnv<'a> {
    pub fn new(global_env: &'a GlobalEvalEnv) -> Self {
        Self {
            module_id: None,
            global_env,
            raw_module_id: None,
            globals: None,
            locals: HashMap::new(),
            precomputed: HashMap::new(),
            stack: None,
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            global_env: self.global_env,
            raw_module_id: self.raw_module_id,
            globals: self.globals,
            locals: self.locals.clone(),
            precomputed: self.precomputed.clone(),
            stack: self.stack,
        }
    }

    pub fn with_globals(&self, globals: &'a GlobalsMap<'a>) -> Self {
        Self {
            module_id: self.module_id,
            global_env: self.global_env,
            raw_module_id: self.raw_module_id,
            globals: Some(globals),
            locals: HashMap::new(),
            precomputed: self.precomputed.clone(),
            stack: self.stack,
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            global_env: self.global_env,
            raw_module_id: self.raw_module_id,
            globals: self.globals,
            locals: self.locals.clone(),
            precomputed: self.precomputed.clone(),
            stack: self.stack,
        }
    }

    pub fn with_raw_module_id(&self, raw_module_id: &'a RawModuleId) -> Self {
        Self {
            module_id: self.module_id,
            global_env: self.global_env,
            raw_module_id: Some(raw_module_id),
            globals: self.globals,
            locals: self.locals.clone(),
            precomputed: self.precomputed.clone(),
            stack: self.stack,
        }
    }

    pub fn with_local(&self, name: &'a str, value: TrackedValue) -> Self {
        let mut env = self.inner();
        env.locals.insert(name, value);
        env
    }

    pub fn raw_module_id(&self) -> Option<&RawModuleId> {
        self.raw_module_id
    }

    pub fn with_precomputed(&self, name: String, value: TrackedValue) -> Self {
        let mut env = self.inner();
        env.precomputed.insert(name, value);
        env
    }

    pub fn without_locals(&self) -> Self {
        Self {
            module_id: self.module_id,
            global_env: self.global_env,
            raw_module_id: self.raw_module_id,
            globals: self.globals,
            locals: HashMap::new(),
            precomputed: self.precomputed.clone(),
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
        self.globals
            .and_then(|globals| globals.get(name))
            .map(|(_, expr, _)| *expr)
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
    /// Raw module ID for import alias resolution in the global eval env.
    pub raw_module_id: Option<RawModuleId>,
    pub captures: HashMap<String, TrackedValue>,
    pub parameters: Vec<String>,
    /// When set, the function is recursive and should be bound under this name
    /// in its own call environment so that recursive calls resolve correctly.
    pub self_name: Option<String>,
    /// For mutually recursive functions: all group members bound at call time.
    /// Shared via Arc so that all members in a group reference the same list.
    pub recursive_group: Option<std::sync::Arc<Vec<(String, crate::FnValue)>>>,
}

impl FnEnv {
    pub fn as_eval_env<'a>(
        &'a self,
        fn_value: &crate::FnValue,
        args: &[TrackedValue],
        stack: Option<&'a StackFrame<'a>>,
        global_env: &'a GlobalEvalEnv,
    ) -> EvalEnv<'a> {
        let mut env = EvalEnv::new(global_env).with_module_id(&self.module_id);
        if let Some(raw_id) = &self.raw_module_id {
            env = env.with_raw_module_id(raw_id);
        }
        env.stack = stack;

        for (name, value) in &self.captures {
            env = env.with_local(name.as_str(), value.clone());
        }
        // Bind the function itself as a local for recursive calls
        if let Some(self_name) = &self.self_name {
            env = env.with_local(
                self_name.as_str(),
                TrackedValue::new(crate::Value::Fn(fn_value.clone())),
            );
        }
        // Bind all members of a mutually recursive group.
        // Each member is bound with the same recursive_group so that
        // calls at any depth correctly resolve all siblings.
        if let Some(group) = &self.recursive_group {
            for (name, group_fn) in group.as_ref() {
                let mut wired_fn = group_fn.clone();
                wired_fn.env.recursive_group = Some(group.clone());
                env = env.with_local(name.as_str(), TrackedValue::new(crate::Value::Fn(wired_fn)));
            }
        }
        for (name, value) in self.parameters.iter().zip(args.iter()) {
            env = env.with_local(name.as_str(), value.clone());
        }

        env
    }
}

pub struct Eval<'p> {
    pub(crate) ctx: EvalCtx,
    pub(crate) externs: HashMap<String, Value>,
    /// Package map for on-demand path hash resolution at runtime.
    pub(crate) packages: Option<PackageMap>,
    /// Marker to keep the lifetime parameter used by callers.
    _phantom: std::marker::PhantomData<&'p ()>,
}

/// Resource effects emitted during evaluation.
///
/// Effects are sent through an unbounded MPSC channel to the caller. Because
/// evaluation is single-threaded and the channel is unbounded, sends never
/// block or fail unless the receiver is dropped.
///
/// **Atomicity:** effects are emitted one at a time as the evaluator
/// encounters resources. If evaluation fails partway through (e.g. due to a
/// runtime error or exception), the caller will have received a partial set
/// of effects. The caller is responsible for discarding or rolling back
/// partial effects on failure — the evaluator itself does not provide
/// transactional guarantees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    CreateResource {
        id: ResourceId,
        inputs: crate::Record,
        dependencies: Vec<ResourceId>,
        source_trace: ids::SourceTrace,
        /// The deployment that owns this effect — i.e. the deployment whose
        /// code path produced it. The DE drops effects whose owner is not
        /// the local deployment.
        owner: ids::DeploymentQid,
    },
    UpdateResource {
        id: ResourceId,
        inputs: crate::Record,
        dependencies: Vec<ResourceId>,
        source_trace: ids::SourceTrace,
        owner: ids::DeploymentQid,
    },
    TouchResource {
        id: ResourceId,
        inputs: crate::Record,
        dependencies: Vec<ResourceId>,
        source_trace: ids::SourceTrace,
        owner: ids::DeploymentQid,
    },
}

impl Effect {
    /// The owning deployment of this effect.
    pub fn owner(&self) -> &ids::DeploymentQid {
        match self {
            Effect::CreateResource { owner, .. }
            | Effect::UpdateResource { owner, .. }
            | Effect::TouchResource { owner, .. } => owner,
        }
    }
}

/// Shared map of `PackageId` → loaded `Package`, used by `Expr::Path`
/// evaluation and `Std/Path` extern functions to resolve content hashes
/// for paths within a package.
pub type PackageMap = Arc<HashMap<crate::PackageId, Arc<dyn crate::Package>>>;

pub struct EvalCtx {
    effects: mpsc::UnboundedSender<Effect>,
    resources: HashMap<ResourceId, crate::Resource>,
    namespace: String,
    /// The local deployment's QID — the bottom of the owner stack and the
    /// fallback when no scope has been pushed.
    local_owner: ids::DeploymentQid,
    /// Stack of effect-owner overrides. The current owner is the top of the
    /// stack, or `local_owner` if the stack is empty. Pushed only when
    /// entering a global expression defined by a foreign package; function
    /// calls do not push (closures are ownership-transparent).
    owner_stack: Mutex<Vec<ids::DeploymentQid>>,
    /// Map from package id to the deployment that owns globals defined in
    /// that package. Populated by the caller (typically the DE) after
    /// resolving cross-repo dependencies. Local globals are absent from this
    /// map and fall through to `local_owner`.
    package_owner: HashMap<crate::PackageId, ids::DeploymentQid>,
    /// Pre-loaded foreign resources, keyed by the foreign deployment's QID
    /// and the resource id within that deployment. The DE populates this
    /// from each foreign environment's RDB namespace before evaluation.
    /// `EvalCtx::resource()` consults this map (instead of `resources`)
    /// when the current owner is foreign.
    foreign_resources: HashMap<ids::DeploymentQid, HashMap<ResourceId, crate::Resource>>,
    /// Cross-deployment dependencies recorded as foreign reads have happened.
    /// Drained by callers via [`EvalCtx::take_foreign_dependencies`].
    foreign_deps: Mutex<HashSet<(ids::EnvironmentQid, ResourceId)>>,
    pub(crate) source_trace: Mutex<ids::SourceTrace>,
    /// Package map for on-demand path hash resolution at runtime. Shared
    /// `Arc` with [`Eval::packages`] so extern functions (e.g. `Std/Path`)
    /// can look up paths against the package the input `Path` carries,
    /// without going through the evaluator.
    packages: Mutex<Option<PackageMap>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListItemOutcome {
    Complete,
    Pending(BTreeSet<ResourceId>),
}

impl EvalCtx {
    pub fn new(
        effects: mpsc::UnboundedSender<Effect>,
        namespace: impl Into<String>,
        local_owner: ids::DeploymentQid,
    ) -> Self {
        Self {
            effects,
            resources: HashMap::new(),
            namespace: namespace.into(),
            local_owner,
            owner_stack: Mutex::new(Vec::new()),
            package_owner: HashMap::new(),
            foreign_resources: HashMap::new(),
            foreign_deps: Mutex::new(HashSet::new()),
            source_trace: Mutex::new(Vec::new()),
            packages: Mutex::new(None),
        }
    }

    /// Set the package map used by extern functions for path lookups. The
    /// evaluator calls this in [`Eval::with_packages`] so extern fns and
    /// `Expr::Path` evaluation share a single source of truth.
    pub(crate) fn set_packages(&self, packages: PackageMap) {
        *self.packages.lock().unwrap() = Some(packages);
    }

    /// Look up the git object hash for `resolved_path` in `package_id`.
    ///
    /// Returns `Ok(Some(hash))` on success, `Ok(None)` when no packages
    /// are registered (e.g. compile-time eval) or the package is unknown
    /// — extern callers fall back to a null hash in that case — and
    /// `Err(PathLookupError::NotFound)` when packages are registered but
    /// the path does not exist in the named package.
    pub fn resolve_path_hash(
        &self,
        resolved_path: &str,
        package_id: &crate::PackageId,
    ) -> Result<Option<gix_hash::ObjectId>, PathLookupError> {
        let guard = self.packages.lock().unwrap();
        let Some(packages) = guard.as_ref() else {
            return Ok(None);
        };
        let Some(package) = packages.get(package_id).cloned() else {
            return Ok(None);
        };
        drop(guard);

        let rel = resolved_path.strip_prefix('/').unwrap_or(resolved_path);
        #[cfg(not(feature = "runtime"))]
        {
            let _ = (rel, package);
            return Ok(None);
        }

        #[cfg(feature = "runtime")]
        {
            let path = std::path::Path::new(rel);
            let result = match tokio::runtime::Handle::try_current() {
                Ok(handle) => tokio::task::block_in_place(|| {
                    handle.block_on(async { package.lookup(path).await })
                }),
                Err(_) => return Ok(None),
            };
            match result {
                Ok(Some(entity)) => Ok(Some(entity.hash())),
                Ok(None) => Err(PathLookupError::NotFound),
                Err(_) => Err(PathLookupError::NotFound),
            }
        }
    }

    pub fn add_resource(&mut self, id: ResourceId, resource: crate::Resource) {
        self.resources.insert(id, resource);
    }

    /// Register a foreign resource (one that already exists in some other
    /// deployment's RDB namespace). Used when evaluating code in a foreign
    /// owner scope so resource lookups can return concrete outputs without
    /// emitting a Create/Update.
    pub fn add_foreign_resource(
        &mut self,
        owner: ids::DeploymentQid,
        id: ResourceId,
        resource: crate::Resource,
    ) {
        self.foreign_resources
            .entry(owner)
            .or_default()
            .insert(id, resource);
    }

    /// Drain the set of recorded cross-deployment dependencies. Each entry is
    /// `(foreign environment QID, resource id within that env)`.
    pub fn take_foreign_dependencies(&self) -> HashSet<(ids::EnvironmentQid, ResourceId)> {
        std::mem::take(&mut *self.foreign_deps.lock().unwrap())
    }

    /// Register a foreign package owner. Globals defined in `package` will be
    /// evaluated with `owner` on top of the owner stack and emit
    /// foreign-owned effects.
    pub fn set_package_owner(&mut self, package: crate::PackageId, owner: ids::DeploymentQid) {
        self.package_owner.insert(package, owner);
    }

    /// The local deployment QID. Used by callers (e.g. the DE) to compare
    /// against the owner stamped on emitted effects.
    pub fn local_owner(&self) -> &ids::DeploymentQid {
        &self.local_owner
    }

    /// The owner of effects emitted right now — top of the owner stack, or
    /// the local owner if the stack is empty.
    pub fn current_owner(&self) -> ids::DeploymentQid {
        self.owner_stack
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_else(|| self.local_owner.clone())
    }

    /// The owner of globals defined by `package`, falling back to the local
    /// deployment when the package isn't a registered foreign package.
    pub fn owner_for_package(&self, package: &crate::PackageId) -> ids::DeploymentQid {
        self.package_owner
            .get(package)
            .cloned()
            .unwrap_or_else(|| self.local_owner.clone())
    }

    /// Run `f` with `owner` pushed on the owner stack. The owner is popped
    /// when `f` returns (success or panic).
    pub fn with_owner<R>(&self, owner: ids::DeploymentQid, f: impl FnOnce() -> R) -> R {
        self.owner_stack.lock().unwrap().push(owner);
        let guard = OwnerStackGuard { ctx: self };
        let result = f();
        drop(guard);
        result
    }

    pub fn emit(&self, effect: Effect) -> Result<(), EvalError> {
        self.effects
            .send(effect)
            .map_err(|send_error| EvalErrorKind::EmitEffect(send_error.0).into())
    }

    pub fn get_resource(
        &self,
        ty: impl Into<String>,
        name: impl Into<String>,
    ) -> Option<&crate::Resource> {
        let resource_id = ResourceId {
            typ: ty.into(),
            name: name.into(),
        };
        self.resources.get(&resource_id)
    }

    pub fn namespace(&self) -> &str {
        self.namespace.as_str()
    }

    pub fn resource(
        &self,
        ty: impl Into<String>,
        name: impl Into<String>,
        inputs: &crate::Record,
        dependencies: BTreeSet<ResourceId>,
    ) -> Result<Option<crate::Record>, EvalError> {
        let ty = ty.into();
        let name = name.into();
        let resource_id = ResourceId {
            typ: ty.clone(),
            name: name.clone(),
        };
        let dependencies = dependencies.into_iter().collect::<Vec<_>>();
        let source_trace = self.source_trace.lock().unwrap().clone();
        let owner = self.current_owner();
        let is_foreign = owner != self.local_owner;

        // Foreign-owner path: read remote state from the foreign deployment's
        // RDB namespace if available. Always emit the foreign-owned effect
        // (the DE drops it) so the data dependency is preserved on the
        // event stream.
        if is_foreign {
            let foreign_resource = self
                .foreign_resources
                .get(&owner)
                .and_then(|map| map.get(&resource_id))
                .cloned();

            // Record the cross-deployment dependency for downstream tracking.
            self.foreign_deps
                .lock()
                .unwrap()
                .insert((owner.environment.clone(), resource_id.clone()));

            // Emit the appropriate foreign-owned effect (the DE drops it).
            let effect = match &foreign_resource {
                None => Effect::CreateResource {
                    id: resource_id,
                    inputs: inputs.clone(),
                    dependencies,
                    source_trace,
                    owner,
                },
                Some(existing)
                    if existing.inputs != *inputs || existing.dependencies != dependencies =>
                {
                    Effect::UpdateResource {
                        id: resource_id,
                        inputs: inputs.clone(),
                        dependencies,
                        source_trace,
                        owner,
                    }
                }
                Some(_) => Effect::TouchResource {
                    id: resource_id,
                    inputs: inputs.clone(),
                    dependencies,
                    source_trace,
                    owner,
                },
            };
            self.emit(effect)?;

            // Return concrete outputs only when the foreign resource is
            // already materialised AND its inputs match — otherwise the
            // local read should be `<pending>`.
            return Ok(foreign_resource.and_then(|r| {
                if r.inputs == *inputs {
                    Some(r.outputs.clone())
                } else {
                    None
                }
            }));
        }

        // Local-owner path (unchanged).
        let Some(resource) = self.get_resource(ty, name) else {
            self.emit(Effect::CreateResource {
                id: resource_id,
                inputs: inputs.clone(),
                dependencies,
                source_trace,
                owner,
            })?;
            return Ok(None);
        };

        if resource.inputs != *inputs || resource.dependencies != dependencies {
            self.emit(Effect::UpdateResource {
                id: resource_id,
                inputs: inputs.clone(),
                dependencies,
                source_trace,
                owner,
            })?;
            return Ok(None);
        }

        self.emit(Effect::TouchResource {
            id: resource_id,
            inputs: inputs.clone(),
            dependencies,
            source_trace,
            owner,
        })?;

        Ok(Some(resource.outputs.clone()))
    }
}

/// RAII guard returned indirectly via [`EvalCtx::with_owner`] — pops the
/// owner stack on drop so panics within the wrapped closure still leave the
/// stack balanced.
struct OwnerStackGuard<'a> {
    ctx: &'a EvalCtx,
}

impl Drop for OwnerStackGuard<'_> {
    fn drop(&mut self) {
        self.ctx.owner_stack.lock().unwrap().pop();
    }
}

/// Outcome of looking up a path in a package via [`EvalCtx::resolve_path_hash`].
#[derive(Debug)]
pub enum PathLookupError {
    /// Packages are registered but the path does not exist in the named
    /// package. Distinct from `Ok(None)`, which means no packages were
    /// registered at all.
    NotFound,
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

    #[error("integer overflow in `{op}`")]
    IntegerOverflow { op: String },

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
    fn assert_path(self) -> Result<PathValue, EvalError>;
    fn assert_int_ref(&self) -> Result<&i64, EvalError>;
    fn assert_str_ref(&self) -> Result<&str, EvalError>;
    fn assert_record_ref(&self) -> Result<&Record, EvalError>;
    fn assert_path_ref(&self) -> Result<&PathValue, EvalError>;
}

macro_rules! impl_value_assertions {
    ($(
        ($variant:ident,
         $owned_fn:ident -> $owned_ret:ty { $ov:ident => $owned_expr:expr },
         $ref_fn:ident -> $ref_ret:ty { $rv:ident => $ref_expr:expr })
    ),* $(,)?) => {
        impl ValueAssertions for Value {
            $(
                fn $owned_fn(self) -> Result<$owned_ret, EvalError> {
                    match self {
                        Value::$variant($ov) => Ok($owned_expr),
                        other => Err(EvalErrorKind::UnexpectedValue(other).into()),
                    }
                }
                fn $ref_fn(&self) -> Result<$ref_ret, EvalError> {
                    match self {
                        Value::$variant($rv) => Ok($ref_expr),
                        other => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
                    }
                }
            )*
        }
        impl ValueAssertions for Option<Value> {
            $(
                fn $owned_fn(self) -> Result<$owned_ret, EvalError> {
                    self.unwrap_or(Value::Nil).$owned_fn()
                }
                fn $ref_fn(&self) -> Result<$ref_ret, EvalError> {
                    match self {
                        Some(Value::$variant($rv)) => Ok($ref_expr),
                        Some(other) => Err(EvalErrorKind::UnexpectedValue(other.clone()).into()),
                        None => Err(EvalErrorKind::UnexpectedValue(Value::Nil).into()),
                    }
                }
            )*
        }
    };
}

impl_value_assertions! {
    (Int,
     assert_int -> i64 { v => v },
     assert_int_ref -> &i64 { v => v }),
    (Str,
     assert_str -> String { v => v },
     assert_str_ref -> &str { v => v.as_str() }),
    (Record,
     assert_record -> Record { v => v },
     assert_record_ref -> &Record { v => v }),
    (Path,
     assert_path -> PathValue { v => v },
     assert_path_ref -> &PathValue { v => v }),
}

pub(crate) fn tracked(value: Value) -> TrackedValue {
    TrackedValue::new(value)
}

pub(crate) fn pending_with(dependencies: BTreeSet<ResourceId>) -> TrackedValue {
    TrackedValue::pending().with_dependencies(dependencies)
}

pub(crate) fn with_dependencies(value: Value, dependencies: BTreeSet<ResourceId>) -> TrackedValue {
    TrackedValue::new(value).with_dependencies(dependencies)
}

impl<'p> Eval<'p> {
    /// Create an `Eval` from pre-collected externs and an evaluation context.
    ///
    /// Standard library externs are registered automatically; `externs` should
    /// contain any additional (package-provided) extern values.
    pub fn from_externs(externs: HashMap<String, Value>, ctx: EvalCtx) -> Self {
        let mut eval = Self {
            ctx,
            externs: HashMap::new(),
            packages: None,
            _phantom: std::marker::PhantomData,
        };
        crate::std::register_std_externs(&mut eval);
        eval.externs.extend(externs);
        eval
    }

    /// Set the package map for on-demand path hash resolution. The same
    /// `Arc` is shared with [`EvalCtx`] so extern functions can look up
    /// paths without re-plumbing through the evaluator.
    pub fn with_packages(mut self, packages: PackageMap) -> Self {
        self.ctx.set_packages(Arc::clone(&packages));
        self.packages = Some(packages);
        self
    }

    /// Resolve a path expression hash on-demand by calling `Package::lookup`.
    ///
    /// Returns a null hash when no packages are available or the path
    /// cannot be resolved — path literals are evaluated even when their
    /// referent does not exist (e.g. compile-time eval) and silently
    /// fall back to a null hash.
    pub(crate) fn resolve_path_hash(
        &self,
        resolved_path: &str,
        package_id: &crate::PackageId,
    ) -> gix_hash::ObjectId {
        match self.ctx.resolve_path_hash(resolved_path, package_id) {
            Ok(Some(hash)) => hash,
            _ => gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
        }
    }

    /// Register an extern value under the given name.
    ///
    /// # Panics
    ///
    /// Panics if `name` is empty or contains whitespace, which would make it
    /// unreachable from SCL source code.
    pub fn add_extern(&mut self, name: impl Into<String>, value: Value) {
        let name = name.into();
        assert!(
            !name.is_empty() && !name.contains(char::is_whitespace),
            "extern name must be non-empty and contain no whitespace, got: {name:?}"
        );
        self.externs.insert(name, value);
    }

    pub fn add_resource(&mut self, id: ResourceId, resource: crate::Resource) {
        self.ctx.add_resource(id, resource);
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
}

impl crate::std::ExternRegistry for Eval<'_> {
    fn add_extern_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<TrackedValue>, &EvalCtx) -> Result<TrackedValue, EvalError>
        + Clone
        + Send
        + Sync
        + 'static,
    ) {
        Eval::add_extern_fn(self, name, f);
    }
}

impl<'p> Eval<'p> {
    pub fn eval_expr(
        &self,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        stacker::maybe_grow(512 * 1024, 2 * 1024 * 1024, || {
            self.eval_expr_inner(env, expr)
        })
    }

    pub(crate) fn eval_expr_inner(
        &self,
        env: &EvalEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<TrackedValue, EvalError> {
        expr.as_ref().eval(self, env, expr)
    }

    pub(crate) fn eval_binary_values(
        &self,
        op: ast::BinaryOp,
        lhs: Value,
        rhs: Value,
    ) -> Result<Value, EvalErrorKind> {
        match op {
            ast::BinaryOp::Add => match (lhs, rhs) {
                (Value::Str(mut lhs), Value::Str(rhs)) => {
                    lhs.push_str(&rhs);
                    Ok(Value::Str(lhs))
                }
                (lhs, rhs) => {
                    eval_numeric_arithmetic(lhs, rhs, "+", i64::checked_add, |a, b| a + b)
                }
            },
            ast::BinaryOp::Sub => {
                eval_numeric_arithmetic(lhs, rhs, "-", i64::checked_sub, |a, b| a - b)
            }
            ast::BinaryOp::Mul => {
                eval_numeric_arithmetic(lhs, rhs, "*", i64::checked_mul, |a, b| a * b)
            }
            ast::BinaryOp::Div => eval_numeric_division(lhs, rhs),
            ast::BinaryOp::Eq => Ok(Value::Bool(lhs == rhs)),
            ast::BinaryOp::Neq => Ok(Value::Bool(lhs != rhs)),
            ast::BinaryOp::Lt => eval_numeric_comparison(op, lhs, rhs, |a, b| a < b, |a, b| a < b),
            ast::BinaryOp::Lte => {
                eval_numeric_comparison(op, lhs, rhs, |a, b| a <= b, |a, b| a <= b)
            }
            ast::BinaryOp::Gt => eval_numeric_comparison(op, lhs, rhs, |a, b| a > b, |a, b| a > b),
            ast::BinaryOp::Gte => {
                eval_numeric_comparison(op, lhs, rhs, |a, b| a >= b, |a, b| a >= b)
            }
            ast::BinaryOp::And | ast::BinaryOp::Or | ast::BinaryOp::NilCoalesce => {
                unreachable!("handled earlier")
            }
        }
    }

    pub(crate) fn eval_list_item(
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

    pub(crate) fn eval_var_name(
        &self,
        env: &EvalEnv<'_>,
        name: &str,
    ) -> Result<TrackedValue, EvalError> {
        if let Some(local_value) = env.lookup_local(name) {
            return Ok(local_value.clone());
        }
        // Check precomputed globals (e.g., mutually recursive function groups)
        if let Some(precomputed) = env.precomputed.get(name) {
            return Ok(precomputed.clone());
        }
        // Global eval env: resolve via accumulated global values.
        if let Some(raw_id) = env.raw_module_id.as_ref()
            && let Some(value) = env.global_env.resolve_variable(name, raw_id)
        {
            return Ok(value.clone());
        }
        // Legacy on-demand global evaluation (used by REPL/IDE).
        if let Some(global_expr) = env.lookup_global(name) {
            let module_id = env.module_id.cloned().unwrap_or_default();
            let frame = StackFrame {
                module_id,
                span: global_expr.span(),
                name: name.to_string(),
                parent: env.stack,
            };
            let global_env = env.without_locals().with_stack_frame(&frame)?;
            // For recursive globals whose body is a function expression,
            // build the closure without capturing the self-reference. Instead,
            // set `self_name` so that the function binds itself as a local at
            // each call site, enabling recursion to arbitrary depth.
            if let crate::ast::Expr::Fn(fn_expr) = global_expr.as_ref() {
                let free_vars = global_expr.as_ref().free_vars();
                if free_vars.contains(name) {
                    let fn_module_id = global_env.module_id()?;
                    let parameters: Vec<String> = fn_expr
                        .params
                        .iter()
                        .map(|param| param.var.name.clone())
                        .collect();
                    let body = fn_expr
                        .body
                        .as_ref()
                        .map(|b| *b.clone())
                        .unwrap_or_else(|| {
                            crate::Loc::new(crate::ast::Expr::Nil, crate::Span::default())
                        });

                    // Evaluate all captures except the self-reference
                    let mut captures = HashMap::new();
                    for free_var in &free_vars {
                        if *free_var != name {
                            captures.insert(
                                free_var.to_string(),
                                self.eval_var_name(&global_env, free_var)?,
                            );
                        }
                    }

                    let fn_val = crate::FnValue {
                        env: FnEnv {
                            module_id: fn_module_id,
                            raw_module_id: env.raw_module_id().cloned(),
                            captures,
                            parameters,
                            self_name: Some(name.to_string()),
                            recursive_group: None,
                        },
                        body,
                    };
                    return Ok(tracked(crate::Value::Fn(fn_val)));
                }
            }
            return self.eval_expr(&global_env, global_expr);
        }
        Ok(tracked(Value::Nil))
    }

    pub fn eval_stmt(
        &self,
        env: &EvalEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Option<(String, TrackedValue)>, EvalError> {
        match stmt {
            ast::ModStmt::Let(_)
            | ast::ModStmt::Import(_)
            | ast::ModStmt::TypeDef(_)
            | ast::ModStmt::ExportTypeDef(_) => Ok(None),
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
}

fn not_nan(value: f64, desc: &str) -> Result<ordered_float::NotNan<f64>, EvalErrorKind> {
    ordered_float::NotNan::new(value)
        .map_err(|_| EvalErrorKind::InvalidNumericResult(format!("{desc} produced NaN")))
}

fn eval_numeric_arithmetic(
    lhs: Value,
    rhs: Value,
    op_name: &str,
    int_op: fn(i64, i64) -> Option<i64>,
    float_op: fn(f64, f64) -> f64,
) -> Result<Value, EvalErrorKind> {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(int_op(a, b).ok_or_else(|| {
            EvalErrorKind::IntegerOverflow {
                op: op_name.to_string(),
            }
        })?)),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(not_nan(
            float_op(a.into_inner(), b.into_inner()),
            op_name,
        )?)),
        (Value::Int(a), Value::Float(b)) => Ok(Value::Float(not_nan(
            float_op(a as f64, b.into_inner()),
            op_name,
        )?)),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Float(not_nan(
            float_op(a.into_inner(), b as f64),
            op_name,
        )?)),
        (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
    }
}

fn eval_numeric_division(lhs: Value, rhs: Value) -> Result<Value, EvalErrorKind> {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => {
            if b == 0 {
                return Err(EvalErrorKind::DivisionByZero);
            }
            Ok(Value::Int(a / b))
        }
        (Value::Float(a), Value::Float(b)) => {
            if b.into_inner() == 0.0 {
                return Err(EvalErrorKind::DivisionByZero);
            }
            Ok(Value::Float(not_nan(a.into_inner() / b.into_inner(), "/")?))
        }
        (Value::Int(a), Value::Float(b)) => {
            if b.into_inner() == 0.0 {
                return Err(EvalErrorKind::DivisionByZero);
            }
            Ok(Value::Float(not_nan(a as f64 / b.into_inner(), "/")?))
        }
        (Value::Float(a), Value::Int(b)) => {
            if b == 0 {
                return Err(EvalErrorKind::DivisionByZero);
            }
            Ok(Value::Float(not_nan(a.into_inner() / b as f64, "/")?))
        }
        (lhs, _) => Err(EvalErrorKind::UnexpectedValue(lhs)),
    }
}

fn eval_numeric_comparison(
    op: ast::BinaryOp,
    lhs: Value,
    rhs: Value,
    int_cmp: fn(i64, i64) -> bool,
    float_cmp: fn(f64, f64) -> bool,
) -> Result<Value, EvalErrorKind> {
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(int_cmp(a, b))),
        (Value::Float(a), Value::Float(b)) => {
            Ok(Value::Bool(float_cmp(a.into_inner(), b.into_inner())))
        }
        (Value::Int(a), Value::Float(b)) => Ok(Value::Bool(float_cmp(a as f64, b.into_inner()))),
        (Value::Float(a), Value::Int(b)) => Ok(Value::Bool(float_cmp(a.into_inner(), b as f64))),
        (lhs, rhs) => Err(EvalErrorKind::InvalidComparison { op, lhs, rhs }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    use super::{Effect, Eval, EvalEnv, GlobalEvalEnv};
    use ids::ResourceId;

    use crate::{ExternFnValue, ModuleId, Resource, TrackedValue, Value};

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
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "seed".to_string(),
        };
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge).with_module_id(&module_id).with_local(
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
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "arg".to_string(),
        };
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
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
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "arg".to_string(),
        };
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
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
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let callee_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "callee".to_string(),
        };
        let arg_dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "arg".to_string(),
        };
        let fn_value = Value::Fn(crate::FnValue {
            env: crate::FnEnv {
                module_id: module_id.clone(),
                raw_module_id: None,
                captures: std::collections::HashMap::new(),
                parameters: vec!["x".to_string()],
                self_name: None,
                recursive_group: None,
            },
            body: parse_expr("123", &module_id),
        });
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
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
        let mut eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let id = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "x".to_string(),
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
                markers: Default::default(),
            },
        );
        let dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "seed".to_string(),
        };
        let mut dependencies = std::collections::BTreeSet::new();
        dependencies.insert(dependency.clone());

        let outputs = eval
            .ctx
            .resource(id.typ.clone(), id.name.clone(), &inputs, dependencies)
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
        let mut eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let id = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "x".to_string(),
        };
        let mut inputs = crate::Record::default();
        inputs.insert("min".to_string(), Value::Int(1));
        inputs.insert("max".to_string(), Value::Int(2));
        let dependency = ResourceId {
            typ: "Std/Random.Int".to_string(),
            name: "seed".to_string(),
        };
        eval.add_resource(
            id.clone(),
            Resource {
                inputs: inputs.clone(),
                outputs: crate::Record::default(),
                dependencies: vec![dependency.clone()],
                markers: Default::default(),
            },
        );
        let mut dependencies = std::collections::BTreeSet::new();
        dependencies.insert(dependency.clone());

        let outputs = eval
            .ctx
            .resource(id.typ.clone(), id.name.clone(), &inputs, dependencies)
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

    #[test]
    fn integer_add_overflow_is_trapped() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("x", TrackedValue::new(Value::Int(i64::MAX)))
            .with_local("y", TrackedValue::new(Value::Int(1)));
        let expr = parse_expr("x + y", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("overflow should trap");
        assert!(
            matches!(err.kind, super::EvalErrorKind::IntegerOverflow { ref op } if op == "+"),
            "expected IntegerOverflow(+), got {:?}",
            err.kind
        );
    }

    #[test]
    fn integer_sub_overflow_is_trapped() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("x", TrackedValue::new(Value::Int(i64::MIN)))
            .with_local("y", TrackedValue::new(Value::Int(1)));
        let expr = parse_expr("x - y", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("overflow should trap");
        assert!(
            matches!(err.kind, super::EvalErrorKind::IntegerOverflow { ref op } if op == "-"),
            "expected IntegerOverflow(-), got {:?}",
            err.kind
        );
    }

    #[test]
    fn integer_mul_overflow_is_trapped() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("x", TrackedValue::new(Value::Int(i64::MAX)))
            .with_local("y", TrackedValue::new(Value::Int(2)));
        let expr = parse_expr("x * y", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("overflow should trap");
        assert!(
            matches!(err.kind, super::EvalErrorKind::IntegerOverflow { ref op } if op == "*"),
            "expected IntegerOverflow(*), got {:?}",
            err.kind
        );
    }

    #[test]
    fn calling_non_function_raises_eval_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("f", TrackedValue::new(Value::Int(42)));
        let expr = parse_expr("f()", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("calling non-function should trap");
        assert!(
            matches!(
                err.kind,
                super::EvalErrorKind::UnexpectedValue(Value::Int(42))
            ),
            "expected UnexpectedValue(Int(42)), got {:?}",
            err.kind
        );
    }

    #[test]
    fn property_access_on_non_record_raises_eval_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("x", TrackedValue::new(Value::Int(42)));
        let expr = parse_expr("x.foo", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("property access on non-record should trap");
        assert!(
            matches!(
                err.kind,
                super::EvalErrorKind::UnexpectedValue(Value::Int(42))
            ),
            "expected UnexpectedValue(Int(42)), got {:?}",
            err.kind
        );
    }

    #[test]
    fn indexed_access_on_non_container_raises_eval_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let eval = Eval::from_externs(
            HashMap::new(),
            super::EvalCtx::new(tx, "test/namespace", crate::placeholder_deployment_qid()),
        );
        let module_id = ModuleId::default();
        let ge = GlobalEvalEnv::default();
        let env = EvalEnv::new(&ge)
            .with_module_id(&module_id)
            .with_local("x", TrackedValue::new(Value::Int(42)));
        let expr = parse_expr("x[0]", &module_id);

        let err = eval
            .eval_expr(&env, &expr)
            .expect_err("indexed access on non-container should trap");
        assert!(
            matches!(
                err.kind,
                super::EvalErrorKind::UnexpectedValue(Value::Int(42))
            ),
            "expected UnexpectedValue(Int(42)), got {:?}",
            err.kind
        );
    }
}
