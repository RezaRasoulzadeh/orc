use std::fmt;

use crate::Record;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    FieldCount { found: usize },
    InvalidId,
    EmptyLabel,
    EmptyCategory,
    InvalidActive,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FieldCount { found } => write!(formatter, "expected 4 fields, found {found}"),
            Self::InvalidId => formatter.write_str("id must be a positive integer"),
            Self::EmptyLabel => formatter.write_str("label must not be empty"),
            Self::EmptyCategory => formatter.write_str("category must not be empty"),
            Self::InvalidActive => formatter.write_str("state must be active or inactive"),
        }
    }
}

impl std::error::Error for ParseError {}

pub fn parse_record(line: &str) -> Result<Record, ParseError> {
    let fields = line.split('|').collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err(ParseError::FieldCount {
            found: fields.len(),
        });
    }
    let id = fields[0]
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(ParseError::InvalidId)?;
    let label = fields[1].trim();
    if label.is_empty() {
        return Err(ParseError::EmptyLabel);
    }
    let category = fields[2].trim();
    if category.is_empty() {
        return Err(ParseError::EmptyCategory);
    }
    let active = match fields[3].trim().to_ascii_lowercase().as_str() {
        "active" => true,
        "inactive" => false,
        _ => return Err(ParseError::InvalidActive),
    };
    Record::new(id, label, category, active).map_err(|_| ParseError::InvalidId)
}
