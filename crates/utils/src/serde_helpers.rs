/// Serde `skip_serializing_if` helper for `bool` fields that default to `false`.
///
/// Takes `&bool` rather than `bool` because `skip_serializing_if` calls the
/// predicate with a reference to the field.
#[allow(clippy::trivially_copy_pass_by_ref)]
pub fn is_false(b: &bool) -> bool {
    !*b
}
