/// `Std/Package` has no extern functions today; its sole purpose is to host
/// the [`Manifest`](Package.scl) type used by the cross-repo imports
/// machinery. The empty `register_extern` is required by the `std_modules!`
/// macro shape.
pub fn register_extern(_eval: &mut impl super::ExternRegistry) {}
