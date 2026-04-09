use std::path::PathBuf;
use std::str::FromStr;

/// Identifies a package (e.g. `Std`, `MyOrg/MyRepo`).
///
/// Packages have unique-prefix identity: no two loaded packages share a prefix.
/// Uses `Vec<String>` segments so that prefix matching against import paths is
/// done at the segment level, avoiding ambiguity (e.g. `Std` vs `Stdout`).
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    segments: Vec<String>,
}

impl PackageId {
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

    pub fn as_slice(&self) -> &[String] {
        &self.segments
    }
}

impl FromIterator<String> for PackageId {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self {
            segments: iter.into_iter().collect(),
        }
    }
}

impl<I> From<I> for PackageId
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

impl serde::Serialize for PackageId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for PackageId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(s.parse().unwrap())
    }
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.segments.join("/"))
    }
}

impl std::fmt::Debug for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PackageId(\"{}\")", self)
    }
}

impl FromStr for PackageId {
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

#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    segments: Vec<String>,
}

impl ModuleId {
    pub fn new(segments: Vec<String>) -> Self {
        Self { segments }
    }

    /// Returns `true` if every segment is a safe identifier (non-empty, no `.`,
    /// no `/`, no `\`, and not `..`). This prevents path-traversal when the
    /// module id is converted to a filesystem path via
    /// [`to_path_buf_with_extension`](Self::to_path_buf_with_extension).
    pub fn is_safe_path(&self) -> bool {
        self.segments.iter().all(|s| {
            !s.is_empty() && s != ".." && s != "." && !s.contains('/') && !s.contains('\\')
        })
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

    /// Returns `true` if the module ID starts with the given package prefix.
    pub fn starts_with_package(&self, prefix: &PackageId) -> bool {
        self.segments.starts_with(prefix.as_slice())
    }

    pub fn suffix_after(&self, prefix: &Self) -> Option<&[String]> {
        if !self.starts_with(prefix) {
            return None;
        }

        Some(&self.segments[prefix.len()..])
    }

    /// Returns the suffix of this module ID after the given package prefix,
    /// or `None` if this module ID does not start with the package prefix.
    pub fn suffix_after_package(&self, prefix: &PackageId) -> Option<&[String]> {
        if !self.starts_with_package(prefix) {
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

impl serde::Serialize for ModuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ModuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(s.parse().unwrap())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_id_display() {
        let id = PackageId::from(["Std"]);
        assert_eq!(id.to_string(), "Std");
        let id = PackageId::from(["MyOrg", "MyRepo"]);
        assert_eq!(id.to_string(), "MyOrg/MyRepo");
    }

    #[test]
    fn package_id_starts_with() {
        let full = PackageId::from(["Std", "Time"]);
        let prefix = PackageId::from(["Std"]);
        assert!(full.starts_with(&prefix));
        assert!(!prefix.starts_with(&full));
    }

    #[test]
    fn module_id_starts_with_package() {
        let module = ModuleId::from(["Std", "Time", "Sleep"]);
        let pkg = PackageId::from(["Std"]);
        assert!(module.starts_with_package(&pkg));

        let other_pkg = PackageId::from(["Other"]);
        assert!(!module.starts_with_package(&other_pkg));
    }

    #[test]
    fn module_id_suffix_after_package() {
        let module = ModuleId::from(["Std", "Time", "Sleep"]);
        let pkg = PackageId::from(["Std"]);
        assert_eq!(
            module.suffix_after_package(&pkg),
            Some(["Time".to_string(), "Sleep".to_string()].as_slice())
        );
    }

    #[test]
    fn safe_path_accepts_normal_segments() {
        let id = ModuleId::from(["Std", "Time"]);
        assert!(id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_dot_dot() {
        let id = ModuleId::from(["Std", "..", "etc"]);
        assert!(!id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_single_dot() {
        let id = ModuleId::from([".", "Foo"]);
        assert!(!id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_slash_in_segment() {
        let id = ModuleId::from(["Std/../../etc"]);
        assert!(!id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_empty_segment() {
        let id = ModuleId::from(["Std", "", "Foo"]);
        assert!(!id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_backslash_in_segment() {
        let id = ModuleId::from(["Std\\..\\etc"]);
        assert!(!id.is_safe_path());
    }
}
