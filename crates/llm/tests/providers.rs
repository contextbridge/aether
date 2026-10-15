mod providers {
    pub mod common;

    mod anthropic {
        mod capture_fixtures;
        mod fixture_tests;
    }
    mod chat_completions_fixture_tests;
    mod openai {
        mod capture_fixtures;
    }
    mod openrouter {
        mod capture_fixtures;
    }
    mod z_ai {
        mod capture_fixtures;
    }
}
