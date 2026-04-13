use std::collections::HashMap;

use crate::{PackageId, RecordType, Type};

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
        /// Doesn't require an `Eval` reference.
        pub(crate) fn collect_std_externs() -> HashMap<String, crate::Value> {
            let mut collector = ExternCollector(HashMap::new());
            register_std_externs(&mut collector);
            collector.0
        }
    };
}

/// Trait for types that can receive extern function registrations.
///
/// Implemented by [`crate::Eval`] and by [`ExternCollector`].
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
    package => "Package.scl",
    random => "Random.scl",
    time => "Time.scl",
}

/// Compiles all standard library modules and returns the value-level type and
/// type-level type exports for each module, keyed by module ID (e.g. `Std/Time`).
pub async fn stdlib_types()
-> Result<HashMap<crate::ModuleId, (Type, RecordType)>, crate::CompileError> {
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

    let user_pkg = Arc::new(crate::InMemoryPackage::new(
        crate::PackageId::from(["_StdlibTypes"]),
        files,
    ));
    let finder = crate::build_default_finder(user_pkg);

    let mut diags = crate::DiagList::new();
    let asg = crate::compile(finder, &["_StdlibTypes", "Main"])
        .await?
        .unpack(&mut diags);

    let modules: HashMap<crate::ModuleId, crate::ast::FileMod> = asg
        .modules()
        .filter_map(|mn| {
            mn.body
                .as_file_mod()
                .map(|fm| (mn.module_id.clone(), fm.clone()))
        })
        .collect();
    let package_names: Vec<PackageId> = asg.packages().keys().cloned().collect();
    let checker = crate::TypeChecker::from_modules(&modules, package_names);
    let std_package_id = PackageId::from([String::from("Std")]);

    let mut result = HashMap::new();

    for (module_id, file_mod) in &modules {
        if module_id.package != std_package_id {
            continue;
        }
        let ge = crate::GlobalTypeEnv::default();
        let env = crate::TypeEnv::new(&ge).with_module_id(module_id);

        let mut diags = crate::DiagList::new();

        let value_type = checker.check_file_mod(&env, file_mod)?.unpack(&mut diags);

        let type_level = checker
            .type_level_exports(&env, file_mod)
            .unpack(&mut diags);

        result.insert(module_id.clone(), (value_type, type_level));
    }

    Ok(result)
}
