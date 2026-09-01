use crate::Record;
use crate::normalize::normalize_label;

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    records: Vec<Record>,
}

impl Catalog {
    pub fn new(records: Vec<Record>) -> Self {
        Self { records }
    }

    pub fn active_by_category(&self, category: &str) -> Vec<&Record> {
        let wanted = normalize_label(category);
        self.records
            .iter()
            .filter(|record| record.active && normalize_label(&record.category) == wanted)
            .collect()
    }

    pub fn find_by_label(&self, label: &str) -> Option<&Record> {
        let wanted = normalize_label(label);
        self.records
            .iter()
            .find(|record| normalize_label(&record.label) == wanted)
    }
}
