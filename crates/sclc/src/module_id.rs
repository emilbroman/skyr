use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    segments: Vec<String>,
}

impl ModuleId {
    pub fn new(segments: Vec<String>) -> Self {
        Self { segments }
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn starts_with(&self, prefix: &Self) -> bool {
        self.segments.starts_with(prefix.segments.as_slice())
    }

    pub fn suffix_after(&self, prefix: &Self) -> Option<&[String]> {
        if !self.starts_with(prefix) {
            return None;
        }

        Some(&self.segments[prefix.len()..])
    }

    pub fn to_path_buf_with_extension(&self, extension: &str) -> PathBuf {
        let mut path = PathBuf::new();
        for segment in &self.segments {
            path.push(segment);
        }
        path.set_extension(extension);
        path
    }

    pub fn as_slice(&self) -> &[String] {
        &self.segments
    }
}

impl FromIterator<String> for ModuleId {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self {
            segments: iter.into_iter().collect(),
        }
    }
}
