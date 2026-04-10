use std::{collections::HashMap, path::Path};

use crate::{ChildEntry, PackageId, RecordType, SourceError, SourceRepo, Type};

macro_rules! std_modules {
    (@unit $module:ident => $scl:literal) => {
        ()
    };
    ($($module:ident => $scl:literal),* $(,)?) => {
        $(mod $module;)*

        pub(crate) const BUNDLED_FILES: [(&'static str, &'static [u8]); <[()]>::len(&[$(std_modules!(@unit $module => $scl)),*])] = [
            $((
                $scl,
                include_bytes!($scl) as &'static [u8],
            )),*
        ];

        pub(crate) fn register_std_externs(registry: &mut impl ExternRegistry) {
            $(
                $module::register_extern(registry);
            )*
        }

        /// Collects all standard library extern functions into a map.
        ///
        /// This is the v2-compatible equivalent of [`register_std_externs`] that
        /// doesn't require an `Eval` reference.
        pub(crate) fn collect_std_externs() -> HashMap<String, crate::Value> {
            let mut collector = ExternCollector(HashMap::new());
            register_std_externs(&mut collector);
            collector.0
        }
    };
}

/// Trait for types that can receive extern function registrations.
///
/// Implemented by [`crate::Eval`] (for the v1 pipeline) and by
/// [`ExternCollector`] (for the v2 pipeline).
pub trait ExternRegistry {
    fn add_extern_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(
            Vec<crate::TrackedValue>,
            &crate::EvalCtx,
        ) -> Result<crate::TrackedValue, crate::EvalError>
        + Clone
        + Send
        + Sync
        + 'static,
    );
}

/// Collects extern functions into a `HashMap<String, Value>` without
/// requiring an [`Eval`](crate::Eval) instance.
pub(crate) struct ExternCollector(pub HashMap<String, crate::Value>);

impl ExternRegistry for ExternCollector {
    fn add_extern_fn(
        &mut self,
        name: impl Into<String>,
        f: impl Fn(
            Vec<crate::TrackedValue>,
            &crate::EvalCtx,
        ) -> Result<crate::TrackedValue, crate::EvalError>
        + Clone
        + Send
        + Sync
        + 'static,
    ) {
        self.0.insert(
            name.into(),
            crate::Value::ExternFn(crate::ExternFnValue::new(Box::new(f))),
        );
    }
}

std_modules! {
    artifact => "Artifact.scl",
    container => "Container.scl",
    crypto => "Crypto.scl",
    dns => "DNS.scl",
    encoding => "Encoding.scl",
    list => "List.scl",
    num => "Num.scl",
    option => "Option.scl",
    random => "Random.scl",
    time => "Time.scl",
}

#[derive(Clone)]
pub struct StdSourceRepo {
    files: HashMap<String, &'static [u8]>,
}

impl StdSourceRepo {
    pub fn new() -> Self {
        // These are embedded into the executable at compile-time.
        let files = BUNDLED_FILES
            .iter()
            .map(|(path, bytes)| (path.to_string(), *bytes))
            .collect();
        Self { files }
    }

    fn normalize(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }
}

impl Default for StdSourceRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SourceRepo for StdSourceRepo {
    fn package_id(&self) -> PackageId {
        [String::from("Std")].into()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, SourceError> {
        let key = Self::normalize(path);
        Ok(self.files.get(&key).map(|data| data.to_vec()))
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, SourceError> {
        let prefix = Self::normalize(path);
        let mut entries = std::collections::BTreeSet::new();
        for key in self.files.keys() {
            let relative = if prefix.is_empty() {
                key.as_str()
            } else if let Some(rest) = key.strip_prefix(&prefix).and_then(|r| r.strip_prefix('/')) {
                rest
            } else {
                continue;
            };
            // Take the first path segment of `relative`
            if let Some(slash_pos) = relative.find('/') {
                entries.insert(ChildEntry::Directory(relative[..slash_pos].to_owned()));
            } else {
                entries.insert(ChildEntry::File(relative.to_owned()));
            }
        }
        Ok(entries.into_iter().collect())
    }
}

/// Compiles all standard library modules and returns the value-level type and
/// type-level type exports for each module, keyed by module ID (e.g. `Std/Time`).
pub async fn stdlib_types()
-> Result<HashMap<crate::ModuleId, (Type, RecordType)>, crate::v2::V2CompileError> {
    use std::path::PathBuf;
    use std::sync::Arc;

    // Derive module names from the embedded .scl files.
    let module_names: Vec<&str> = BUNDLED_FILES
        .iter()
        .filter_map(|(path, _)| path.strip_suffix(".scl"))
        .collect();

    // Build a Main.scl that imports every stdlib module so that
    // the Loader discovers and parses them all.
    let main_scl = module_names
        .iter()
        .map(|m| format!("import Std/{m}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut files = HashMap::new();
    files.insert(PathBuf::from("Main.scl"), main_scl.into_bytes());

    let user_pkg = Arc::new(crate::v2::InMemoryPackage::new(
        crate::PackageId::from(["_StdlibTypes"]),
        files,
    ));
    let finder = crate::v2::build_default_finder(user_pkg);

    let mut diags = crate::DiagList::new();
    let asg = crate::v2::compile(finder, &["_StdlibTypes", "Main"])
        .await?
        .unpack(&mut diags);

    let unit = crate::v2::asg_to_compilation_unit(&asg);
    let checker = crate::TypeChecker::new(&unit);
    let std_package_id = PackageId::from([String::from("Std")]);

    let mut result = HashMap::new();

    for (module_id, file_mod) in unit.modules() {
        if module_id.package != std_package_id {
            continue;
        }
        let env = crate::TypeEnv::new().with_module_id(module_id);

        let mut diags = crate::DiagList::new();

        let value_type = checker.check_file_mod(&env, file_mod)?.unpack(&mut diags);

        let type_level = checker
            .type_level_exports(&env, file_mod)
            .unpack(&mut diags);

        result.insert(module_id.clone(), (value_type, type_level));
    }

    Ok(result)
}
