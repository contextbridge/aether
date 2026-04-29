use mcp_servers::coding::error::BashError;
use mcp_servers::coding::tools::bash::{BashInput, BashResult, execute_command, read_background_bash};
use std::time::Duration;

#[tokio::test]
async fn test_basic_command() {
    let args = BashInput {
        command: "echo 'hello world'".to_string(),
        timeout: None,
        description: None,
        run_in_background: None,
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Completed(output) => {
            assert_eq!(output.output.trim(), "hello world");
            assert_eq!(output.exit_code, 0);
            assert_eq!(output.killed, Some(false));
            assert_eq!(output.shell_id, None);
        }
        BashResult::Background(_) => panic!("Expected completed result, got background"),
    }
}

#[tokio::test]
async fn test_command_with_exit_code() {
    let args = BashInput { command: "exit 42".to_string(), timeout: None, description: None, run_in_background: None };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Completed(output) => {
            assert_eq!(output.exit_code, 42);
            assert_eq!(output.killed, Some(false));
        }
        BashResult::Background(_) => panic!("Expected completed result, got background"),
    }
}

#[tokio::test]
async fn test_command_with_stderr() {
    let args = BashInput {
        command: "echo 'error' >&2".to_string(),
        timeout: None,
        description: None,
        run_in_background: None,
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Completed(output) => {
            assert_eq!(output.output.trim(), "error");
            assert_eq!(output.exit_code, 0);
        }
        BashResult::Background(_) => panic!("Expected completed result, got background"),
    }
}

#[tokio::test]
async fn test_command_timeout() {
    let args = BashInput {
        command: "sleep 10".to_string(),
        timeout: Some(100), // 100ms timeout
        description: None,
        run_in_background: None,
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Completed(output) => {
            assert!(output.output.contains("timed out"));
            assert_eq!(output.exit_code, -1);
            assert_eq!(output.killed, Some(true));
        }
        BashResult::Background(_) => panic!("Expected completed result, got background"),
    }
}

#[tokio::test]
async fn test_timeout_validation() {
    let args = BashInput {
        command: "echo test".to_string(),
        timeout: Some(700_000), // Exceeds max of 600000
        description: None,
        run_in_background: None,
    };

    let result = execute_command(args).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), BashError::TimeoutTooLarge));
}

#[tokio::test]
async fn test_background_process() {
    let args = BashInput {
        command: "echo 'line1'; sleep 0.1; echo 'line2'".to_string(),
        timeout: None,
        description: Some("Test background command".to_string()),
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            assert!(!handle.shell_id.is_empty());

            let (result1, mut handle_opt) = read_background_bash(handle, None).await.unwrap();
            assert!(result1.status == "running" || result1.status == "completed");

            let mut combined_output = result1.output.clone();
            let (final_status, final_exit) = loop {
                let Some(handle) = handle_opt.take() else {
                    panic!("expected at least one read iteration");
                };
                let (result, next) = read_background_bash(handle, None).await.unwrap();
                combined_output.push_str(&result.output);
                match next {
                    Some(h) => {
                        handle_opt = Some(h);
                        tokio::task::yield_now().await;
                    }
                    None => break (result.status, result.exit_code),
                }
            };

            assert_eq!(final_status, "completed");
            assert_eq!(final_exit, Some(0));
            assert!(combined_output.contains("line1"));
            assert!(combined_output.contains("line2"));
        }
        BashResult::Completed(_) => panic!("Expected background result, got completed"),
    }
}

#[tokio::test]
async fn test_background_process_with_timeout() {
    let args = BashInput {
        command: "sleep 10".to_string(),
        timeout: Some(100), // 100ms timeout
        description: None,
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            // Wait for timeout to occur
            tokio::time::sleep(Duration::from_millis(200)).await;

            let (result, _) = read_background_bash(handle, None).await.unwrap();

            assert_eq!(result.status, "failed");
            assert!(result.output.contains("timed out"));
        }
        BashResult::Completed(_) => panic!("Expected background result, got completed"),
    }
}

#[tokio::test]
async fn test_rm_command_blocked() {
    let args = BashInput { command: "rm".to_string(), timeout: None, description: None, run_in_background: None };

    let result = execute_command(args).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), BashError::Forbidden(_)));
}

#[tokio::test]
async fn test_read_background_bash() {
    let args = BashInput {
        command: "echo 'line1'; sleep 0.1; echo 'line2'; echo 'error' >&2".to_string(),
        timeout: None,
        description: None,
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            // Wait a bit for output to be generated
            tokio::time::sleep(Duration::from_millis(200)).await;

            let (result, _) = read_background_bash(handle, None).await.unwrap();

            assert!(result.output.contains("line1"));
            assert!(result.output.contains("line2"));
            assert!(result.output.contains("error"));
            assert_eq!(result.status, "completed");
            assert_eq!(result.exit_code, Some(0));
        }
        BashResult::Completed(_) => panic!("Expected background result"),
    }
}

#[tokio::test]
async fn test_read_background_bash_with_filter() {
    let args = BashInput {
        command: "echo 'ERROR: something went wrong'; echo 'INFO: all good'; echo 'ERROR: another issue'".to_string(),
        timeout: None,
        description: None,
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            // Wait a bit for output to be generated
            tokio::time::sleep(Duration::from_millis(100)).await;

            let (result, _) = read_background_bash(handle, Some("ERROR".to_string())).await.unwrap();

            assert!(result.output.contains("ERROR: something went wrong"));
            assert!(result.output.contains("ERROR: another issue"));
            assert!(!result.output.contains("INFO: all good"));
            assert_eq!(result.status, "completed");
        }
        BashResult::Completed(_) => panic!("Expected background result"),
    }
}

#[tokio::test]
async fn test_read_background_bash_running_status() {
    let args = BashInput {
        command: "echo 'start'; sleep 10; echo 'end'".to_string(),
        timeout: None,
        description: None,
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            // Give the echo output time to propagate through the async pipe
            tokio::time::sleep(Duration::from_millis(500)).await;

            let (result, _) = read_background_bash(handle, None).await.unwrap();

            assert!(result.output.contains("start"));
            assert_eq!(result.status, "running");
            assert_eq!(result.exit_code, None);
        }
        BashResult::Completed(_) => panic!("Expected background result"),
    }
}

#[tokio::test]
async fn test_read_background_bash_failed_status() {
    let args = BashInput {
        command: "sleep 10".to_string(),
        timeout: Some(100), // Will timeout
        description: None,
        run_in_background: Some(true),
    };

    let result = execute_command(args).await.unwrap();

    match result {
        BashResult::Background(handle) => {
            // Wait for timeout
            tokio::time::sleep(Duration::from_millis(200)).await;

            let (result, _) = read_background_bash(handle, None).await.unwrap();

            assert_eq!(result.status, "failed");
            assert!(result.exit_code.is_some());
        }
        BashResult::Completed(_) => panic!("Expected background result"),
    }
}
