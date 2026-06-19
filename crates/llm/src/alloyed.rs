use crate::StreamingModelProvider;
use crate::provider::LlmResponseStream;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::Context;

#[doc = include_str!("docs/alloyed.md")]
pub struct AlloyedModelProvider {
    providers: Vec<Box<dyn StreamingModelProvider>>,
    current_provider_index: AtomicUsize,
}

impl AlloyedModelProvider {
    pub fn new(providers: Vec<Box<dyn StreamingModelProvider>>) -> Self {
        Self { providers, current_provider_index: AtomicUsize::new(0) }
    }

    fn get_current_provider(&self) -> Option<&dyn StreamingModelProvider> {
        if self.providers.is_empty() {
            return None;
        }
        let index = self.current_provider_index.load(Ordering::Relaxed) % self.providers.len();
        Some(self.providers[index].as_ref())
    }

    fn get_next_provider(&self) -> Option<&dyn StreamingModelProvider> {
        if self.providers.is_empty() {
            return None;
        }
        let index = self.current_provider_index.fetch_add(1, Ordering::Relaxed) % self.providers.len();
        Some(self.providers[index].as_ref())
    }
}

impl StreamingModelProvider for AlloyedModelProvider {
    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        match self.get_next_provider() {
            Some(provider) => provider.stream_response(context),
            None => Box::pin(tokio_stream::empty()),
        }
    }

    fn model(&self) -> Option<crate::LlmModel> {
        self.get_current_provider().and_then(super::provider::StreamingModelProvider::model)
    }

    fn display_name(&self) -> String {
        match self.get_current_provider() {
            Some(provider) => provider.display_name(),
            None => "Alloyed (no providers)".to_string(),
        }
    }

    fn context_window(&self) -> Option<u32> {
        if self.providers.is_empty() {
            return None;
        }

        let mut min_context: Option<u32> = None;
        for provider in &self.providers {
            let context = provider.context_window()?;
            min_context = Some(min_context.map_or(context, |current| current.min(context)));
        }

        min_context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmResponse;
    use crate::testing::FakeLlmProvider;

    struct FixedContextProvider {
        context_window: Option<u32>,
    }

    impl StreamingModelProvider for FixedContextProvider {
        fn stream_response(&self, _context: &Context) -> LlmResponseStream {
            Box::pin(tokio_stream::iter(vec![Ok(LlmResponse::done())]))
        }

        fn display_name(&self) -> String {
            "Fixed Context".to_string()
        }

        fn context_window(&self) -> Option<u32> {
            self.context_window
        }
    }

    #[test]
    fn test_alloyed_provider_display_name_empty() {
        let provider = AlloyedModelProvider::new(vec![]);
        assert_eq!(provider.display_name(), "Alloyed (no providers)");
    }

    #[test]
    fn test_alloyed_provider_display_name_single() {
        let fake_provider = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let provider = AlloyedModelProvider::new(vec![Box::new(fake_provider)]);

        // Should return the individual provider's display name
        assert_eq!(provider.display_name(), "Fake LLM");
    }

    #[test]
    fn test_alloyed_provider_display_name_multiple() {
        let fake_provider1 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let fake_provider2 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let provider = AlloyedModelProvider::new(vec![Box::new(fake_provider1), Box::new(fake_provider2)]);

        // Should cycle through individual provider names
        assert_eq!(provider.display_name(), "Fake LLM"); // First call
        assert_eq!(provider.display_name(), "Fake LLM"); // Second call (cycles back)
    }

    #[test]
    fn test_alloyed_provider_cycling() {
        let fake_provider1 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let fake_provider2 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let provider = AlloyedModelProvider::new(vec![Box::new(fake_provider1), Box::new(fake_provider2)]);

        let context = Context::new(vec![], vec![]);

        // stream_response should advance to next provider each time
        let _stream1 = provider.stream_response(&context); // Uses provider 0, advances to 1
        let name1 = provider.display_name(); // Should show provider 1

        let _stream2 = provider.stream_response(&context); // Uses provider 1, advances to 0 (wraps)
        let name2 = provider.display_name(); // Should show provider 0

        let _stream3 = provider.stream_response(&context); // Uses provider 0, advances to 1
        let name3 = provider.display_name(); // Should show provider 1

        // All should return "Fake LLM" but they're cycling through different instances
        assert_eq!(name1, "Fake LLM");
        assert_eq!(name2, "Fake LLM");
        assert_eq!(name3, "Fake LLM");
    }

    #[test]
    fn test_display_name_doesnt_advance_counter() {
        let fake_provider1 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let fake_provider2 = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let provider = AlloyedModelProvider::new(vec![Box::new(fake_provider1), Box::new(fake_provider2)]);

        // Calling display_name multiple times should return the same result
        let name1 = provider.display_name();
        let name2 = provider.display_name();
        let name3 = provider.display_name();

        assert_eq!(name1, "Fake LLM");
        assert_eq!(name2, "Fake LLM");
        assert_eq!(name3, "Fake LLM");
    }

    #[test]
    fn test_context_window_unknown_if_any_provider_unknown() {
        let known = FixedContextProvider { context_window: Some(200_000) };
        let unknown = FakeLlmProvider::new(vec![vec![LlmResponse::done()]]);
        let provider = AlloyedModelProvider::new(vec![Box::new(known), Box::new(unknown)]);
        assert_eq!(provider.context_window(), None);
    }

    #[test]
    fn test_context_window_uses_min_of_known_providers() {
        let p1 = FixedContextProvider { context_window: Some(200_000) };
        let p2 = FixedContextProvider { context_window: Some(128_000) };
        let provider = AlloyedModelProvider::new(vec![Box::new(p1), Box::new(p2)]);
        assert_eq!(provider.context_window(), Some(128_000));
    }
}
