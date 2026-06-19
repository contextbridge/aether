mod providers {
    pub mod common;

    mod anthropic {
        mod capture_fixtures;
        mod fixture_tests;
    }
    mod openai {
        mod capture_fixtures;
        mod fixture_tests;
        mod streaming_tests;
    }
    mod openrouter {
        mod capture_fixtures;
        mod fixture_tests;
        mod usage_tests;
    }
    mod z_ai {
        mod capture_fixtures;
        mod fixture_tests;
        mod types_tests;
    }
    #[cfg(feature = "codex")]
    mod codex {
        mod fixture_tests;
    }
}
