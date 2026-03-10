use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
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

impl<I> From<I> for ModuleId
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    fn from(value: I) -> Self {
        Self {
            segments: value.into_iter().map(Into::into).collect(),
        }
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.segments.join("/"))
    }
}

impl std::fmt::Debug for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleId(\"{}\")", self)
    }
}

impl FromStr for ModuleId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            segments: s
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}
