//! Print one corpus case's report: `cargo run -p openmhs-outcome --example show -- <case dir>`.
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("a case directory"));
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).expect("readable");
    let contract = serde_json::from_str(&read("contract.json")).expect("a contract");
    let records = openmhs_outcome::parse_evidence(&read("evidence.jsonl")).expect("evidence");
    let report = openmhs_outcome::verify(&contract, &records).expect("checkable");
    println!("{}", String::from_utf8_lossy(&report.to_canonical_json()));
}
