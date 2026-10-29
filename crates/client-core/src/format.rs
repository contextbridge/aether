pub fn token_count(count: u64) -> String {
    let (unit, suffix) = match count {
        0..1_000 => return count.to_string(),
        1_000..1_000_000 => (1_000, "k"),
        _ => (1_000_000, "M"),
    };
    let tenths = count * 10 / unit;
    match tenths % 10 {
        0 => format!("{}{suffix}", tenths / 10),
        tenth => format!("{}.{tenth}{suffix}", tenths / 10),
    }
}
