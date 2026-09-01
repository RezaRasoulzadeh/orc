/// Produces a stable comparison key for user-visible labels.
pub fn normalize_label(input: &str) -> String {
    input.trim().to_lowercase()
}
