#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: u32,
    pub label: String,
    pub category: String,
    pub active: bool,
}

impl Record {
    pub fn new(
        id: u32,
        label: impl Into<String>,
        category: impl Into<String>,
        active: bool,
    ) -> Result<Self, &'static str> {
        let label = label.into();
        let category = category.into();
        if id == 0 {
            return Err("id must be positive");
        }
        if label.trim().is_empty() {
            return Err("label must not be empty");
        }
        if category.trim().is_empty() {
            return Err("category must not be empty");
        }
        Ok(Self {
            id,
            label: label.trim().to_owned(),
            category: category.trim().to_owned(),
            active,
        })
    }
}
