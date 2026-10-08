use std::any::Any;

pub fn panic_message(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_formatted_message_from_caught_panic() {
        let payload = std::panic::catch_unwind(|| panic!("boom {}", 1)).unwrap_err();
        assert_eq!(panic_message(&*payload), "boom 1");
    }

    #[test]
    fn extracts_static_message_from_caught_panic() {
        let payload = std::panic::catch_unwind(|| panic!("boom")).unwrap_err();
        assert_eq!(panic_message(&*payload), "boom");
    }
}
