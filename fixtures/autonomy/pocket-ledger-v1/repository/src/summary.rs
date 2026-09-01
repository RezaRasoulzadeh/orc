use std::collections::BTreeMap;

use crate::Record;
use crate::normalize::normalize_label;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LedgerSummary {
    pub total: usize,
    pub active: usize,
    pub by_category: BTreeMap<String, usize>,
}

pub fn summarize(records: &[Record]) -> LedgerSummary {
    let mut summary = LedgerSummary {
        total: records.len(),
        ..LedgerSummary::default()
    };
    for record in records {
        if record.active {
            summary.active += 1;
        }
        *summary
            .by_category
            .entry(normalize_label(&record.category))
            .or_default() += 1;
    }
    summary
}
