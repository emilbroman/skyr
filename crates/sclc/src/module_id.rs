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

/// Identifies a module within a specific package.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    pub package: PackageId,
    pub path: Vec<String>,
}

impl ModuleId {
    pub fn new(package: PackageId, path: Vec<String>) -> Self {
        Self { package, path }
    }

    /// Returns `true` if every path segment is a safe identifier (non-empty, no `.`,
    /// no `/`, no `\`, and not `..`). This prevents path-traversal when the
    /// module id is converted to a filesystem path via
    /// [`to_path_buf_with_extension`](Self::to_path_buf_with_extension).
    pub fn is_safe_path(&self) -> bool {
        self.path.iter().all(|s| {
            !s.is_empty() && s != ".." && s != "." && !s.contains('/') && !s.contains('\\')
        })
    }

    pub fn is_empty(&self) -> bool {
        self.package.is_empty() && self.path.is_empty()
    }

    /// Convert the module path (not the package) to a filesystem path with the
    /// given extension.
    pub fn to_path_buf_with_extension(&self, extension: &str) -> PathBuf {
        let mut path = PathBuf::new();
        for segment in &self.path {
            path.push(segment);
        }
        path.set_extension(extension);
        path
    }

    /// Returns all segments as a flat slice: package segments followed by path segments.
    /// This is a convenience for code that needs backward-compatible flat segment access.
    pub fn all_segments(&self) -> Vec<String> {
        let mut segments = self.package.as_slice().to_vec();
        segments.extend(self.path.iter().cloned());
        segments
    }
}

impl serde::Serialize for ModuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ModuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize as flat string "Package/Module/Path" — assumes first segment is
        // the package. This is a lossy heuristic for backward compatibility.
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        let segments: Vec<String> = s
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect();
        if segments.is_empty() {
            return Ok(ModuleId::default());
        }
        Ok(ModuleId {
            package: PackageId::new(vec![segments[0].clone()]),
            path: segments[1..].to_vec(),
        })
    }
}

impl std::fmt::Display for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.package.is_empty() {
            write!(f, "{}", self.path.join("/"))
        } else if self.path.is_empty() {
            write!(f, "{}", self.package)
        } else {
            write!(f, "{}/{}", self.package, self.path.join("/"))
        }
    }
}

impl std::fmt::Debug for ModuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleId(\"{}\")", self)
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
    fn module_id_display() {
        let id = ModuleId::new(PackageId::from(["Std"]), vec!["Time".to_string()]);
        assert_eq!(id.to_string(), "Std/Time");

        let id = ModuleId::new(PackageId::default(), vec!["Main".to_string()]);
        assert_eq!(id.to_string(), "Main");

        let id = ModuleId::new(PackageId::from(["Std"]), vec![]);
        assert_eq!(id.to_string(), "Std");
    }

    #[test]
    fn module_id_to_path() {
        let id = ModuleId::new(
            PackageId::from(["Std"]),
            vec!["Time".to_string(), "Sleep".to_string()],
        );
        assert_eq!(
            id.to_path_buf_with_extension("scl"),
            PathBuf::from("Time/Sleep.scl")
        );
    }

    #[test]
    fn safe_path_accepts_normal_segments() {
        let id = ModuleId::new(PackageId::from(["Std"]), vec!["Time".to_string()]);
        assert!(id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_dot_dot() {
        let id = ModuleId::new(
            PackageId::from(["Std"]),
            vec!["..".to_string(), "etc".to_string()],
        );
        assert!(!id.is_safe_path());
    }

    #[test]
    fn safe_path_rejects_empty_segment() {
        let id = ModuleId::new(
            PackageId::from(["Std"]),
            vec!["".to_string(), "Foo".to_string()],
        );
        assert!(!id.is_safe_path());
    }

    #[test]
    fn all_segments() {
        let id = ModuleId::new(PackageId::from(["Std"]), vec!["Time".to_string()]);
        assert_eq!(id.all_segments(), vec!["Std", "Time"]);
    }
}
