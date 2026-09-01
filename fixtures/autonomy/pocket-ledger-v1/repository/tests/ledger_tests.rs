use pocket_ledger::{Catalog, ParseError, Record, parse_record, summarize};

fn record(id: u32, label: &str, category: &str, active: bool) -> Record {
    Record::new(id, label, category, active).unwrap()
}

#[test]
fn parses_valid_records_and_normalizes_state_case() {
    assert_eq!(
        parse_record("7 | Quarterly Report | Finance | ACTIVE").unwrap(),
        record(7, "Quarterly Report", "Finance", true)
    );
}

#[test]
fn rejects_wrong_field_count_and_invalid_state() {
    assert_eq!(
        parse_record("1|label|category"),
        Err(ParseError::FieldCount { found: 3 })
    );
    assert_eq!(
        parse_record("1|label|category|unknown"),
        Err(ParseError::InvalidActive)
    );
}

#[test]
fn catalog_queries_are_case_insensitive() {
    let catalog = Catalog::new(vec![
        record(1, "Alpha", "Ops", true),
        record(2, "Beta", "ops", false),
        record(3, "Gamma", "Sales", true),
    ]);
    assert_eq!(catalog.active_by_category(" OPS ").len(), 1);
    assert_eq!(catalog.find_by_label(" alpha ").unwrap().id, 1);
}

#[test]
fn summary_counts_state_and_category() {
    let records = vec![
        record(1, "Alpha", "Ops", true),
        record(2, "Beta", "ops", false),
        record(3, "Gamma", "Sales", true),
    ];
    let summary = summarize(&records);
    assert_eq!(summary.total, 3);
    assert_eq!(summary.active, 2);
    assert_eq!(summary.by_category["ops"], 2);
    assert_eq!(summary.by_category["sales"], 1);
}
