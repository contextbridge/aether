use client_core::format::token_count;

#[test]
fn token_counts_read_in_thousands_and_millions_to_a_tenth() {
    let cases = [
        (0, "0"),
        (850, "850"),
        (1_000, "1k"),
        (12_000, "12k"),
        (12_345, "12.3k"),
        (999_999, "999.9k"),
        (1_000_000, "1M"),
        (1_250_000, "1.2M"),
    ];

    for (count, expected) in cases {
        assert_eq!(token_count(count), expected, "{count}");
    }
}
