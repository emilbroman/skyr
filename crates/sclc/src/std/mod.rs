use std::{collections::HashMap, convert::Infallible, path::Path};

use crate::{ModuleId, SourceRepo};

macro_rules! std_modules {
    (@unit $module:ident => $scl:literal) => {
        ()
    };
    ($($module:ident => $scl:literal),* $(,)?) => {
        $(mod $module;)*

        const BUNDLED_FILES: [(&'static str, &'static [u8]); <[()]>::len(&[$(std_modules!(@unit $module => $scl)),*])] = [
            $((
                $scl,
                include_bytes!($scl) as &'static [u8],
            )),*
        ];

        fn register_std_externs(eval: &mut crate::Eval) {
            $(
                $module::register_extern(eval);
            )*
        }
    };
}

std_modules! {
    encoding => "Encoding.scl",
    num => "Num.scl",
    random => "Random.scl",
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

impl SourceRepo for StdSourceRepo {
    type Err = Infallible;

    fn package_id(&self) -> ModuleId {
        [String::from("Std")].into()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        let key = Self::normalize(path);
        Ok(self.files.get(&key).map(|data| data.to_vec()))
    }

    fn register_extern(eval: &mut crate::Eval) {
        register_std_externs(eval);
    }
}
