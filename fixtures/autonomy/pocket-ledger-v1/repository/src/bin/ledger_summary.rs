use std::io::{self, BufRead};

use pocket_ledger::{parse_record, summarize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for line in io::stdin().lock().lines() {
        records.push(parse_record(&line?)?);
    }
    let summary = summarize(&records);
    println!("total={} active={}", summary.total, summary.active);
    for (category, count) in summary.by_category {
        println!("{category}={count}");
    }
    Ok(())
}
